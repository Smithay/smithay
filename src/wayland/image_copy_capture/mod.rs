//! Image copy capture protocol (screencopy)
//!
//! This module implements the `ext-image-copy-capture-v1` protocol, which allows
//! clients to capture the contents of outputs, toplevels, or other image sources.
//!
//! ## Overview
//!
//! The protocol works as follows:
//!
//! 1. Client creates a capture session from an [`ImageCaptureSource`]
//! 2. Compositor sends buffer constraints (size, formats) via the session
//! 3. Client allocates a matching buffer
//! 4. Client creates a frame, attaches the buffer, and requests capture
//! 5. Compositor renders to the buffer and signals completion
//!
//! ## How to use it
//!
//! ```no_run
//! use smithay::delegate_image_copy_capture;
//! use smithay::delegate_image_capture_source;
//! use smithay::delegate_output_capture_source;
//! use smithay::wayland::image_copy_capture::{
//!     ImageCopyCaptureState, ImageCopyCaptureHandler, BufferConstraints,
//!     Session, SessionRef, Frame,
//! };
//! use smithay::wayland::image_capture_source::{
//!     ImageCaptureSourceState, ImageCaptureSourceHandler, ImageCaptureSource,
//!     OutputCaptureSourceState, OutputCaptureSourceHandler,
//! };
//! use smithay::output::Output;
//!
//! pub struct State {
//!     image_capture_source: ImageCaptureSourceState,
//!     output_capture_source: OutputCaptureSourceState,
//!     image_copy_capture: ImageCopyCaptureState,
//! }
//!
//! impl ImageCopyCaptureHandler for State {
//!     fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
//!         &mut self.image_copy_capture
//!     }
//!
//!     fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
//!         // Return buffer constraints for the given source
//!         // Return None to reject capture of this source
//!         todo!()
//!     }
//!
//!     fn new_session(&mut self, session: Session) {
//!         // Store the session for later use
//!     }
//!
//!     fn frame(&mut self, session: &SessionRef, frame: Frame) {
//!         // Perform the actual capture
//!         // Call frame.success(...) or frame.fail(...) when done
//!     }
//! }
//!
//! # impl ImageCaptureSourceHandler for State {}
//! # impl OutputCaptureSourceHandler for State {
//! #     fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
//! #         &mut self.output_capture_source
//! #     }
//! #     fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
//! #         source.user_data().insert_if_missing(|| output.downgrade());
//! #     }
//! # }
//! # smithay::delegate_image_capture_source!(State);
//! # smithay::delegate_output_capture_source!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! let state = ImageCopyCaptureState::new::<State>(&display_handle);
//!
//! delegate_image_copy_capture!(State);
//! ```
//!
//! ## Session Lifecycle
//!
//! Sessions are managed using RAII patterns:
//!
//! - [`Session`] is an owned wrapper that sends `stopped` when dropped
//! - [`SessionRef`] is a cloneable reference for passing to frame handlers
//! - When a session is stopped (by dropping or explicit `stop()`), all pending frames are failed
//!
//! The same pattern applies to [`CursorSession`] and [`CursorSessionRef`].

// Based on cosmic-comp's screencopy implementation by Victoria Brekenfeld (@Drakulix)
// and Ian Douglas Scott (@ids1024) at System76.
// Original source: https://github.com/pop-os/cosmic-comp/blob/master/src/wayland/protocols/screencopy.rs

use std::{
    ops,
    sync::{Arc, Mutex},
    time::Duration,
};

use wayland_protocols::ext::image_copy_capture::v1::server::{
    ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason},
    ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};
use wayland_server::{
    backend::GlobalId,
    protocol::{wl_buffer::WlBuffer, wl_shm},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};

#[cfg(feature = "backend_drm")]
use crate::backend::{
    allocator::{Buffer as AllocBuffer, Fourcc, Modifier},
    drm::DrmNode,
};
use crate::utils::{user_data::UserDataMap, Buffer as BufferCoords, IsAlive, Rectangle, Size, Transform};
use crate::wayland::image_capture_source::ImageCaptureSource;

// Buffer validation imports
use crate::backend::renderer::{buffer_type, BufferType};
#[cfg(feature = "backend_drm")]
use crate::wayland::dmabuf::get_dmabuf;
use crate::wayland::shm::with_buffer_contents;

// Re-export FailureReason for convenience
pub use wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::FailureReason as CaptureFailureReason;

/// Buffer constraints that the compositor requires for capture.
///
/// Clients must allocate buffers matching these constraints.
#[derive(Debug, Clone)]
pub struct BufferConstraints {
    /// Required buffer size in pixels.
    pub size: Size<i32, BufferCoords>,
    /// Supported SHM buffer formats.
    pub shm: Vec<wl_shm::Format>,
    /// DMA-BUF constraints, if DMA-BUF capture is supported.
    #[cfg(feature = "backend_drm")]
    pub dma: Option<DmabufConstraints>,
}

/// DMA-BUF specific constraints.
#[cfg(feature = "backend_drm")]
#[derive(Debug, Clone)]
pub struct DmabufConstraints {
    /// The DRM render node for buffer allocation.
    pub node: DrmNode,
    /// Supported format/modifier combinations.
    pub formats: Vec<(Fourcc, Vec<Modifier>)>,
}

// ============================================================================
// Buffer validation
// ============================================================================

/// Validate that a buffer meets the capture constraints.
///
/// Returns `Ok(())` if the buffer is valid, or a `FailureReason` if not.
fn validate_buffer(buffer: &WlBuffer, constraints: &BufferConstraints) -> Result<(), FailureReason> {
    match buffer_type(buffer) {
        Some(BufferType::Shm) => validate_shm_buffer(buffer, constraints),
        #[cfg(feature = "backend_drm")]
        Some(BufferType::Dma) => validate_dmabuf(buffer, constraints),
        _ => Err(FailureReason::BufferConstraints),
    }
}

/// Validate an SHM buffer against constraints.
fn validate_shm_buffer(buffer: &WlBuffer, constraints: &BufferConstraints) -> Result<(), FailureReason> {
    with_buffer_contents(buffer, |_, _, data| {
        if data.width < constraints.size.w || data.height < constraints.size.h {
            return Err(FailureReason::BufferConstraints);
        }

        if !constraints.shm.contains(&data.format) {
            return Err(FailureReason::BufferConstraints);
        }

        Ok(())
    })
    .map_err(|_| FailureReason::BufferConstraints)?
}

/// Validate a DMA-BUF against constraints.
#[cfg(feature = "backend_drm")]
fn validate_dmabuf(buffer: &WlBuffer, constraints: &BufferConstraints) -> Result<(), FailureReason> {
    let dmabuf = get_dmabuf(buffer).map_err(|_| FailureReason::BufferConstraints)?;

    let dma_constraints = constraints.dma.as_ref().ok_or(FailureReason::BufferConstraints)?;

    let size = dmabuf.size();
    if size.w < constraints.size.w || size.h < constraints.size.h {
        return Err(FailureReason::BufferConstraints);
    }

    let format = dmabuf.format();
    let format_valid = dma_constraints
        .formats
        .iter()
        .any(|(f, modifiers)| *f == format.code && modifiers.contains(&format.modifier));

    if !format_valid {
        return Err(FailureReason::BufferConstraints);
    }

    Ok(())
}

// ============================================================================
// Session types
// ============================================================================

/// Inner state for a capture session.
#[derive(Debug)]
struct SessionInner {
    stopped: bool,
    constraints: Option<BufferConstraints>,
    draw_cursors: bool,
    source: ImageCaptureSource,
    active_frames: Vec<FrameRef>,
}

impl SessionInner {
    fn new(source: ImageCaptureSource, draw_cursors: bool) -> Self {
        Self {
            stopped: false,
            constraints: None,
            draw_cursors,
            source,
            active_frames: Vec::new(),
        }
    }
}

/// A cloneable reference to a capture session.
///
/// Use this to check session state or access the capture source.
/// For methods that might stop the session, use the owned [`Session`].
#[derive(Debug, Clone)]
pub struct SessionRef {
    obj: ExtImageCopyCaptureSessionV1,
    inner: Arc<Mutex<SessionInner>>,
    user_data: Arc<UserDataMap>,
}

impl PartialEq for SessionRef {
    fn eq(&self, other: &Self) -> bool {
        self.obj == other.obj
    }
}

impl IsAlive for SessionRef {
    fn alive(&self) -> bool {
        self.obj.is_alive()
    }
}

impl SessionRef {
    /// Update the buffer constraints for this session.
    ///
    /// This sends the constraint events to the client and stores them
    /// for frame validation.
    pub fn update_constraints(&self, constraints: BufferConstraints) {
        let mut inner = self.inner.lock().unwrap();

        if !self.obj.is_alive() || inner.stopped {
            return;
        }

        self.obj
            .buffer_size(constraints.size.w as u32, constraints.size.h as u32);

        for fmt in &constraints.shm {
            self.obj.shm_format(*fmt);
        }

        #[cfg(feature = "backend_drm")]
        if let Some(dma) = constraints.dma.as_ref() {
            let node = Vec::from(dma.node.dev_id().to_ne_bytes());
            self.obj.dmabuf_device(node);
            for (fmt, modifiers) in &dma.formats {
                let modifiers = modifiers
                    .iter()
                    .flat_map(|modifier| u64::from(*modifier).to_ne_bytes())
                    .collect::<Vec<u8>>();
                self.obj.dmabuf_format(*fmt as u32, modifiers);
            }
        }

        self.obj.done();
        inner.constraints = Some(constraints);
    }

    /// Get the current buffer constraints, if set.
    pub fn current_constraints(&self) -> Option<BufferConstraints> {
        self.inner.lock().unwrap().constraints.clone()
    }

    /// Get the capture source for this session.
    pub fn source(&self) -> ImageCaptureSource {
        self.inner.lock().unwrap().source.clone()
    }

    /// Whether the compositor should draw cursors in the captured content.
    pub fn draw_cursor(&self) -> bool {
        self.inner.lock().unwrap().draw_cursors
    }

    /// Access the [`UserDataMap`] for storing compositor-specific session data.
    pub fn user_data(&self) -> &UserDataMap {
        &self.user_data
    }
}

/// An owned capture session.
///
/// When dropped, this session will be stopped and all pending frames will be failed.
/// The compositor receives sessions via [`ImageCopyCaptureHandler::new_session`] and
/// should store them for the duration of the capture.
#[derive(Debug)]
pub struct Session(SessionRef);

impl ops::Deref for Session {
    type Target = SessionRef;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq<SessionRef> for Session {
    fn eq(&self, other: &SessionRef) -> bool {
        self.0 == *other
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut inner = self.0.inner.lock().unwrap();

        if !self.0.obj.is_alive() || inner.stopped {
            return;
        }

        // Fail all active frames
        for frame in inner.active_frames.drain(..) {
            frame
                .inner
                .lock()
                .unwrap()
                .fail(&frame.obj, FailureReason::Stopped);
        }

        self.0.obj.stopped();
        inner.constraints.take();
        inner.stopped = true;
    }
}

impl Session {
    /// Explicitly stop this session.
    ///
    /// This is equivalent to dropping the session.
    pub fn stop(self) {
        drop(self);
    }

    /// Get a cloneable reference to this session.
    pub fn as_ref(&self) -> SessionRef {
        self.0.clone()
    }
}

// ============================================================================
// Cursor session types
// ============================================================================

/// Inner state for a cursor capture session.
#[derive(Debug)]
struct CursorSessionInner {
    session_obj: Option<ExtImageCopyCaptureSessionV1>,
    stopped: bool,
    constraints: Option<BufferConstraints>,
    source: ImageCaptureSource,
    position: Option<crate::utils::Point<i32, BufferCoords>>,
    hotspot: crate::utils::Point<i32, BufferCoords>,
    active_frames: Vec<FrameRef>,
}

impl CursorSessionInner {
    fn new(source: ImageCaptureSource) -> Self {
        Self {
            session_obj: None,
            stopped: false,
            constraints: None,
            source,
            position: None,
            hotspot: crate::utils::Point::from((0, 0)),
            active_frames: Vec::new(),
        }
    }
}

/// A cloneable reference to a cursor capture session.
#[derive(Debug, Clone)]
pub struct CursorSessionRef {
    obj: ExtImageCopyCaptureCursorSessionV1,
    inner: Arc<Mutex<CursorSessionInner>>,
    user_data: Arc<UserDataMap>,
}

impl PartialEq for CursorSessionRef {
    fn eq(&self, other: &Self) -> bool {
        self.obj == other.obj
    }
}

impl IsAlive for CursorSessionRef {
    fn alive(&self) -> bool {
        self.obj.is_alive()
    }
}

impl CursorSessionRef {
    /// Update the buffer constraints for cursor capture.
    pub fn update_constraints(&self, constraints: BufferConstraints) {
        let mut inner = self.inner.lock().unwrap();

        if !self.obj.is_alive() || inner.stopped {
            return;
        }

        if let Some(session_obj) = inner.session_obj.as_ref() {
            session_obj.buffer_size(constraints.size.w as u32, constraints.size.h as u32);
            for fmt in &constraints.shm {
                session_obj.shm_format(*fmt);
            }
            #[cfg(feature = "backend_drm")]
            if let Some(dma) = constraints.dma.as_ref() {
                let node = Vec::from(dma.node.dev_id().to_ne_bytes());
                session_obj.dmabuf_device(node);
                for (fmt, modifiers) in &dma.formats {
                    let modifiers = modifiers
                        .iter()
                        .flat_map(|modifier| u64::from(*modifier).to_ne_bytes())
                        .collect::<Vec<u8>>();
                    session_obj.dmabuf_format(*fmt as u32, modifiers);
                }
            }
            session_obj.done();
        }

        inner.constraints = Some(constraints);
    }

    /// Get the current buffer constraints, if set.
    pub fn current_constraints(&self) -> Option<BufferConstraints> {
        self.inner.lock().unwrap().constraints.clone()
    }

    /// Get the capture source for this session.
    pub fn source(&self) -> ImageCaptureSource {
        self.inner.lock().unwrap().source.clone()
    }

    /// Whether the cursor is currently on this capture source.
    pub fn has_cursor(&self) -> bool {
        self.inner.lock().unwrap().position.is_some()
    }

    /// Update the cursor position on this capture source.
    ///
    /// Pass `None` when the cursor leaves the capture source.
    pub fn set_cursor_pos(&self, position: Option<crate::utils::Point<i32, BufferCoords>>) {
        if !self.obj.is_alive() {
            return;
        }

        let mut inner = self.inner.lock().unwrap();

        if inner.position == position {
            return;
        }

        if inner.position.is_none() && position.is_some() {
            self.obj.enter();
            self.obj.hotspot(inner.hotspot.x, inner.hotspot.y);
        }

        if let Some(new_pos) = position {
            self.obj.position(new_pos.x, new_pos.y);
        } else if inner.position.is_some() {
            self.obj.leave();
        }

        inner.position = position;
    }

    /// Update the cursor hotspot.
    pub fn set_cursor_hotspot(&self, hotspot: impl Into<crate::utils::Point<i32, BufferCoords>>) {
        if !self.obj.is_alive() {
            return;
        }

        let hotspot = hotspot.into();
        let mut inner = self.inner.lock().unwrap();

        if inner.hotspot == hotspot {
            return;
        }

        inner.hotspot = hotspot;
        if inner.position.is_some() {
            self.obj.hotspot(hotspot.x, hotspot.y);
        }
    }

    /// Access the [`UserDataMap`] for storing compositor-specific session data.
    pub fn user_data(&self) -> &UserDataMap {
        &self.user_data
    }
}

/// An owned cursor capture session.
#[derive(Debug)]
pub struct CursorSession(CursorSessionRef);

impl ops::Deref for CursorSession {
    type Target = CursorSessionRef;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq<CursorSessionRef> for CursorSession {
    fn eq(&self, other: &CursorSessionRef) -> bool {
        self.0 == *other
    }
}

impl Drop for CursorSession {
    fn drop(&mut self) {
        let mut inner = self.0.inner.lock().unwrap();

        if !self.0.obj.is_alive() || inner.stopped {
            return;
        }

        if let Some(session_obj) = inner.session_obj.as_ref() {
            session_obj.stopped();
        }
        inner.constraints.take();

        for frame in inner.active_frames.drain(..) {
            frame
                .inner
                .lock()
                .unwrap()
                .fail(&frame.obj, FailureReason::Stopped);
        }

        inner.stopped = true;
    }
}

impl CursorSession {
    /// Explicitly stop this session.
    pub fn stop(self) {
        drop(self);
    }

    /// Get a cloneable reference to this session.
    pub fn as_ref(&self) -> CursorSessionRef {
        self.0.clone()
    }
}

// ============================================================================
// Frame types
// ============================================================================

/// Inner state for a capture frame.
#[derive(Debug)]
struct FrameInner {
    /// Buffer constraints for validation.
    constraints: Option<BufferConstraints>,
    buffer: Option<WlBuffer>,
    damage: Vec<Rectangle<i32, BufferCoords>>,
    /// Reference to parent session (stored for future use).
    #[allow(dead_code)]
    session_obj: Weak<ExtImageCopyCaptureSessionV1>,
    capture_requested: bool,
    failed: Option<FailureReason>,
    ready: bool,
}

impl FrameInner {
    fn new(session_obj: ExtImageCopyCaptureSessionV1, constraints: Option<BufferConstraints>) -> Self {
        Self {
            constraints,
            buffer: None,
            damage: Vec::new(),
            session_obj: session_obj.downgrade(),
            capture_requested: false,
            failed: None,
            ready: false,
        }
    }

    fn fail(&mut self, frame: &ExtImageCopyCaptureFrameV1, reason: FailureReason) {
        if self.ready || self.failed.is_some() {
            return;
        }
        self.failed = Some(reason);
        if self.capture_requested {
            frame.failed(reason);
        }
    }
}

/// A cloneable reference to a capture frame.
///
/// Used for tracking active frames within a session.
#[derive(Clone, Debug)]
pub struct FrameRef {
    obj: ExtImageCopyCaptureFrameV1,
    inner: Arc<Mutex<FrameInner>>,
}

impl PartialEq for FrameRef {
    fn eq(&self, other: &Self) -> bool {
        self.obj == other.obj
    }
}

impl FrameRef {
    /// Get the buffer attached to this frame.
    ///
    /// # Panics
    ///
    /// Panics if no buffer has been attached (should only be called after
    /// the capture request has been received).
    pub fn buffer(&self) -> WlBuffer {
        self.inner
            .lock()
            .unwrap()
            .buffer
            .clone()
            .expect("no buffer attached")
    }

    /// Get the damage regions for this frame.
    pub fn damage(&self) -> Vec<Rectangle<i32, BufferCoords>> {
        self.inner.lock().unwrap().damage.clone()
    }

    /// Check if this frame has already failed.
    pub fn has_failed(&self) -> bool {
        self.inner.lock().unwrap().failed.is_some()
    }
}

/// An owned capture frame.
///
/// The compositor receives frames via [`ImageCopyCaptureHandler::frame`] and
/// must call either [`Frame::success`] or [`Frame::fail`] to complete the capture.
///
/// If the frame is dropped without calling either method, it will automatically fail.
#[derive(Debug)]
pub struct Frame(FrameRef);

impl ops::Deref for Frame {
    type Target = FrameRef;
    fn deref(&self) -> &FrameRef {
        &self.0
    }
}

impl PartialEq<FrameRef> for Frame {
    fn eq(&self, other: &FrameRef) -> bool {
        self.0 == *other
    }
}

impl Frame {
    /// Signal successful capture.
    ///
    /// # Arguments
    ///
    /// * `transform` - The transform applied to the captured content
    /// * `damage` - Optional damage regions, or `None` for full damage
    /// * `presented` - The presentation timestamp
    pub fn success(
        self,
        transform: impl Into<Transform>,
        damage: impl Into<Option<Vec<Rectangle<i32, BufferCoords>>>>,
        presented: impl Into<Duration>,
    ) {
        {
            let inner = self.0.inner.lock().unwrap();
            if !inner.capture_requested || inner.failed.is_some() {
                return;
            }
        }

        self.0.obj.transform(transform.into().into());
        for damage in damage.into().into_iter().flatten() {
            self.0
                .obj
                .damage(damage.loc.x, damage.loc.y, damage.size.w, damage.size.h);
        }

        let time = presented.into();
        let tv_sec_hi = (time.as_secs() >> 32) as u32;
        let tv_sec_lo = (time.as_secs() & 0xFFFFFFFF) as u32;
        let tv_nsec = time.subsec_nanos();
        self.0.obj.presentation_time(tv_sec_hi, tv_sec_lo, tv_nsec);

        self.0.inner.lock().unwrap().ready = true;
        self.0.obj.ready();

        // Prevent drop from sending fail
        std::mem::forget(self);
    }

    /// Signal failed capture.
    pub fn fail(self, reason: FailureReason) {
        self.0.inner.lock().unwrap().fail(&self.0.obj, reason);
        // Prevent drop from sending fail again
        std::mem::forget(self);
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // If success() or fail() wasn't called, send Unknown failure
        self.0
            .inner
            .lock()
            .unwrap()
            .fail(&self.0.obj, FailureReason::Unknown);
    }
}

// ============================================================================
// Handler trait
// ============================================================================

/// Handler trait for the image copy capture protocol.
///
/// Implement this on your compositor's state type to handle capture requests.
pub trait ImageCopyCaptureHandler:
    GlobalDispatch<ExtImageCopyCaptureManagerV1, ImageCopyCaptureGlobalData>
    + Dispatch<ExtImageCopyCaptureManagerV1, ()>
    + Dispatch<ExtImageCopyCaptureSessionV1, SessionData>
    + Dispatch<ExtImageCopyCaptureSessionV1, CursorSessionData>
    + Dispatch<ExtImageCopyCaptureCursorSessionV1, CursorSessionData>
    + Dispatch<ExtImageCopyCaptureFrameV1, FrameData>
    + 'static
{
    /// Returns a mutable reference to the [`ImageCopyCaptureState`] delegate type.
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState;

    /// Return buffer constraints for capturing the given source.
    ///
    /// Return `None` to reject capture of this source (e.g., if permissions
    /// are not granted or the source is invalid).
    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints>;

    /// Return buffer constraints for capturing the cursor on the given source.
    ///
    /// Return `None` if cursor capture is not supported for this source.
    fn cursor_capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        let _ = source;
        None
    }

    /// Called when a new capture session is created.
    ///
    /// The compositor should store this session and call
    /// [`SessionRef::update_constraints`] when the capture constraints change.
    fn new_session(&mut self, session: Session);

    /// Called when a new cursor capture session is created.
    fn new_cursor_session(&mut self, session: CursorSession) {
        let _ = session;
    }

    /// Called when a frame capture is requested.
    ///
    /// The compositor should render the captured content to the frame's buffer
    /// and call either [`Frame::success`] or [`Frame::fail`].
    fn frame(&mut self, session: &SessionRef, frame: Frame);

    /// Called when a cursor frame capture is requested.
    fn cursor_frame(&mut self, session: &CursorSessionRef, frame: Frame) {
        let _ = session;
        frame.fail(FailureReason::Unknown);
    }

    /// Called when a frame is aborted (e.g., buffer destroyed before capture)
    /// or the client disconnects.
    ///
    /// Note: In case of implicit destruction the order is undefined and might
    /// not follow explicit protocol definitions.
    fn frame_aborted(&mut self, frame: FrameRef) {
        let _ = frame;
    }

    /// Called when a session is destroyed.
    ///
    /// Note: Destruction might happen explicitly by the client, or implicitly
    /// when the client quits. In case of implicit destruction the order the
    /// callbacks are called in is undefined.
    fn session_destroyed(&mut self, session: SessionRef) {
        let _ = session;
    }

    /// Called when a cursor session is destroyed.
    ///
    /// Note: Destruction might happen explicitly by the client, or implicitly
    /// when the client quits. In case of implicit destruction the order the
    /// callbacks are called in is undefined.
    fn cursor_session_destroyed(&mut self, session: CursorSessionRef) {
        let _ = session;
    }
}

// ============================================================================
// State and data types
// ============================================================================

/// Data associated with the image copy capture manager global.
#[allow(missing_debug_implementations)]
pub struct ImageCopyCaptureGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

impl std::fmt::Debug for ImageCopyCaptureGlobalData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageCopyCaptureGlobalData")
            .finish_non_exhaustive()
    }
}

/// User data for session protocol resources.
#[derive(Debug)]
pub struct SessionData {
    inner: Arc<Mutex<SessionInner>>,
    user_data: Arc<UserDataMap>,
}

/// User data for cursor session protocol resources.
#[derive(Debug)]
pub struct CursorSessionData {
    inner: Arc<Mutex<CursorSessionInner>>,
    user_data: Arc<UserDataMap>,
}

/// User data for frame protocol resources.
#[derive(Debug)]
pub struct FrameData {
    inner: Arc<Mutex<FrameInner>>,
}

/// State of the image copy capture protocol.
#[derive(Debug)]
pub struct ImageCopyCaptureState {
    global: GlobalId,
    sessions: Vec<SessionRef>,
    cursor_sessions: Vec<CursorSessionRef>,
}

impl ImageCopyCaptureState {
    /// Register a new [`ExtImageCopyCaptureManagerV1`] global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: ImageCopyCaptureHandler,
    {
        Self::new_with_filter::<D, _>(display, |_| true)
    }

    /// Register a new [`ExtImageCopyCaptureManagerV1`] global with a client filter.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: ImageCopyCaptureHandler,
        F: Fn(&Client) -> bool + Clone + Send + Sync + 'static,
    {
        let global = display.create_global::<D, ExtImageCopyCaptureManagerV1, _>(
            1,
            ImageCopyCaptureGlobalData {
                filter: Box::new(filter),
            },
        );

        Self {
            global,
            sessions: Vec::new(),
            cursor_sessions: Vec::new(),
        }
    }

    /// Get the [`GlobalId`] of the image copy capture manager.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    /// Get all active sessions.
    pub fn sessions(&self) -> &[SessionRef] {
        &self.sessions
    }

    /// Get all active cursor sessions.
    pub fn cursor_sessions(&self) -> &[CursorSessionRef] {
        &self.cursor_sessions
    }

    /// Clean up dead sessions.
    pub fn cleanup(&mut self) {
        self.sessions.retain(|s| s.alive());
        self.cursor_sessions.retain(|s| s.alive());
    }
}

// ============================================================================
// Dispatch implementations
// ============================================================================

impl<D> GlobalDispatch<ExtImageCopyCaptureManagerV1, ImageCopyCaptureGlobalData, D> for ImageCopyCaptureState
where
    D: ImageCopyCaptureHandler,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        _global_data: &ImageCopyCaptureGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &ImageCopyCaptureGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ExtImageCopyCaptureManagerV1, (), D> for ImageCopyCaptureState
where
    D: ImageCopyCaptureHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_image_copy_capture_manager_v1::Request::CreateSession {
                session,
                source,
                options,
            } => {
                let Some(capture_source) = ImageCaptureSource::from_resource(&source) else {
                    // Invalid source - create stopped session
                    let inner = Arc::new(Mutex::new(SessionInner {
                        stopped: true,
                        constraints: None,
                        draw_cursors: false,
                        source: ImageCaptureSource::new(),
                        active_frames: Vec::new(),
                    }));
                    let user_data = Arc::new(UserDataMap::new());
                    let obj = data_init.init(
                        session,
                        SessionData {
                            inner: inner.clone(),
                            user_data: user_data.clone(),
                        },
                    );
                    obj.stopped();
                    return;
                };

                let draw_cursors = options
                    .into_result()
                    .map(|o| o.contains(ext_image_copy_capture_manager_v1::Options::PaintCursors))
                    .unwrap_or(false);
                let inner = Arc::new(Mutex::new(SessionInner::new(
                    capture_source.clone(),
                    draw_cursors,
                )));
                let user_data = Arc::new(UserDataMap::new());

                let obj = data_init.init(
                    session,
                    SessionData {
                        inner: inner.clone(),
                        user_data: user_data.clone(),
                    },
                );

                let session_ref = SessionRef {
                    obj,
                    inner,
                    user_data,
                };

                if let Some(constraints) = state.capture_constraints(&capture_source) {
                    session_ref.update_constraints(constraints);
                    state
                        .image_copy_capture_state()
                        .sessions
                        .push(session_ref.clone());
                    state.new_session(Session(session_ref));
                } else {
                    // Source rejected
                    session_ref.obj.stopped();
                    session_ref.inner.lock().unwrap().stopped = true;
                }
            }
            ext_image_copy_capture_manager_v1::Request::CreatePointerCursorSession {
                session,
                source,
                pointer: _,
            } => {
                let Some(capture_source) = ImageCaptureSource::from_resource(&source) else {
                    // Invalid source - create stopped session
                    let inner = Arc::new(Mutex::new(CursorSessionInner::new(ImageCaptureSource::new())));
                    inner.lock().unwrap().stopped = true;
                    let user_data = Arc::new(UserDataMap::new());
                    let obj = data_init.init(session, CursorSessionData { inner, user_data });
                    // Note: cursor session doesn't have a stopped event
                    let _ = obj;
                    return;
                };

                let inner = Arc::new(Mutex::new(CursorSessionInner::new(capture_source.clone())));
                let user_data = Arc::new(UserDataMap::new());

                let obj = data_init.init(
                    session,
                    CursorSessionData {
                        inner: inner.clone(),
                        user_data: user_data.clone(),
                    },
                );

                let session_ref = CursorSessionRef {
                    obj,
                    inner,
                    user_data,
                };

                if let Some(constraints) = state.cursor_capture_constraints(&capture_source) {
                    session_ref.update_constraints(constraints);
                    state
                        .image_copy_capture_state()
                        .cursor_sessions
                        .push(session_ref.clone());
                    state.new_cursor_session(CursorSession(session_ref));
                } else {
                    session_ref.inner.lock().unwrap().stopped = true;
                }
            }
            ext_image_copy_capture_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<ExtImageCopyCaptureSessionV1, SessionData, D> for ImageCopyCaptureState
where
    D: ImageCopyCaptureHandler,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        resource: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        data: &SessionData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_image_copy_capture_session_v1::Request::CreateFrame { frame } => {
                let constraints = data.inner.lock().unwrap().constraints.clone();
                let inner = Arc::new(Mutex::new(FrameInner::new(resource.clone(), constraints)));
                let obj = data_init.init(frame, FrameData { inner: inner.clone() });
                data.inner
                    .lock()
                    .unwrap()
                    .active_frames
                    .push(FrameRef { obj, inner });
            }
            ext_image_copy_capture_session_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        resource: &ExtImageCopyCaptureSessionV1,
        data: &SessionData,
    ) {
        let session_ref = SessionRef {
            obj: resource.clone(),
            inner: data.inner.clone(),
            user_data: data.user_data.clone(),
        };
        state.session_destroyed(session_ref);
    }
}

// Dispatch for session created from cursor session's get_capture_session
impl<D> Dispatch<ExtImageCopyCaptureSessionV1, CursorSessionData, D> for ImageCopyCaptureState
where
    D: ImageCopyCaptureHandler,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        resource: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        data: &CursorSessionData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_image_copy_capture_session_v1::Request::CreateFrame { frame } => {
                let constraints = data.inner.lock().unwrap().constraints.clone();
                let inner = Arc::new(Mutex::new(FrameInner::new(resource.clone(), constraints)));
                let obj = data_init.init(frame, FrameData { inner: inner.clone() });
                data.inner
                    .lock()
                    .unwrap()
                    .active_frames
                    .push(FrameRef { obj, inner });
            }
            ext_image_copy_capture_session_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<ExtImageCopyCaptureCursorSessionV1, CursorSessionData, D> for ImageCopyCaptureState
where
    D: ImageCopyCaptureHandler,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        data: &CursorSessionData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_image_copy_capture_cursor_session_v1::Request::GetCaptureSession { session } => {
                let mut inner = data.inner.lock().unwrap();

                if inner.session_obj.is_some() {
                    // Protocol error: only one session allowed
                    return;
                }

                let obj = data_init.init(
                    session,
                    CursorSessionData {
                        inner: data.inner.clone(),
                        user_data: data.user_data.clone(),
                    },
                );

                if inner.stopped {
                    obj.stopped();
                } else if let Some(constraints) = inner.constraints.as_ref() {
                    obj.buffer_size(constraints.size.w as u32, constraints.size.h as u32);
                    for fmt in &constraints.shm {
                        obj.shm_format(*fmt);
                    }
                    #[cfg(feature = "backend_drm")]
                    if let Some(dma) = constraints.dma.as_ref() {
                        let node = Vec::from(dma.node.dev_id().to_ne_bytes());
                        obj.dmabuf_device(node);
                        for (fmt, modifiers) in &dma.formats {
                            let modifiers = modifiers
                                .iter()
                                .flat_map(|modifier| u64::from(*modifier).to_ne_bytes())
                                .collect::<Vec<u8>>();
                            obj.dmabuf_format(*fmt as u32, modifiers);
                        }
                    }
                    obj.done();
                }

                inner.session_obj = Some(obj);
            }
            ext_image_copy_capture_cursor_session_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        resource: &ExtImageCopyCaptureCursorSessionV1,
        data: &CursorSessionData,
    ) {
        let session_ref = CursorSessionRef {
            obj: resource.clone(),
            inner: data.inner.clone(),
            user_data: data.user_data.clone(),
        };
        state.cursor_session_destroyed(session_ref);
    }
}

impl<D> Dispatch<ExtImageCopyCaptureFrameV1, FrameData, D> for ImageCopyCaptureState
where
    D: ImageCopyCaptureHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_image_copy_capture_frame_v1::Request::AttachBuffer { buffer } => {
                let mut inner = data.inner.lock().unwrap();
                if inner.capture_requested {
                    // Protocol error: can't attach after capture
                    return;
                }
                inner.buffer = Some(buffer);
            }
            ext_image_copy_capture_frame_v1::Request::DamageBuffer { x, y, width, height } => {
                let mut inner = data.inner.lock().unwrap();
                if inner.capture_requested {
                    return;
                }
                // Validate coordinates
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    return;
                }
                inner.damage.push(Rectangle {
                    loc: (x, y).into(),
                    size: (width, height).into(),
                });
            }
            ext_image_copy_capture_frame_v1::Request::Capture => {
                let frame_ref = FrameRef {
                    obj: resource.clone(),
                    inner: data.inner.clone(),
                };

                {
                    let mut inner = data.inner.lock().unwrap();
                    if inner.capture_requested || inner.failed.is_some() {
                        return;
                    }
                    inner.capture_requested = true;

                    let buffer = match inner.buffer.as_ref() {
                        Some(b) => b,
                        None => {
                            inner.fail(resource, FailureReason::BufferConstraints);
                            return;
                        }
                    };

                    match inner.constraints.as_ref() {
                        Some(constraints) => {
                            if let Err(reason) = validate_buffer(buffer, constraints) {
                                inner.fail(resource, reason);
                                return;
                            }
                        }
                        None => {
                            // Session was never properly initialized
                            inner.fail(resource, FailureReason::BufferConstraints);
                            return;
                        }
                    }
                }

                // Find the session this frame belongs to
                let copy_capture_state = state.image_copy_capture_state();

                // Try regular sessions first
                for session in &copy_capture_state.sessions {
                    let session_inner = session.inner.lock().unwrap();
                    if session_inner.active_frames.iter().any(|f| f == &frame_ref) {
                        drop(session_inner);
                        let session_ref = session.clone();
                        let frame = Frame(frame_ref);
                        state.frame(&session_ref, frame);
                        return;
                    }
                }

                // Try cursor sessions
                for session in &copy_capture_state.cursor_sessions {
                    let session_inner = session.inner.lock().unwrap();
                    if session_inner.active_frames.iter().any(|f| f == &frame_ref) {
                        drop(session_inner);
                        let session_ref = session.clone();
                        let frame = Frame(frame_ref);
                        state.cursor_frame(&session_ref, frame);
                        return;
                    }
                }

                // Frame not found in any session
                data.inner.lock().unwrap().fail(resource, FailureReason::Unknown);
            }
            ext_image_copy_capture_frame_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        resource: &ExtImageCopyCaptureFrameV1,
        data: &FrameData,
    ) {
        let frame_ref = FrameRef {
            obj: resource.clone(),
            inner: data.inner.clone(),
        };

        // Remove from active frames in sessions
        for session in &state.image_copy_capture_state().sessions {
            session
                .inner
                .lock()
                .unwrap()
                .active_frames
                .retain(|f| f != &frame_ref);
        }
        for session in &state.image_copy_capture_state().cursor_sessions {
            session
                .inner
                .lock()
                .unwrap()
                .active_frames
                .retain(|f| f != &frame_ref);
        }

        state.frame_aborted(frame_ref);
    }
}

// ============================================================================
// Delegate macro
// ============================================================================

/// Macro to delegate implementation of the image copy capture protocol to [`ImageCopyCaptureState`].
///
/// You must also implement [`ImageCopyCaptureHandler`] to use this.
#[macro_export]
macro_rules! delegate_image_copy_capture {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        const _: () = {
            use $crate::reexports::wayland_protocols::ext::image_copy_capture::v1::server::{
                ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1,
                ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
                ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
                ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
            };
            use $crate::reexports::wayland_server::{delegate_dispatch, delegate_global_dispatch};
            use $crate::wayland::image_copy_capture::{
                ImageCopyCaptureGlobalData, ImageCopyCaptureState,
                SessionData, CursorSessionData, FrameData,
            };

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCopyCaptureManagerV1: ImageCopyCaptureGlobalData] => ImageCopyCaptureState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCopyCaptureManagerV1: ()] => ImageCopyCaptureState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCopyCaptureSessionV1: SessionData] => ImageCopyCaptureState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCopyCaptureSessionV1: CursorSessionData] => ImageCopyCaptureState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCopyCaptureCursorSessionV1: CursorSessionData] => ImageCopyCaptureState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCopyCaptureFrameV1: FrameData] => ImageCopyCaptureState
            );
        };
    };
}
