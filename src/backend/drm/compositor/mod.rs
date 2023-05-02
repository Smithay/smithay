//! Composition for [`Element`]s using drm planes
//!
//! When possible composition can be (partially) offloaded to the display driver by assigning
//! elements to drm planes. This is especially important for latency intensive fullscreen clients
//! like video renderers or games.
//!
//! The [`DrmCompositor`] does so by walking the stack of provided [`Element`]s from front to back
//! while trying to assign each element to a drm overlay plane. Each item that fails the plane test
//! will be rendered on the primary plane using the provided [`Renderer`].
//! Additionally it will try to assign the top most element that fit's into the cursor size (as specified
//! by the [`DrmDevice`](crate::backend::drm::DrmDevice)) on the cursor plane. If the element can not be
//! directly scanned out the renderer will be used to render the element into an [`Offscreen`] buffer that
//! is copied over to the allocated gbm buffer for the cursor plane.
//!
//! Note: While the [`DrmCompositor`] also works on *legacy* drm the use of overlay and cursor planes is disabled in that case.
//! Direct scan-out will only work with an atomic [`DrmSurface`].
//!
//! ## What makes a [`Element`] eligible for direct scan-out
//!
//! ### General
//!
//! First the element has to provide a [`UnderlyingStorage`] which can be exported as a drm framebuffer.
//! Currently this is limited to wayland buffers, but may be extended in the future.
//! This module provides a default exporter based on [`gbm`] which should fit most use-cases.
//!
//! If a certain combination of elements works can only be determined by asking the driver by submitting
//! a atomic commit test. If that test fails the element is scheduled to be rendered on the primary plane.
//!
//! ### Overlay planes
//!
//! The element can only be directly scanned out if it's geometry does not overlap with an already assigned
//! element on a plane higher in the stack.
//!
//! ### Underlay planes
//!
//! An underlay plane is only used if it does not overlap with an already assigned plane lower in the stack
//! and the element is fully opaque.
//!
//! ### Primary plane
//!
//! For an element to be considered to be directly scanned out on the primary plane it has to be the last remaining
//! visible element on the output and no other element has been assigned to the primary plane. If there are multiple
//! element assigned to the primary plane the renderer will be used to composite the primary plane into a allocator
//! provided buffer. Additionally the element has to be either fully opaque or the clear color has to match the CRTC
//! background color and no overlap with an underlay is found.
//!
//! # How to use it
//!
//! ```no_run
//! # use smithay::backend::{
//! #     allocator::gbm::{GbmAllocator, GbmDevice},
//! #     drm::{DrmDevice, DrmDeviceFd},
//! #     renderer::{
//! #       element::surface::WaylandSurfaceRenderElement,
//! #       gles::{GlesTexture, GlesRenderer},
//! #     },
//! # };
//! # use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
//! # use std::{collections::HashSet, mem::MaybeUninit};
//! #
//! use smithay::{
//!     backend::drm::{compositor::DrmCompositor, DrmSurface},
//!     output::{Output, PhysicalProperties, Subpixel},
//!     utils::Size,
//! };
//!
//! // ...initialize the output, drm device, drm surface and allocator
//! #
//! # const CLEAR_COLOR: [f32; 4] = [0f32, 0f32, 0f32, 0f32];
//! #
//! let output = Output::new(
//!     "e-DP".into(),
//!     PhysicalProperties {
//!         size: Size::from((800, 600)),
//!         make: "N/A".into(),
//!         model: "N/A".into(),
//!         subpixel: Subpixel::Unknown,
//!     },
//! );
//!
//! # let device: DrmDevice = todo!();
//! # let surface: DrmSurface = todo!();
//! # let allocator: GbmAllocator<DrmDeviceFd> = todo!();
//! # let exporter: GbmDevice<DrmDeviceFd> = todo!();
//! # let color_formats = &[DrmFourcc::Argb8888];
//! # let renderer_formats = HashSet::from([DrmFormat {
//! #     code: DrmFourcc::Argb8888,
//! #     modifier: DrmModifier::Linear,
//! # }]);
//! # let gbm: GbmDevice<DrmDeviceFd> = todo!();
//! # let mut renderer: GlesRenderer = todo!();
//! #
//! let mut compositor: DrmCompositor<_, _, (), _> = DrmCompositor::new(
//!     &output,
//!     surface,
//!     None,
//!     allocator,
//!     exporter,
//!     color_formats,
//!     renderer_formats,
//!     device.cursor_size(),
//!     Some(gbm),
//! )
//! .expect("failed to initialize drm compositor");
//!
//! # let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
//! let render_frame_result = compositor
//!     .render_frame::<_, _, GlesTexture>(&mut renderer, &elements, CLEAR_COLOR)
//!     .expect("failed to render frame");
//!
//! if render_frame_result.damage.is_some() {
//!     compositor.queue_frame(()).expect("failed to queue frame");
//!
//!     // ...wait for VBlank event
//!
//!     compositor
//!         .frame_submitted()
//!         .expect("failed to mark frame as submitted");
//! } else {
//!     // ...re-schedule frame
//! }
//! ```
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    os::unix::io::AsFd,
    rc::Rc,
    sync::{Arc, Mutex},
};

use ::gbm::{BufferObject, BufferObjectFlags};
use drm::control::{connector, crtc, framebuffer, plane, Mode, PlaneType};
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use indexmap::IndexMap;
use tracing::{debug, error, info, info_span, instrument, trace, warn};
use wayland_server::{protocol::wl_buffer::WlBuffer, Resource};

use crate::{
    backend::{
        allocator::{
            dmabuf::{AsDmabuf, Dmabuf},
            format::get_opaque,
            gbm::{GbmAllocator, GbmDevice},
            Allocator, Buffer, Slot, Swapchain,
        },
        color::{TransformType, Transformation, CMS},
        drm::{DrmError, PlaneDamageClips},
        renderer::{
            buffer_y_inverted,
            damage::{Error as OutputDamageTrackerError, OutputDamageTracker, OutputNoMode},
            element::{
                Element, Id, RenderElement, RenderElementPresentationState, RenderElementState,
                RenderElementStates, RenderingReason, UnderlyingStorage,
            },
            utils::{CommitCounter, DamageBag, DamageSnapshot},
            Bind, Blit, DebugFlags, ExportMem, Frame as RendererFrame, Offscreen, Renderer, Texture,
        },
        SwapBuffersError,
    },
    output::Output,
    utils::{Buffer as BufferCoords, DevPath, Physical, Point, Rectangle, Scale, Size, Transform},
};

use super::{DrmDeviceFd, DrmSurface, PlaneClaim, PlaneInfo, Planes};

mod elements;
pub mod gbm;

use elements::*;

impl RenderElementState {
    pub(crate) fn zero_copy(visible_area: usize) -> Self {
        RenderElementState {
            visible_area,
            presentation_state: RenderElementPresentationState::ZeroCopy,
        }
    }

    pub(crate) fn rendering_with_reason(reason: RenderingReason) -> Self {
        RenderElementState {
            visible_area: 0,
            presentation_state: RenderElementPresentationState::Rendering { reason: Some(reason) },
        }
    }
}

#[derive(Debug)]
enum ScanoutBuffer<B: Buffer> {
    Wayland(crate::backend::renderer::utils::Buffer),
    Swapchain(Slot<B>),
    Cursor(BufferObject<()>),
}

impl<B: Buffer> From<UnderlyingStorage> for ScanoutBuffer<B> {
    fn from(storage: UnderlyingStorage) -> Self {
        match storage {
            UnderlyingStorage::Wayland(buffer) => Self::Wayland(buffer),
        }
    }
}

enum DrmFramebuffer<F: AsRef<framebuffer::Handle>> {
    Exporter(F),
    Gbm(super::gbm::GbmFramebuffer),
}

impl<F> AsRef<framebuffer::Handle> for DrmFramebuffer<F>
where
    F: AsRef<framebuffer::Handle>,
{
    fn as_ref(&self) -> &framebuffer::Handle {
        match self {
            DrmFramebuffer::Exporter(e) => e.as_ref(),
            DrmFramebuffer::Gbm(g) => g.as_ref(),
        }
    }
}

impl<F> std::fmt::Debug for DrmFramebuffer<F>
where
    F: AsRef<framebuffer::Handle> + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exporter(arg0) => f.debug_tuple("Exporter").field(arg0).finish(),
            Self::Gbm(arg0) => f.debug_tuple("Gbm").field(arg0).finish(),
        }
    }
}

struct DrmScanoutBuffer<B: Buffer, F: AsRef<framebuffer::Handle>> {
    buffer: ScanoutBuffer<B>,
    fb: OwnedFramebuffer<DrmFramebuffer<F>>,
}

impl<B, F> std::fmt::Debug for DrmScanoutBuffer<B, F>
where
    B: Buffer + std::fmt::Debug,
    F: AsRef<framebuffer::Handle> + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrmScanoutBuffer")
            .field("buffer", &self.buffer)
            .field("fb", &self.fb)
            .finish()
    }
}

impl<B: Buffer, F: AsRef<framebuffer::Handle>> AsRef<framebuffer::Handle> for DrmScanoutBuffer<B, F> {
    fn as_ref(&self) -> &drm::control::framebuffer::Handle {
        self.fb.as_ref()
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum ElementFramebufferCacheKey {
    Wayland(wayland_server::Weak<WlBuffer>),
}

impl ElementFramebufferCacheKey {
    fn is_alive(&self) -> bool {
        match self {
            ElementFramebufferCacheKey::Wayland(buffer) => buffer.upgrade().is_ok(),
        }
    }
}

impl From<&UnderlyingStorage> for ElementFramebufferCacheKey {
    fn from(storage: &UnderlyingStorage) -> Self {
        match storage {
            UnderlyingStorage::Wayland(buffer) => Self::Wayland(buffer.downgrade()),
        }
    }
}

#[derive(Debug)]
struct ElementFramebufferCache<B>
where
    B: AsRef<framebuffer::Handle>,
{
    /// Cache for framebuffer handles per cache key (e.g. wayland buffer)
    fb_cache: HashMap<ElementFramebufferCacheKey, Result<OwnedFramebuffer<B>, ExportBufferError>>,
}

impl<B> ElementFramebufferCache<B>
where
    B: AsRef<framebuffer::Handle>,
{
    fn get(&self, buffer: &UnderlyingStorage) -> Option<Result<OwnedFramebuffer<B>, ExportBufferError>> {
        self.fb_cache
            .get(&ElementFramebufferCacheKey::from(buffer))
            .cloned()
    }

    fn insert(&mut self, buffer: &UnderlyingStorage, fb: Result<OwnedFramebuffer<B>, ExportBufferError>) {
        self.fb_cache.insert(ElementFramebufferCacheKey::from(buffer), fb);
    }

    fn cleanup(&mut self) {
        self.fb_cache.retain(|key, _| key.is_alive());
    }
}

impl<B> Clone for ElementFramebufferCache<B>
where
    B: AsRef<framebuffer::Handle>,
{
    fn clone(&self) -> Self {
        Self {
            fb_cache: self.fb_cache.clone(),
        }
    }
}

impl<B> Default for ElementFramebufferCache<B>
where
    B: AsRef<framebuffer::Handle>,
{
    fn default() -> Self {
        Self {
            fb_cache: Default::default(),
        }
    }
}

#[derive(Debug)]
struct PlaneConfig<B> {
    pub src: Rectangle<f64, BufferCoords>,
    pub dst: Rectangle<i32, Physical>,
    pub transform: Transform,
    pub damage_clips: Option<PlaneDamageClips>,
    pub buffer: Owned<B>,
    pub plane_claim: PlaneClaim,
}

impl<B> Clone for PlaneConfig<B> {
    fn clone(&self) -> Self {
        Self {
            src: self.src,
            dst: self.dst,
            transform: self.transform,
            damage_clips: self.damage_clips.clone(),
            buffer: self.buffer.clone(),
            plane_claim: self.plane_claim.clone(),
        }
    }
}

#[derive(Debug)]
struct PlaneState<B> {
    skip: bool,
    element_state: Option<(Id, CommitCounter)>,
    config: Option<PlaneConfig<B>>,
}

impl<B> Default for PlaneState<B> {
    fn default() -> Self {
        Self {
            skip: true,
            element_state: Default::default(),
            config: Default::default(),
        }
    }
}

impl<B> PlaneState<B> {
    fn buffer(&self) -> Option<&B> {
        self.config.as_ref().map(|config| &*config.buffer)
    }
}

impl<B> Clone for PlaneState<B> {
    fn clone(&self) -> Self {
        Self {
            skip: self.skip,
            element_state: self.element_state.clone(),
            config: self.config.clone(),
        }
    }
}

#[derive(Debug)]
struct FrameState<B: AsRef<framebuffer::Handle>> {
    planes: HashMap<plane::Handle, PlaneState<B>>,
}

impl<B: AsRef<framebuffer::Handle>> FrameState<B> {
    fn is_assigned(&self, handle: plane::Handle) -> bool {
        self.planes
            .get(&handle)
            .map(|config| config.config.is_some())
            .unwrap_or(false)
    }

    fn overlaps(&self, handle: plane::Handle, element_geometry: Rectangle<i32, Physical>) -> bool {
        self.planes
            .get(&handle)
            .and_then(|state| {
                state
                    .config
                    .as_ref()
                    .map(|config| config.dst.overlaps(element_geometry))
            })
            .unwrap_or(false)
    }

    fn plane_state(&self, handle: plane::Handle) -> Option<&PlaneState<B>> {
        self.planes.get(&handle)
    }

    fn plane_state_mut(&mut self, handle: plane::Handle) -> Option<&mut PlaneState<B>> {
        self.planes.get_mut(&handle)
    }

    fn plane_buffer(&self, handle: plane::Handle) -> Option<&B> {
        self.plane_state(handle)
            .and_then(|state| state.config.as_ref().map(|config| &*config.buffer))
    }

    fn is_empty(&self) -> bool {
        if self.planes.is_empty() {
            return true;
        }
        self.planes.iter().all(|p| p.1.skip)
    }
}

#[derive(Debug)]
struct Owned<B>(Rc<B>);

impl<B> std::ops::Deref for Owned<B> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<B> From<B> for Owned<B> {
    fn from(outer: B) -> Self {
        Self(Rc::new(outer))
    }
}

impl<B> AsRef<framebuffer::Handle> for Owned<B>
where
    B: AsRef<framebuffer::Handle>,
{
    fn as_ref(&self) -> &framebuffer::Handle {
        (*self.0).as_ref()
    }
}

impl<B> Clone for Owned<B> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<B: AsRef<framebuffer::Handle>> FrameState<B> {
    fn from_planes(planes: &Planes) -> Self {
        let cursor_plane_count = usize::from(planes.cursor.is_some());
        let mut tmp = HashMap::with_capacity(planes.overlay.len() + cursor_plane_count + 1);
        tmp.insert(planes.primary.handle, PlaneState::default());
        if let Some(info) = planes.cursor.as_ref() {
            tmp.insert(info.handle, PlaneState::default());
        }
        tmp.extend(
            planes
                .overlay
                .iter()
                .map(|info| (info.handle, PlaneState::default())),
        );

        FrameState { planes: tmp }
    }
}

impl<B: AsRef<framebuffer::Handle>> FrameState<B> {
    fn test_state(
        &mut self,
        surface: &DrmSurface,
        plane: plane::Handle,
        state: PlaneState<B>,
        allow_modeset: bool,
    ) -> Result<(), DrmError> {
        let current_config = match self.planes.get_mut(&plane) {
            Some(config) => config,
            None => return Ok(()),
        };
        let backup = current_config.clone();
        *current_config = state;

        let res = surface.test_state(
            self.planes
                .iter()
                // Filter out any skipped planes
                .filter(|(_, state)| !state.skip)
                .map(|(handle, state)| super::surface::PlaneState {
                    handle: *handle,
                    config: state.config.as_ref().map(|config| super::PlaneConfig {
                        src: config.src,
                        dst: config.dst,
                        transform: config.transform,
                        damage_clips: config.damage_clips.as_ref().map(|d| d.blob()),
                        fb: *config.buffer.as_ref(),
                    }),
                }),
            allow_modeset,
        );

        if res.is_err() {
            // test failed, restore previous state
            self.planes.insert(plane, backup);
        }

        res
    }

    fn commit(&self, surface: &DrmSurface, event: bool) -> Result<(), crate::backend::drm::error::Error> {
        surface.commit(
            self.planes
                .iter()
                // Filter out any skipped planes
                .filter(|(_, state)| !state.skip)
                .map(|(handle, state)| super::surface::PlaneState {
                    handle: *handle,
                    config: state.config.as_ref().map(|config| super::PlaneConfig {
                        src: config.src,
                        dst: config.dst,
                        transform: config.transform,
                        damage_clips: config.damage_clips.as_ref().map(|d| d.blob()),
                        fb: *config.buffer.as_ref(),
                    }),
                }),
            event,
        )
    }

    fn page_flip(&self, surface: &DrmSurface, event: bool) -> Result<(), crate::backend::drm::error::Error> {
        surface.page_flip(
            self.planes
                .iter()
                // Filter out any skipped planes
                .filter(|(_, state)| !state.skip)
                .map(|(handle, state)| super::surface::PlaneState {
                    handle: *handle,
                    config: state.config.as_ref().map(|config| super::PlaneConfig {
                        src: config.src,
                        dst: config.dst,
                        transform: config.transform,
                        damage_clips: config.damage_clips.as_ref().map(|d| d.blob()),
                        fb: *config.buffer.as_ref(),
                    }),
                }),
            event,
        )
    }
}

/// Possible buffers to export as a framebuffer using [`ExportFramebuffer`]
#[derive(Debug)]
pub enum ExportBuffer<'a, B: Buffer> {
    /// A wayland buffer
    Wayland(&'a WlBuffer),
    /// A [`Allocator`] buffer
    Allocator(&'a B),
}

impl<'a, B: Buffer> From<&'a UnderlyingStorage> for ExportBuffer<'a, B> {
    fn from(storage: &'a UnderlyingStorage) -> Self {
        match storage {
            UnderlyingStorage::Wayland(buffer) => Self::Wayland(buffer),
        }
    }
}

/// Export a [`ExportBuffer`] as a framebuffer
pub trait ExportFramebuffer<B: Buffer>
where
    B: Buffer,
{
    /// Type of the framebuffer
    type Framebuffer: AsRef<framebuffer::Handle>;

    /// Type of the error
    type Error: std::error::Error;

    /// Add a framebuffer for the specified buffer
    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, B>,
        allow_opaque_fallback: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error>;
}

impl<F, B> ExportFramebuffer<B> for Arc<Mutex<F>>
where
    F: ExportFramebuffer<B>,
    B: Buffer,
{
    type Framebuffer = <F as ExportFramebuffer<B>>::Framebuffer;
    type Error = <F as ExportFramebuffer<B>>::Error;

    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, B>,
        allow_opaque_fallback: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        let guard = self.lock().unwrap();
        guard.add_framebuffer(drm, buffer, allow_opaque_fallback)
    }
}

impl<F, B> ExportFramebuffer<B> for Rc<RefCell<F>>
where
    F: ExportFramebuffer<B>,
    B: Buffer,
{
    type Framebuffer = <F as ExportFramebuffer<B>>::Framebuffer;
    type Error = <F as ExportFramebuffer<B>>::Error;

    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, B>,
        allow_opaque_fallback: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        self.borrow().add_framebuffer(drm, buffer, allow_opaque_fallback)
    }
}

type Frame<A, F> = FrameState<
    DrmScanoutBuffer<
        <A as Allocator>::Buffer,
        <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer,
    >,
>;

type FrameErrorType<A, F> = FrameError<
    <A as Allocator>::Error,
    <<A as Allocator>::Buffer as AsDmabuf>::Error,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
>;

type FrameResult<T, A, F> = Result<T, FrameErrorType<A, F>>;

type RenderFrameErrorType<A, F, R> = RenderFrameError<
    <A as Allocator>::Error,
    <<A as Allocator>::Buffer as AsDmabuf>::Error,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
    R,
>;

#[derive(Debug)]
/// Defines the element for the primary plane in cases where a composited buffer was used.
pub struct PrimarySwapchainElement<B: Buffer, F: AsRef<framebuffer::Handle>> {
    /// The slot from the swapchain
    slot: Owned<DrmScanoutBuffer<B, F>>,
    /// The transform applied during rendering
    pub transform: Transform,
    /// The damage on the primary plane
    pub damage: DamageSnapshot<i32, BufferCoords>,
}

impl<B: Buffer, F: AsRef<framebuffer::Handle>> PrimarySwapchainElement<B, F> {
    /// Access the underlying swapchain buffer
    pub fn buffer(&self) -> &B {
        match &self.slot.0.buffer {
            ScanoutBuffer::Swapchain(slot) => slot,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug)]
/// Defines the element for the primary plane
pub enum PrimaryPlaneElement<'a, B: Buffer, F: AsRef<framebuffer::Handle>, E> {
    /// A slot from the swapchain was used for rendering
    /// the primary plane
    Swapchain(PrimarySwapchainElement<B, F>),
    /// An element has been assigned for direct scan-out
    Element(&'a E),
}

/// Result for [`DrmCompositor::render_frame`]
///
/// **Note**: This struct may contain a reference to the composited buffer
/// of the primary display plane. Dropping it will remove said reference and
/// allows the buffer to be reused.
///
/// Keeping the buffer longer may cause the following issues:
/// - **Too much damage** - until the buffer is marked free it is not considered
/// submitted by the swapchain, causing the age value of newly queried buffers
/// to be lower than necessary, potentially resulting in more rendering than necessary.
/// To avoid this make sure the buffer is dropped before starting the next render.
/// - **Exhaustion of swapchain images** - Continuing rendering while holding on
/// to too many buffers may cause the swapchain to run out of images, returning errors
/// on rendering until buffers are freed again. The exact amount of images in a
/// swapchain is an implementation detail, but should generally be expect to be
/// large enough to hold onto at least one `RenderFrameResult`.
pub struct RenderFrameResult<'a, B: Buffer, F: AsRef<framebuffer::Handle>, C: CMS, E> {
    /// Damage of this frame
    pub damage: Option<Vec<Rectangle<i32, Physical>>>,
    /// The render element states of this frame
    pub states: RenderElementStates,
    /// Element for the primary plane
    pub primary_element: PrimaryPlaneElement<'a, B, F, E>,
    /// Overlay elements in front to back order
    pub overlay_elements: Vec<&'a E>,
    /// Optional cursor plane element
    ///
    /// If set always above all other elements
    pub cursor_element: Option<&'a E>,

    pub output_profile: C::ColorProfile,

    primary_plane_element_id: Id,
}

struct SwapchainElement<'a, 'b, B: Buffer> {
    id: Id,
    slot: &'a Slot<B>,
    transform: Transform,
    damage: &'b DamageSnapshot<i32, BufferCoords>,
}

impl<'a, 'b, B: Buffer> Element for SwapchainElement<'a, 'b, B> {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        Rectangle::from_loc_and_size((0, 0), self.slot.size()).to_f64()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(
            (0, 0),
            self.slot.size().to_logical(1, self.transform).to_physical(1),
        )
    }

    fn transform(&self) -> Transform {
        self.transform
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        self.damage
            .damage_since(commit)
            .map(|d| {
                d.into_iter()
                    .map(|d| d.to_logical(1, self.transform, &self.slot.size()).to_physical(1))
                    .collect()
            })
            .unwrap_or_else(|| vec![self.geometry(scale)])
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        vec![self.geometry(scale)]
    }
}

enum FrameResultDamageElement<'a, 'b, E, B: Buffer> {
    Element(&'a E),
    Swapchain(SwapchainElement<'a, 'b, B>),
}

impl<'a, 'b, E, B> Element for FrameResultDamageElement<'a, 'b, E, B>
where
    E: Element,
    B: Buffer,
{
    fn id(&self) -> &Id {
        match self {
            FrameResultDamageElement::Element(e) => e.id(),
            FrameResultDamageElement::Swapchain(e) => e.id(),
        }
    }

    fn current_commit(&self) -> CommitCounter {
        match self {
            FrameResultDamageElement::Element(e) => e.current_commit(),
            FrameResultDamageElement::Swapchain(e) => e.current_commit(),
        }
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        match self {
            FrameResultDamageElement::Element(e) => e.src(),
            FrameResultDamageElement::Swapchain(e) => e.src(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            FrameResultDamageElement::Element(e) => e.geometry(scale),
            FrameResultDamageElement::Swapchain(e) => e.geometry(scale),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self {
            FrameResultDamageElement::Element(e) => e.location(scale),
            FrameResultDamageElement::Swapchain(e) => e.location(scale),
        }
    }

    fn transform(&self) -> Transform {
        match self {
            FrameResultDamageElement::Element(e) => e.transform(),
            FrameResultDamageElement::Swapchain(e) => e.transform(),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        match self {
            FrameResultDamageElement::Element(e) => e.damage_since(scale, commit),
            FrameResultDamageElement::Swapchain(e) => e.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        match self {
            FrameResultDamageElement::Element(e) => e.opaque_regions(scale),
            FrameResultDamageElement::Swapchain(e) => e.opaque_regions(scale),
        }
    }
}

/// Error for [`RenderFrameResult::blit_frame_result`]
#[derive(Debug, thiserror::Error)]
pub enum BlitFrameResultError<R: std::error::Error, E: std::error::Error> {
    /// A render error occurred
    #[error(transparent)]
    Rendering(R),
    /// A error occurred during exporting the buffer
    #[error(transparent)]
    Export(E),
}

impl<'a, B, F, C, E> RenderFrameResult<'a, B, F, C, E>
where
    B: Buffer,
    C: CMS,
    F: AsRef<framebuffer::Handle>,
{
    /// Get the damage of this frame for the specified dtr and age
    pub fn damage_from_age(
        &self,
        damage_tracker: &mut OutputDamageTracker,
        age: usize,
        filter: impl IntoIterator<Item = Id>,
    ) -> Result<(Option<Vec<Rectangle<i32, Physical>>>, RenderElementStates), OutputNoMode>
    where
        E: Element,
    {
        let filter_ids: HashSet<Id> = filter.into_iter().collect();

        let mut elements: Vec<FrameResultDamageElement<'_, '_, E, B>> =
            Vec::with_capacity(usize::from(self.cursor_element.is_some()) + self.overlay_elements.len() + 1);
        if let Some(cursor) = self.cursor_element {
            if !filter_ids.contains(cursor.id()) {
                elements.push(FrameResultDamageElement::Element(cursor));
            }
        }

        elements.extend(
            self.overlay_elements
                .iter()
                .filter(|e| !filter_ids.contains(e.id()))
                .map(|e| FrameResultDamageElement::Element(*e)),
        );

        let primary_render_element = match &self.primary_element {
            PrimaryPlaneElement::Swapchain(PrimarySwapchainElement {
                slot,
                transform,
                damage,
            }) => FrameResultDamageElement::Swapchain(SwapchainElement {
                id: self.primary_plane_element_id.clone(),
                transform: *transform,
                slot: match &slot.0.buffer {
                    ScanoutBuffer::Swapchain(slot) => slot,
                    _ => unreachable!(),
                },
                damage,
            }),
            PrimaryPlaneElement::Element(e) => FrameResultDamageElement::Element(*e),
        };

        elements.push(primary_render_element);

        damage_tracker.damage_output(age, &elements)
    }
}

impl<'a, B, F, C, E> RenderFrameResult<'a, B, F, C, E>
where
    B: Buffer + AsDmabuf,
    <B as AsDmabuf>::Error: std::fmt::Debug,
    C: CMS + 'static,
    F: AsRef<framebuffer::Handle>,
{
    /// Blit the frame result into a currently bound buffer
    #[allow(clippy::too_many_arguments)]
    pub fn blit_frame_result<R>(
        &self,
        cms: &mut C,
        size: impl Into<Size<i32, Physical>>,
        transform: Transform,
        scale: impl Into<Scale<f64>>,
        renderer: &mut R,
        damage: impl IntoIterator<Item = Rectangle<i32, Physical>>,
        filter: impl IntoIterator<Item = Id>,
    ) -> Result<(), BlitFrameResultError<<R as Renderer>::Error, <B as AsDmabuf>::Error>>
    where
        R: Renderer + Blit<Dmabuf>,
        <R as Renderer>::TextureId: 'static,
        E: Element + RenderElement<R, C>,
    {
        let size = size.into();
        let scale = scale.into();
        let filter_ids: HashSet<Id> = filter.into_iter().collect();
        let damage = damage.into_iter().collect::<Vec<_>>();

        // If we have no damage we can exit early
        if damage.is_empty() {
            return Ok(());
        }

        let mut opaque_regions: Vec<Rectangle<i32, Physical>> = Vec::new();

        let mut elements_to_render: Vec<&'a E> =
            Vec::with_capacity(usize::from(self.cursor_element.is_some()) + self.overlay_elements.len() + 1);

        if let Some(cursor_element) = self.cursor_element.as_ref() {
            if !filter_ids.contains(cursor_element.id()) {
                elements_to_render.push(*cursor_element);
                opaque_regions.extend(cursor_element.opaque_regions(scale));
            }
        }

        for element in self
            .overlay_elements
            .iter()
            .filter(|e| !filter_ids.contains(e.id()))
        {
            elements_to_render.push(element);
            opaque_regions.extend(element.opaque_regions(scale));
        }

        let primary_dmabuf = match &self.primary_element {
            PrimaryPlaneElement::Swapchain(PrimarySwapchainElement { slot, .. }) => {
                let dmabuf = match &slot.0.buffer {
                    ScanoutBuffer::Swapchain(slot) => slot.export().map_err(BlitFrameResultError::Export)?,
                    _ => unreachable!(),
                };
                let size = dmabuf.size();
                let geometry = Rectangle::from_loc_and_size(
                    (0, 0),
                    size.to_logical(1, Transform::Normal).to_physical(1),
                );
                opaque_regions.push(geometry);
                Some((dmabuf, geometry))
            }
            PrimaryPlaneElement::Element(e) => {
                elements_to_render.push(*e);
                opaque_regions.extend(e.opaque_regions(scale));
                None
            }
        };

        let clear_damage = opaque_regions.iter().fold(damage.clone(), |damage, region| {
            damage
                .into_iter()
                .flat_map(|geo| geo.subtract_rect(*region))
                .collect::<Vec<_>>()
        });

        if !clear_damage.is_empty() {
            trace!("clearing frame damage {:#?}", clear_damage);

            let mut frame = renderer
                .render(size, transform, cms, &self.output_profile)
                .map_err(BlitFrameResultError::Rendering)?;

            frame
                .clear([0f32, 0f32, 0f32, 1f32], &clear_damage, &self.output_profile) // Full alpha, profile doesn't matter
                .map_err(BlitFrameResultError::Rendering)?;

            frame.finish().map_err(BlitFrameResultError::Rendering)?;
        }

        // first do the potential blit
        if let Some((dmabuf, geometry)) = primary_dmabuf {
            let blit_damage = damage
                .iter()
                .filter_map(|d| d.intersection(geometry))
                .collect::<Vec<_>>();

            trace!("blitting frame with damage: {:#?}", blit_damage);

            for rect in blit_damage {
                renderer
                    .blit_from(
                        dmabuf.clone(),
                        rect,
                        rect,
                        crate::backend::renderer::TextureFilter::Linear,
                    )
                    .map_err(BlitFrameResultError::Rendering)?;
            }
        }

        // then render the remaining elements if any
        if !elements_to_render.is_empty() {
            trace!("drawing {} frame element(s)", elements_to_render.len());

            let mut frame = renderer
                .render(size, transform, cms, &self.output_profile)
                .map_err(BlitFrameResultError::Rendering)?;

            for element in elements_to_render.iter().rev() {
                let src = element.src();
                let dst = element.geometry(scale);
                let element_damage = damage
                    .iter()
                    .filter_map(|d| {
                        d.intersection(dst).map(|mut d| {
                            d.loc -= dst.loc;
                            d
                        })
                    })
                    .collect::<Vec<_>>();

                // no need to render without damage
                if element_damage.is_empty() {
                    continue;
                }

                trace!("drawing frame element with damage: {:#?}", element_damage);

                element
                    .draw(&mut frame, src, dst, &element_damage)
                    .map_err(BlitFrameResultError::Rendering)?;
            }

            frame.finish().map_err(BlitFrameResultError::Rendering)
        } else {
            Ok(())
        }
    }
}

impl<
        'a,
        B: Buffer + std::fmt::Debug,
        C: CMS,
        F: AsRef<framebuffer::Handle> + std::fmt::Debug,
        E: std::fmt::Debug,
    > std::fmt::Debug for RenderFrameResult<'a, B, F, C, E>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderFrameResult")
            .field("damage", &self.damage)
            .field("states", &self.states)
            .field("primary_element", &self.primary_element)
            .field("overlay_elements", &self.overlay_elements)
            .field("cursor_element", &self.cursor_element)
            .finish()
    }
}

#[derive(Debug)]
struct CursorState<G: AsFd + 'static> {
    allocator: GbmAllocator<G>,
    framebuffer_exporter: GbmDevice<G>,
    previous_output_transform: Option<Transform>,
    previous_output_scale: Option<Scale<f64>>,
}

#[derive(Debug, thiserror::Error, Copy, Clone)]
enum ExportBufferError {
    #[error("the buffer has no underlying storage")]
    NoUnderlyingStorage,
    #[error("exporting the framebuffer failed")]
    ExportFailed,
    #[error("no framebuffer could be exported")]
    Unsupported,
}

impl From<ExportBufferError> for Option<RenderingReason> {
    fn from(err: ExportBufferError) -> Self {
        if matches!(err, ExportBufferError::ExportFailed) {
            // Export failed could mean the buffer could
            // not be used to add a drm framebuffer. This
            // especially can happen on kmsro devices where
            // a buffer format not usable for scan-out can
            // not be used to add a framebuffer
            // We can try to give the client another chance
            // by announcing a scan-out tranche
            Some(RenderingReason::ScanoutFailed)
        } else {
            // We provide no reason for rendering here as there
            // is no action that can be taken to make it work
            None
        }
    }
}

#[derive(Debug)]
struct OverlayPlaneElementIds {
    plane_plane_ids: IndexMap<plane::Handle, Id>,
    plane_id_element_ids: IndexMap<Id, Id>,
}

impl OverlayPlaneElementIds {
    fn from_planes(planes: &Planes) -> Self {
        let overlay_plane_count = planes.overlay.len();

        Self {
            plane_plane_ids: IndexMap::with_capacity(overlay_plane_count),
            plane_id_element_ids: IndexMap::with_capacity(overlay_plane_count),
        }
    }

    fn plane_id_for_element_id(&mut self, plane: &plane::Handle, element_id: &Id) -> Id {
        // Either get the existing plane id for the plane when the stored element id
        // matches or generate a new Id (and update the element id)
        self.plane_plane_ids
            .entry(*plane)
            .and_modify(|plane_id| {
                let current_element_id = self.plane_id_element_ids.get(plane_id).unwrap();

                if current_element_id != element_id {
                    *plane_id = Id::new();
                    self.plane_id_element_ids
                        .insert(plane_id.clone(), element_id.clone());
                }
            })
            .or_insert_with(|| {
                let plane_id = Id::new();
                self.plane_id_element_ids
                    .insert(plane_id.clone(), element_id.clone());
                plane_id
            })
            .clone()
    }

    fn contains_plane_id(&self, plane_id: &Id) -> bool {
        self.plane_id_element_ids.contains_key(plane_id)
    }

    fn remove_plane(&mut self, plane: &plane::Handle) {
        if let Some(plane_id) = self.plane_plane_ids.remove(plane) {
            self.plane_id_element_ids.remove(&plane_id);
        }
    }
}

/// Composite an output using a combination of planes and rendering
///
/// see the [`module docs`](crate::backend::drm::compositor) for more information
#[derive(Debug)]
pub struct DrmCompositor<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<A::Buffer>,
    <F as ExportFramebuffer<A::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    output: Output,
    surface: Arc<DrmSurface>,
    planes: Planes,
    overlay_plane_element_ids: OverlayPlaneElementIds,
    damage_tracker: OutputDamageTracker,
    primary_plane_element_id: Id,
    primary_plane_damage_bag: DamageBag<i32, BufferCoords>,

    framebuffer_exporter: F,

    current_frame: Frame<A, F>,
    pending_frame: Option<(Frame<A, F>, U)>,
    queued_frame: Option<(Frame<A, F>, U)>,
    next_frame: Option<Frame<A, F>>,

    swapchain: Swapchain<A>,

    cursor_size: Size<i32, Physical>,
    cursor_state: Option<CursorState<G>>,

    element_states: IndexMap<
        Id,
        ElementFramebufferCache<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>,
    >,

    debug_flags: DebugFlags,
    span: tracing::Span,
}

impl<A, F, U, G> DrmCompositor<A, F, U, G>
where
    A: Allocator,
    <A as Allocator>::Error: std::error::Error + Send + Sync,
    <A as Allocator>::Buffer: AsDmabuf,
    <A::Buffer as AsDmabuf>::Error: std::error::Error + Send + Sync + std::fmt::Debug,
    F: ExportFramebuffer<A::Buffer>,
    <F as ExportFramebuffer<A::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    <F as ExportFramebuffer<A::Buffer>>::Error: std::error::Error + Send + Sync,
    G: AsFd + Clone,
{
    /// Initialize a new [`DrmCompositor`]
    ///
    /// - `output` is used to determine the current mode, scale and transform
    /// - `surface` for the compositor to use
    /// - `planes` defines which planes the compositor is allowed to use for direct scan-out.
    ///           `None` will result in the compositor to use all planes as specified by [`DrmSurface::planes`]
    /// - `allocator` used for the primary plane swapchain
    /// - `color_formats` are tested in order until a working configuration is found
    /// - `renderer_formats` as reported by the used renderer, used to build the intersection between
    ///                      the possible scan-out formats of the primary plane and the renderer
    /// - `framebuffer_exporter` is used to create drm framebuffers for the swapchain buffers (and if possible
    ///                          for element buffers) for scan-out
    /// - `cursor_size` as reported by the drm device, used for creating buffer for the cursor plane
    /// - `gbm` device used for creating buffers for the cursor plane, `None` will disable the cursor plane
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(allocator, framebuffer_exporter))]
    pub fn new(
        output: &Output,
        surface: DrmSurface,
        planes: Option<Planes>,
        mut allocator: A,
        framebuffer_exporter: F,
        color_formats: &[DrmFourcc],
        renderer_formats: HashSet<DrmFormat>,
        cursor_size: Size<u32, BufferCoords>,
        gbm: Option<GbmDevice<G>>,
    ) -> FrameResult<Self, A, F> {
        let span = info_span!(
            parent: None,
            "drm_compositor",
            output = output.name(),
            device = ?surface.dev_path(),
            crtc = ?surface.crtc(),
        );

        let mut error = None;
        let surface = Arc::new(surface);
        let mut planes = match planes {
            Some(planes) => planes,
            None => surface.planes()?,
        };

        // The selection algorithm expects the planes to be ordered form front to back
        planes
            .overlay
            .sort_by_key(|p| std::cmp::Reverse(p.zpos.unwrap_or_default()));

        let cursor_size = Size::from((cursor_size.w as i32, cursor_size.h as i32));
        let damage_tracker = OutputDamageTracker::from_output(output);

        for format in color_formats {
            debug!("Testing color format: {}", format);
            match Self::find_supported_format(
                surface.clone(),
                &planes,
                allocator,
                &framebuffer_exporter,
                renderer_formats.clone(),
                *format,
            ) {
                Ok((swapchain, current_frame)) => {
                    let cursor_state = gbm.map(|gbm| {
                        let cursor_allocator = GbmAllocator::new(
                            gbm.clone(),
                            BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
                        );
                        CursorState {
                            allocator: cursor_allocator,
                            framebuffer_exporter: gbm,
                            previous_output_scale: None,
                            previous_output_transform: None,
                        }
                    });

                    let overlay_plane_element_ids = OverlayPlaneElementIds::from_planes(&planes);

                    let drm_renderer = DrmCompositor {
                        primary_plane_element_id: Id::new(),
                        primary_plane_damage_bag: DamageBag::new(4),
                        current_frame,
                        pending_frame: None,
                        queued_frame: None,
                        next_frame: None,
                        swapchain,
                        framebuffer_exporter,
                        cursor_size,
                        cursor_state,
                        surface,
                        damage_tracker,
                        output: output.clone(),
                        planes,
                        overlay_plane_element_ids,
                        element_states: IndexMap::new(),
                        debug_flags: DebugFlags::empty(),
                        span,
                    };

                    return Ok(drm_renderer);
                }
                Err((alloc, err)) => {
                    warn!("Preferred format {} not available: {:?}", format, err);
                    allocator = alloc;
                    error = Some(err);
                }
            }
        }
        Err(error.unwrap())
    }

    fn find_supported_format(
        drm: Arc<DrmSurface>,
        planes: &Planes,
        allocator: A,
        framebuffer_exporter: &F,
        mut renderer_formats: HashSet<DrmFormat>,
        code: DrmFourcc,
    ) -> Result<(Swapchain<A>, Frame<A, F>), (A, FrameErrorType<A, F>)> {
        // select a format
        let mut plane_formats = match drm.supported_formats(drm.plane()) {
            Ok(formats) => formats.iter().cloned().collect::<HashSet<_>>(),
            Err(err) => return Err((allocator, err.into())),
        };

        let opaque_code = get_opaque(code).unwrap_or(code);
        if !plane_formats
            .iter()
            .any(|fmt| fmt.code == code || fmt.code == opaque_code)
        {
            return Err((allocator, FrameError::NoSupportedPlaneFormat));
        }
        plane_formats.retain(|fmt| fmt.code == code || fmt.code == opaque_code);
        renderer_formats.retain(|fmt| fmt.code == code);

        trace!("Plane formats: {:?}", plane_formats);
        trace!("Renderer formats: {:?}", renderer_formats);

        let plane_modifiers = plane_formats
            .iter()
            .map(|fmt| fmt.modifier)
            .collect::<HashSet<_>>();
        let renderer_modifiers = renderer_formats
            .iter()
            .map(|fmt| fmt.modifier)
            .collect::<HashSet<_>>();
        debug!(
            "Remaining intersected modifiers: {:?}",
            plane_modifiers
                .intersection(&renderer_modifiers)
                .collect::<HashSet<_>>()
        );

        if plane_formats.is_empty() {
            return Err((allocator, FrameError::NoSupportedPlaneFormat));
        } else if renderer_formats.is_empty() {
            return Err((allocator, FrameError::NoSupportedRendererFormat));
        }

        let formats = {
            // Special case: if a format supports explicit LINEAR (but no implicit Modifiers)
            // and the other doesn't support any modifier, force Implicit.
            // This should at least result in a working pipeline possibly with a linear buffer,
            // but we cannot be sure.
            if (plane_formats.len() == 1
                && plane_formats.iter().next().unwrap().modifier == DrmModifier::Invalid
                && renderer_formats
                    .iter()
                    .all(|x| x.modifier != DrmModifier::Invalid)
                && renderer_formats.iter().any(|x| x.modifier == DrmModifier::Linear))
                || (renderer_formats.len() == 1
                    && renderer_formats.iter().next().unwrap().modifier == DrmModifier::Invalid
                    && plane_formats.iter().all(|x| x.modifier != DrmModifier::Invalid)
                    && plane_formats.iter().any(|x| x.modifier == DrmModifier::Linear))
            {
                vec![DrmFormat {
                    code,
                    modifier: DrmModifier::Invalid,
                }]
            } else {
                plane_modifiers
                    .intersection(&renderer_modifiers)
                    .cloned()
                    .map(|modifier| DrmFormat { code, modifier })
                    .collect::<Vec<_>>()
            }
        };
        debug!("Testing Formats: {:?}", formats);

        let modifiers = formats.iter().map(|x| x.modifier).collect::<Vec<_>>();
        let mode = drm.pending_mode();

        let mut swapchain: Swapchain<A> = Swapchain::new(
            allocator,
            mode.size().0 as u32,
            mode.size().1 as u32,
            code,
            modifiers,
        );

        // Test format
        let buffer = match swapchain.acquire() {
            Ok(buffer) => buffer.unwrap(),
            Err(err) => return Err((swapchain.allocator, FrameError::Allocator(err))),
        };

        let dmabuf = match buffer.export() {
            Ok(dmabuf) => dmabuf,
            Err(err) => {
                return Err((swapchain.allocator, FrameError::AsDmabufError(err)));
            }
        };

        let fb_buffer = match framebuffer_exporter.add_framebuffer(
            drm.device_fd(),
            ExportBuffer::Allocator(&buffer),
            true,
        ) {
            Ok(Some(fb_buffer)) => fb_buffer,
            Ok(None) => return Err((swapchain.allocator, FrameError::NoFramebuffer)),
            Err(err) => return Err((swapchain.allocator, FrameError::FramebufferExport(err))),
        };
        buffer
            .userdata()
            .insert_if_missing(|| OwnedFramebuffer::new(DrmFramebuffer::Exporter(fb_buffer)));

        let mode = drm.pending_mode();
        let handle = buffer
            .userdata()
            .get::<OwnedFramebuffer<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>>()
            .unwrap()
            .clone();

        let mode_size = Size::from((mode.size().0 as i32, mode.size().1 as i32));

        let mut current_frame_state = FrameState::from_planes(planes);
        let plane_claim = match drm.claim_plane(planes.primary.handle) {
            Some(claim) => claim,
            None => {
                warn!("failed to claim primary plane",);
                return Err((swapchain.allocator, FrameError::PrimaryPlaneClaimFailed));
            }
        };

        let plane_state = PlaneState {
            skip: false,
            element_state: None,
            config: Some(PlaneConfig {
                src: Rectangle::from_loc_and_size(Point::default(), dmabuf.size()).to_f64(),
                dst: Rectangle::from_loc_and_size(Point::default(), mode_size),
                transform: Transform::Normal,
                damage_clips: None,
                buffer: Owned::from(DrmScanoutBuffer {
                    buffer: ScanoutBuffer::Swapchain(buffer),
                    fb: handle,
                }),
                plane_claim,
            }),
        };

        match current_frame_state.test_state(&drm, planes.primary.handle, plane_state, true) {
            Ok(_) => {
                debug!("Chosen format: {:?}", dmabuf.format());
                Ok((swapchain, current_frame_state))
            }
            Err(err) => {
                warn!(
                    "Mode-setting failed with automatically selected buffer format {:?}: {}",
                    dmabuf.format(),
                    err
                );
                Err((swapchain.allocator, err.into()))
            }
        }
    }

    /// Render the next frame
    ///
    /// - `elements` for this frame in front-to-back order
    #[instrument(level = "trace", parent = &self.span, skip_all)]
    pub fn render_frame<'a, R, C, E, Target>(
        &'a mut self,
        renderer: &mut R,
        cms: &mut C,
        elements: &'a [E],
        clear_color: [f32; 4],
        clear_profile: &C::ColorProfile,
        output_profile: &C::ColorProfile,
    ) -> Result<RenderFrameResult<'a, A::Buffer, F::Framebuffer, C, E>, RenderFrameErrorType<A, F, R>>
    where
        E: RenderElement<R, C>,
        R: Renderer + Bind<Dmabuf> + Offscreen<Target> + ExportMem,
        <R as Renderer>::TextureId: Texture + 'static,
        C: CMS + 'static,
    {
        // Just reset any next state, this will put
        // any already acquired slot back to the swapchain
        self.next_frame.take();

        let current_size = self.output.current_mode().unwrap().size;
        let output_scale = self.output.current_scale().fractional_scale().into();
        let output_transform = self.output.current_transform();

        // Output transform is specified in surface-rotation, so inversion gives us the
        // render transform for the output itself.
        let output_transform = output_transform.invert();

        // Geometry of the output derived from the output mode including the transform
        // This is used to calculate the intersection between elements and the output.
        // The renderer (and also the logic for direct scan-out) will take care of the
        // actual transform during rendering
        let output_geometry: Rectangle<_, Physical> =
            Rectangle::from_loc_and_size((0, 0), output_transform.transform_size(current_size));

        // We always acquire a buffer from the swapchain even
        // if we could end up doing direct scan-out on the primary plane.
        // The reason is that we can't know upfront and we need a framebuffer
        // on the primary plane to test overlay/cursor planes
        let primary_plane_buffer = self
            .swapchain
            .acquire()
            .map_err(FrameError::Allocator)?
            .ok_or(FrameError::NoFreeSlotsError)?;

        // It is safe to call export multiple times as the Slot will cache the dmabuf for us
        let dmabuf = primary_plane_buffer.export().map_err(FrameError::AsDmabufError)?;

        // Let's check if we already have a cached framebuffer for this Slot, if not try to export
        // it and use the Slot userdata to cache it
        let maybe_buffer = primary_plane_buffer
            .userdata()
            .get::<OwnedFramebuffer<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>>();
        if maybe_buffer.is_none() {
            let fb_buffer = self
                .framebuffer_exporter
                .add_framebuffer(
                    self.surface.device_fd(),
                    ExportBuffer::Allocator(&primary_plane_buffer),
                    true,
                )
                .map_err(FrameError::FramebufferExport)?
                .ok_or(FrameError::NoFramebuffer)?;
            primary_plane_buffer
                .userdata()
                .insert_if_missing(|| OwnedFramebuffer::new(DrmFramebuffer::Exporter(fb_buffer)));
        }

        // This unwrap is safe as we error out above if we were unable to export a framebuffer
        let fb = primary_plane_buffer
            .userdata()
            .get::<OwnedFramebuffer<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>>()
            .unwrap()
            .clone();

        let mut output_damage: Vec<Rectangle<i32, Physical>> = Vec::new();
        let mut opaque_regions: Vec<Rectangle<i32, Physical>> = Vec::new();
        let mut element_states = IndexMap::new();
        let mut render_element_states = RenderElementStates {
            states: Default::default(),
        };

        // So first we want to create a clean state, for that we have to reset all overlay and cursor planes
        // to nothing. We only want to test if the primary plane alone can be used for scan-out.
        let mut next_frame_state = {
            let previous_state = self
                .pending_frame
                .as_ref()
                .map(|(state, _)| state)
                .unwrap_or(&self.current_frame);

            // This will create an empty frame state, all planes are skipped by default
            let mut next_frame_state = FrameState::from_planes(&self.planes);

            // We want to set skip to false on all planes that previously had something assigned so that
            // they get cleared when they are not longer used
            for (handle, plane_state) in next_frame_state.planes.iter_mut() {
                let reset_state = previous_state
                    .plane_state(*handle)
                    .map(|state| state.config.is_some())
                    .unwrap_or(false);

                if reset_state {
                    plane_state.skip = false;
                }
            }

            next_frame_state
        };

        // We want to make sure we can actually scan-out the primary plane, so
        // explicitly set skip to false
        let plane_claim = self
            .surface
            .claim_plane(self.planes.primary.handle)
            .ok_or_else(|| {
                error!("failed to claim primary plane");
                FrameError::PrimaryPlaneClaimFailed
            })?;
        let primary_plane_state = PlaneState {
            skip: false,
            element_state: None,
            config: Some(PlaneConfig {
                src: Rectangle::from_loc_and_size(Point::default(), dmabuf.size()).to_f64(),
                dst: Rectangle::from_loc_and_size(Point::default(), current_size),
                // NOTE: We do not apply the transform to the primary plane as this is handled by the dtr/renderer
                transform: Transform::Normal,
                damage_clips: None,
                buffer: Owned::from(DrmScanoutBuffer {
                    buffer: ScanoutBuffer::Swapchain(primary_plane_buffer),
                    fb,
                }),
                plane_claim,
            }),
        };

        // test that we can scan-out the primary plane
        //
        // Note: this should only fail if the device has been
        // deactivated or we lost access (like during a vt switch)
        next_frame_state
            .test_state(
                &self.surface,
                self.planes.primary.handle,
                primary_plane_state,
                self.surface.commit_pending(),
            )
            .map_err(FrameError::DrmError)?;

        // This holds all elements that are visible on the output
        // A element is considered visible if it intersects with the output geometry
        // AND is not completely hidden behind opaque regions
        let mut output_elements: Vec<(&'a E, usize)> = Vec::with_capacity(elements.len());

        for element in elements.iter() {
            let element_id = element.id();
            let element_geometry = element.geometry(output_scale);
            let element_loc = element_geometry.loc;

            // First test if the element overlaps with the output
            // if not we can skip it
            let element_output_geometry = match element_geometry.intersection(output_geometry) {
                Some(geo) => geo,
                None => continue,
            };

            // Then test if the element is completely hidden behind opaque regions
            let element_visible_area = opaque_regions
                .iter()
                .fold([element_output_geometry].to_vec(), |geometry, opaque_region| {
                    geometry
                        .into_iter()
                        .flat_map(|g| g.subtract_rect(*opaque_region))
                        .collect::<Vec<_>>()
                })
                .into_iter()
                .fold(0usize, |acc, item| acc + (item.size.w * item.size.h) as usize);

            if element_visible_area == 0 {
                // No need to draw a completely hidden element
                trace!("skipping completely obscured element {:?}", element.id());

                // We allow multiple instance of a single element, so do not
                // override the state if we already have one
                if !render_element_states.states.contains_key(element_id) {
                    render_element_states
                        .states
                        .insert(element_id.clone(), RenderElementState::skipped());
                }
                continue;
            }

            output_elements.push((element, element_visible_area));

            let element_opaque_regions = element
                .opaque_regions(output_scale)
                .into_iter()
                .map(|mut region| {
                    region.loc += element_loc;
                    region
                })
                .filter_map(|geo| geo.intersection(output_geometry))
                .collect::<Vec<_>>();

            opaque_regions.extend(element_opaque_regions);
        }

        let overlay_plane_lookup: HashMap<plane::Handle, PlaneInfo> =
            self.planes.overlay.iter().map(|p| (p.handle, *p)).collect();

        // This will hold the element that has been selected for direct scan-out on
        // the primary plane if any
        let mut primary_plane_scanout_element: Option<&'a E> = None;
        // This will hold all elements that have been assigned to the primary plane
        // for rendering
        let mut primary_plane_elements: Vec<&'a E> = Vec::with_capacity(elements.len());
        // This will hold the element per plane that has been assigned to a overlay/underlay
        // plane for direct scan-out
        let mut overlay_plane_elements: IndexMap<plane::Handle, &'a E> =
            IndexMap::with_capacity(self.planes.overlay.len());
        // This will hold the element assigned on the cursor plane if any
        let mut cursor_plane_element: Option<&'a E> = None;

        let output_elements_len = output_elements.len();
        for (index, (element, element_visible_area)) in output_elements.into_iter().enumerate() {
            let element_id = element.id();
            let element_geometry = element.geometry(output_scale);
            let remaining_elements = output_elements_len - index;

            // Check if we found our last item, we can try to do
            // direct scan-out on the primary plane
            // If we already assigned an element to
            // an underlay plane we will have a hole punch element
            // on the primary plane, this will disable direct scan-out
            // on the primary plane.
            let try_assign_primary_plane = if remaining_elements == 1 && primary_plane_elements.is_empty() {
                let element_is_opaque = element_is_opaque(element, output_scale);
                let crtc_background_matches_clear_color =
                    (clear_color[0] == 0f32 && clear_color[1] == 0f32 && clear_color[2] == 0f32)
                        || clear_color[3] == 0f32;
                let element_spans_complete_output = element_geometry.contains_rect(output_geometry);
                let overlaps_with_underlay = self
                    .planes
                    .overlay
                    .iter()
                    .filter(|p| p.zpos.unwrap_or_default() < self.planes.primary.zpos.unwrap_or_default())
                    .any(|p| next_frame_state.overlaps(p.handle, element_geometry));
                !overlaps_with_underlay
                    && (crtc_background_matches_clear_color
                        || (element_spans_complete_output && element_is_opaque))
            } else {
                false
            };

            match self.try_assign_element(
                renderer,
                cms,
                element,
                &mut element_states,
                &primary_plane_elements,
                output_scale,
                &mut next_frame_state,
                &mut output_damage,
                output_transform,
                output_geometry,
                output_profile,
                try_assign_primary_plane,
            ) {
                Ok(direct_scan_out_plane) => {
                    match direct_scan_out_plane.type_ {
                        drm::control::PlaneType::Overlay => {
                            overlay_plane_elements.insert(direct_scan_out_plane.handle, element);
                        }
                        drm::control::PlaneType::Primary => primary_plane_scanout_element = Some(element),
                        drm::control::PlaneType::Cursor => cursor_plane_element = Some(element),
                    }

                    if let Some(state) = render_element_states.states.get_mut(element_id) {
                        state.presentation_state = RenderElementPresentationState::ZeroCopy;
                        state.visible_area += element_visible_area;
                    } else {
                        render_element_states.states.insert(
                            element_id.clone(),
                            RenderElementState::zero_copy(element_visible_area),
                        );
                    }
                }
                Err(reason) => {
                    if let Some(reason) = reason {
                        if !render_element_states.states.contains_key(element_id) {
                            render_element_states.states.insert(
                                element_id.clone(),
                                RenderElementState::rendering_with_reason(reason),
                            );
                        }
                    }

                    primary_plane_elements.push(element);
                }
            }
        }

        // Cleanup old state (e.g. old dmabuffers)
        for element_state in element_states.values_mut() {
            element_state.cleanup();
        }
        self.element_states = element_states;

        let previous_state = self
            .pending_frame
            .as_ref()
            .map(|(state, _)| state)
            .unwrap_or(&self.current_frame);

        // If a plane has been moved or no longer has a buffer we need to report that as damage
        for (handle, previous_plane_state) in previous_state.planes.iter() {
            if let Some(previous_config) = previous_plane_state.config.as_ref() {
                let next_state = next_frame_state
                    .plane_state(*handle)
                    .as_ref()
                    .and_then(|state| state.config.as_ref());

                // plane has been removed, so remove the plane from the plane id cache
                if next_state.is_none() {
                    self.overlay_plane_element_ids.remove_plane(handle);
                }
                if next_state
                    .map(|next_config| next_config.dst != previous_config.dst)
                    .unwrap_or(true)
                {
                    output_damage.push(previous_config.dst);

                    if let Some(next_config) = next_state {
                        trace!(
                            "damaging move plane {:?}: {:?} -> {:?}",
                            handle,
                            previous_config.dst,
                            next_config.dst
                        );
                        output_damage.push(next_config.dst);
                    } else {
                        trace!("damaging removed plane {:?}: {:?}", handle, previous_config.dst);
                    }
                }
            }
        }

        let render = next_frame_state
            .plane_buffer(self.planes.primary.handle)
            .map(|config| matches!(&config.buffer, ScanoutBuffer::Swapchain(_)))
            .unwrap_or(false);

        if render {
            trace!(
                "rendering {} elements on the primary plane {:?}",
                primary_plane_elements.len(),
                self.planes.primary.handle,
            );
            let (dmabuf, age) = {
                let primary_plane_state = next_frame_state.plane_state(self.planes.primary.handle).unwrap();
                let config = primary_plane_state.config.as_ref().unwrap();
                let slot = match &config.buffer.buffer {
                    ScanoutBuffer::Swapchain(slot) => slot,
                    _ => unreachable!(),
                };

                // It is safe to call export multiple times as the Slot will cache the dmabuf for us
                let dmabuf = slot.export().map_err(FrameError::AsDmabufError)?;
                let age = slot.age().into();
                (dmabuf, age)
            };

            renderer
                .bind(dmabuf)
                .map_err(OutputDamageTrackerError::Rendering)?;

            // store the current renderer debug flags and replace them
            // with our own
            let renderer_debug_flags = renderer.debug_flags();
            renderer.set_debug_flags(self.debug_flags);

            // First we collect all our fake elements for overlay and underlays
            // This is used to transport the opaque regions for elements that
            // have been assigned to planes and to realize hole punching for
            // underlays. We use an Id per plane/element combination to not
            // interfer with the element damage state in the output damage tracker.
            // Using the original element id would store the commit in the
            // OutputDamageTracker without actual rendering anything -> bad
            // Using a id per plane could result in an issue when a different
            // element with the same geometry gets assigned and has the same
            // commit -> unlikely but possible
            // So we use an Id per plane for as long as we have the same element
            // on that plane.
            let mut elements = overlay_plane_elements
                .iter()
                .map(|(p, element)| {
                    let id = self
                        .overlay_plane_element_ids
                        .plane_id_for_element_id(p, element.id());

                    let is_underlay = overlay_plane_lookup.get(p).unwrap().zpos.unwrap_or_default()
                        < self.planes.primary.zpos.unwrap_or_default();
                    if is_underlay {
                        HolepunchRenderElement::from_render_element(id, element, output_scale).into()
                    } else {
                        OverlayPlaneElement::from_render_element(id, *element).into()
                    }
                })
                .collect::<Vec<_>>();

            // Then render all remaining elements assigned to the primary plane
            elements.extend(
                primary_plane_elements
                    .iter()
                    .map(|e| DrmRenderElements::Other(*e)),
            );

            let render_res = self.damage_tracker.render_output(
                renderer,
                cms,
                age,
                &elements,
                clear_color,
                clear_profile,
                output_profile,
            );

            // restore the renderer debug flags
            renderer.set_debug_flags(renderer_debug_flags);

            match render_res {
                Ok((render_damage, states)) => {
                    for (id, state) in states.states.into_iter() {
                        // Skip the state for our fake elements
                        if self.overlay_plane_element_ids.contains_plane_id(&id) {
                            continue;
                        }

                        if let Some(existing_state) = render_element_states.states.get_mut(&id) {
                            if matches!(
                                existing_state.presentation_state,
                                RenderElementPresentationState::Skipped
                            ) {
                                *existing_state = state;
                            } else {
                                existing_state.visible_area += state.visible_area;
                            }
                        } else {
                            render_element_states.states.insert(id.clone(), state);
                        }
                    }

                    // Fixup damage on plane, if we used the plane for direct scan-out before
                    // but now use it for rendering we do not replace the damage which is
                    // the whole plane initially.
                    let had_direct_scan_out = previous_state
                        .plane_state(self.planes.primary.handle)
                        .map(|state| state.element_state.is_some())
                        .unwrap_or(true);

                    let primary_plane_state = next_frame_state
                        .plane_state_mut(self.planes.primary.handle)
                        .unwrap();
                    let config = primary_plane_state.config.as_mut().unwrap();

                    if !had_direct_scan_out {
                        if let Some(render_damage) = render_damage {
                            trace!("rendering damage: {:?}", render_damage);

                            self.primary_plane_damage_bag.add(render_damage.iter().map(|d| {
                                d.to_logical(1).to_buffer(
                                    1,
                                    Transform::Normal,
                                    &output_geometry.size.to_logical(1),
                                )
                            }));
                            output_damage.extend(render_damage.clone());
                            config.damage_clips = PlaneDamageClips::from_damage(
                                self.surface.device_fd(),
                                config.src,
                                config.dst,
                                render_damage,
                            )
                            .ok()
                            .flatten();
                        } else {
                            trace!("skipping primary plane, no damage");

                            primary_plane_state.skip = true;
                            *config = previous_state
                                .plane_state(self.planes.primary.handle)
                                .and_then(|state| state.config.as_ref().cloned())
                                .unwrap_or_else(|| config.clone());
                        }
                    } else {
                        trace!(
                            "clearing previous direct scan-out on primary plane, damaging complete output"
                        );
                        output_damage.push(output_geometry);
                        self.primary_plane_damage_bag
                            .add([output_geometry.to_logical(1).to_buffer(
                                1,
                                Transform::Normal,
                                &output_geometry.size.to_logical(1),
                            )]);
                    }
                }
                Err(err) => {
                    // Rendering failed at some point, reset the buffers
                    // as we probably now have some half drawn buffer
                    self.swapchain.reset_buffers();
                    return Err(RenderFrameError::from(err));
                }
            }
        }

        self.next_frame = Some(next_frame_state);

        let primary_plane_element = if render {
            let slot = {
                let primary_plane_state = self
                    .next_frame
                    .as_ref()
                    .unwrap()
                    .plane_state(self.planes.primary.handle)
                    .unwrap();
                let config = primary_plane_state.config.as_ref().unwrap();
                config.buffer.clone()
            };
            PrimaryPlaneElement::Swapchain(PrimarySwapchainElement {
                slot,
                transform: output_transform,
                damage: self.primary_plane_damage_bag.snapshot(),
            })
        } else {
            PrimaryPlaneElement::Element(primary_plane_scanout_element.unwrap())
        };

        let damage = if output_damage.is_empty() {
            None
        } else {
            Some(output_damage)
        };
        let frame_reference: RenderFrameResult<'a, A::Buffer, F::Framebuffer, C, E> = RenderFrameResult {
            primary_element: primary_plane_element,
            damage,
            overlay_elements: overlay_plane_elements.into_values().collect(),
            cursor_element: cursor_plane_element,
            states: render_element_states,
            output_profile: output_profile.clone(),
            primary_plane_element_id: self.primary_plane_element_id.clone(),
        };

        Ok(frame_reference)
    }

    /// Queues the current frame for scan-out.
    ///
    /// If `render_frame` has not been called prior to this function or returned no damage
    /// this function will return [`FrameError::EmptyFrame`]. Instead of calling `queue_frame` it
    /// is the callers responsibility to re-schedule the frame. A simple strategy for frame
    /// re-scheduling is to queue a one-shot timer that will trigger after approximately one
    /// retrace duration.
    ///
    /// *Note*: This function needs to be followed up with [`DrmCompositor::frame_submitted`]
    /// when a vblank event is received, that denotes successful scan-out of the frame.
    /// Otherwise the underlying swapchain will eventually run out of buffers.
    ///
    /// `user_data` can be used to attach some data to a specific buffer and later retrieved with [`DrmCompositor::frame_submitted`]
    pub fn queue_frame(&mut self, user_data: U) -> FrameResult<(), A, F> {
        let next_frame = self.next_frame.take().ok_or(FrameErrorType::<A, F>::EmptyFrame)?;

        if next_frame.is_empty() {
            return Err(FrameErrorType::<A, F>::EmptyFrame);
        }

        if let Some(plane_state) = next_frame.plane_state(self.planes.primary.handle) {
            if !plane_state.skip {
                let slot = plane_state.buffer().and_then(|config| match &config.buffer {
                    ScanoutBuffer::Swapchain(slot) => Some(slot),
                    _ => None,
                });

                if let Some(slot) = slot {
                    self.swapchain.submitted(slot);
                }
            }
        }

        self.queued_frame = Some((next_frame, user_data));
        if self.pending_frame.is_none() {
            self.submit()?;
        }
        Ok(())
    }

    fn submit(&mut self) -> FrameResult<(), A, F> {
        let (state, user_data) = self.queued_frame.take().unwrap();

        let flip = if self.surface.commit_pending() {
            state.commit(&self.surface, true)
        } else {
            state.page_flip(&self.surface, true)
        };
        if flip.is_ok() {
            self.pending_frame = Some((state, user_data));
        }

        flip.map_err(FrameError::DrmError)
    }

    /// Marks the current frame as submitted.
    ///
    /// *Note*: Needs to be called, after the vblank event of the matching [`DrmDevice`](super::super::DrmDevice)
    /// was received after calling [`DrmCompositor::queue_frame`] on this surface.
    /// Otherwise the underlying swapchain will run out of buffers eventually.
    pub fn frame_submitted(&mut self) -> FrameResult<Option<U>, A, F> {
        if let Some((mut pending, user_data)) = self.pending_frame.take() {
            std::mem::swap(&mut pending, &mut self.current_frame);
            if self.queued_frame.is_some() {
                self.submit()?;
            }
            Ok(Some(user_data))
        } else {
            Ok(None)
        }
    }

    /// Reset the underlying buffers
    pub fn reset_buffers(&mut self) {
        self.swapchain.reset_buffers();
    }

    /// Returns the underlying [`crtc`](drm::control::crtc) of this surface
    pub fn crtc(&self) -> crtc::Handle {
        self.surface.crtc()
    }

    /// Returns the underlying [`plane`](drm::control::plane) of this surface
    pub fn plane(&self) -> plane::Handle {
        self.surface.plane()
    }

    /// Currently used [`connector`](drm::control::connector)s of this `Surface`
    pub fn current_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.surface.current_connectors()
    }

    /// Returns the pending [`connector`](drm::control::connector)s
    /// used for the next frame queued via [`queue_frame`](DrmCompositor::queue_frame).
    pub fn pending_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.surface.pending_connectors()
    }

    /// Tries to add a new [`connector`](drm::control::connector)
    /// to be used after the next commit.
    ///
    /// **Warning**: You need to make sure, that the connector is not used with another surface
    /// or was properly removed via `remove_connector` + `commit` before adding it to another surface.
    /// Behavior if failing to do so is undefined, but might result in rendering errors or the connector
    /// getting removed from the other surface without updating it's internal state.
    ///
    /// Fails if the `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).
    pub fn add_connector(&self, connector: connector::Handle) -> FrameResult<(), A, F> {
        self.surface
            .add_connector(connector)
            .map_err(FrameError::DrmError)
    }

    /// Tries to mark a [`connector`](drm::control::connector)
    /// for removal on the next commit.    
    pub fn remove_connector(&self, connector: connector::Handle) -> FrameResult<(), A, F> {
        self.surface
            .remove_connector(connector)
            .map_err(FrameError::DrmError)
    }

    /// Tries to replace the current connector set with the newly provided one on the next commit.
    ///
    /// Fails if one new `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).    
    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> FrameResult<(), A, F> {
        self.surface
            .set_connectors(connectors)
            .map_err(FrameError::DrmError)
    }

    /// Returns the currently active [`Mode`](drm::control::Mode)
    /// of the underlying [`crtc`](drm::control::crtc)    
    pub fn current_mode(&self) -> Mode {
        self.surface.current_mode()
    }

    /// Returns the currently pending [`Mode`](drm::control::Mode)
    /// to be used after the next commit.    
    pub fn pending_mode(&self) -> Mode {
        self.surface.pending_mode()
    }

    /// Tries to set a new [`Mode`](drm::control::Mode)
    /// to be used after the next commit.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`](drm::control::crtc) or any of the
    /// pending [`connector`](drm::control::connector)s.
    pub fn use_mode(&mut self, mode: Mode) -> FrameResult<(), A, F> {
        self.surface.use_mode(mode).map_err(FrameError::DrmError)?;
        let (w, h) = mode.size();
        self.swapchain.resize(w as _, h as _);
        Ok(())
    }

    /// Set the [`DebugFlags`] to use
    ///
    /// Note: This will reset the primary plane swapchain if
    /// the flags differ from the current flags
    pub fn set_debug_flags(&mut self, flags: DebugFlags) {
        if self.debug_flags != flags {
            self.debug_flags = flags;
            self.swapchain.reset_buffers();
        }
    }

    /// Returns the current enabled [`DebugFlags`]
    pub fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }

    /// Returns a reference to the underlying drm surface
    pub fn surface(&self) -> &DrmSurface {
        &self.surface
    }

    /// Get the format of the underlying swapchain
    pub fn format(&self) -> DrmFourcc {
        self.swapchain.format()
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "trace", skip_all)]
    fn try_assign_element<'a, R, C, E, Target>(
        &mut self,
        renderer: &mut R,
        cms: &mut C,
        element: &'a E,
        element_states: &mut IndexMap<
            Id,
            ElementFramebufferCache<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>,
        >,
        primary_plane_elements: &[&'a E],
        scale: Scale<f64>,
        frame_state: &mut Frame<A, F>,
        output_damage: &mut Vec<Rectangle<i32, Physical>>,
        output_transform: Transform,
        output_geometry: Rectangle<i32, Physical>,
        output_profile: &C::ColorProfile,
        try_assign_primary_plane: bool,
    ) -> Result<PlaneInfo, Option<RenderingReason>>
    where
        R: Renderer + Bind<Dmabuf> + Offscreen<Target> + ExportMem,
        C: CMS + 'static,
        E: RenderElement<R, C>,
    {
        let mut rendering_reason: Option<RenderingReason> = None;

        if !cms
            .input_transformation(
                &element.color_profile(),
                output_profile,
                TransformType::InputToOutput,
            )
            .map_err(|_| None)?
            .is_identity()
        {
            rendering_reason = Some(RenderingReason::ColorTransformFailed);
        }

        if try_assign_primary_plane && !rendering_reason.is_some() {
            match self.try_assign_primary_plane(
                renderer,
                element,
                element_states,
                scale,
                frame_state,
                output_damage,
                output_transform,
                output_geometry,
            )? {
                Ok(plane) => {
                    trace!(
                        "assigned element {:?} to primary plane {:?}",
                        element.id(),
                        self.planes.primary.handle
                    );
                    return Ok(plane);
                }
                Err(err) => rendering_reason = rendering_reason.or(err),
            }
        }

        if let Some(plane) = self.try_assign_cursor_plane(
            renderer,
            cms,
            element,
            element_states,
            scale,
            frame_state,
            output_damage,
            output_transform,
            output_geometry,
            output_profile,
        ) {
            trace!(
                "assigned element {:?} to cursor plane {:?}",
                element.id(),
                self.planes.cursor.as_ref().map(|p| p.handle)
            );
            return Ok(plane);
        }

        if rendering_reason.is_some() {
            return Err(rendering_reason);
        }

        match self.try_assign_overlay_plane(
            renderer,
            element,
            element_states,
            primary_plane_elements,
            scale,
            frame_state,
            output_damage,
            output_transform,
            output_geometry,
        )? {
            Ok(plane) => {
                trace!("assigned element {:?} to overlay plane", element.id());
                return Ok(plane);
            }
            Err(err) => rendering_reason = rendering_reason.or(err),
        }

        Err(rendering_reason)
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "trace", skip_all)]
    fn try_assign_cursor_plane<R, C, E, Target>(
        &mut self,
        renderer: &mut R,
        cms: &mut C,
        element: &E,
        element_states: &mut IndexMap<
            Id,
            ElementFramebufferCache<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>,
        >,
        scale: Scale<f64>,
        frame_state: &mut Frame<A, F>,
        output_damage: &mut Vec<Rectangle<i32, Physical>>,
        output_transform: Transform,
        output_geometry: Rectangle<i32, Physical>,
        output_profile: &C::ColorProfile,
    ) -> Option<PlaneInfo>
    where
        R: Renderer + Offscreen<Target> + ExportMem,
        C: CMS + 'static,
        E: RenderElement<R, C>,
    {
        // if we have no cursor plane we can exit early
        let Some(plane_info) = self.planes.cursor.as_ref() else {
            return None;
        };

        // something is already assigned to our cursor plane
        if frame_state.is_assigned(plane_info.handle) {
            trace!(
                "skipping element {:?} on cursor plane {:?}, plane already has element assigned",
                element.id(),
                plane_info.handle
            );
            return None;
        }

        let element_geometry = element.geometry(scale);
        let element_size = output_transform.transform_size(element_geometry.size);

        // if the element is greater than the cursor size we can not
        // use the cursor plane to scan out the element
        if element_size.w > self.cursor_size.w || element_size.h > self.cursor_size.h {
            trace!(
                "element {:?} too big for cursor plane {:?}, skipping",
                element.id(),
                plane_info.handle,
            );
            return None;
        }

        // if the element exposes the underlying storage we can try to do
        // direct scan-out
        if let Some(underlying_storage) = element.underlying_storage(renderer) {
            trace!(
                "trying to assign element {:?} for direct scan-out on cursor plane {:?}",
                element.id(),
                plane_info.handle,
            );
            if let Ok(Ok(plane)) = self.try_assign_plane(
                element,
                element_geometry,
                &underlying_storage,
                plane_info,
                element_states,
                scale,
                frame_state,
                output_damage,
                output_transform,
                output_geometry,
            ) {
                trace!(
                    "assigned element {:?} for direct scan-out on cursor plane {:?}",
                    element.id(),
                    plane_info.handle,
                );
                return Some(plane);
            }
        }

        let Some(cursor_state) = self.cursor_state.as_mut() else {
            trace!("no cursor state, skipping cursor rendering");
            return None;
        };

        // this calculates the location of the cursor plane taking the simulated transform
        // into consideration
        let cursor_plane_location = output_transform
            .transform_point_in(element.location(scale), &output_geometry.size)
            - output_transform.transform_point_in(Point::default(), &self.cursor_size);

        let previous_state = self
            .pending_frame
            .as_ref()
            .map(|(state, _)| state)
            .unwrap_or(&self.current_frame);

        let previous_element_state = previous_state
            .plane_state(plane_info.handle)
            .and_then(|state| state.element_state.as_ref());

        // if the output transform or scale change we have to (re-)render the cursor plane,
        // also if the element changed or reports damage we have to render it
        let render = cursor_state
            .previous_output_transform
            .map(|t| t != output_transform)
            .unwrap_or(true)
            || cursor_state
                .previous_output_scale
                .map(|s| s != scale)
                .unwrap_or(true)
            || previous_element_state
                .map(|(id, commit_counter)| {
                    id != element.id() || !element.damage_since(scale, Some(*commit_counter)).is_empty()
                })
                .unwrap_or(true);

        // check if the cursor plane location changed
        let reposition = previous_state
            .plane_state(plane_info.handle)
            .and_then(|state| {
                state
                    .config
                    .as_ref()
                    .map(|config| config.dst.loc != cursor_plane_location)
            })
            .unwrap_or_default();

        // ok, nothing changed, try to keep the previous state
        if !render && !reposition {
            let mut plane_state = previous_state.plane_state(plane_info.handle).unwrap().clone();
            plane_state.skip = true;

            let res = frame_state.test_state(&self.surface, plane_info.handle, plane_state, false);

            if res.is_ok() {
                return Some(*plane_info);
            } else {
                return None;
            }
        }

        // we no not have to re-render but update the planes location
        if !render && reposition {
            trace!("repositioning cursor plane");
            let mut plane_state = previous_state.plane_state(plane_info.handle).unwrap().clone();
            plane_state.skip = false;
            let config = plane_state.config.as_mut().unwrap();
            config.dst.loc = cursor_plane_location;
            let res = frame_state.test_state(&self.surface, plane_info.handle, plane_state, false);

            if res.is_ok() {
                return Some(*plane_info);
            } else {
                return None;
            }
        }

        trace!(
            "trying to render element {:?} on cursor plane {:?}",
            element.id(),
            plane_info.handle
        );

        // if we fail to create a buffer we can just return false and
        // force the cursor to be rendered on the primary plane
        let mut cursor_buffer = match cursor_state.allocator.create_buffer(
            self.cursor_size.w as u32,
            self.cursor_size.h as u32,
            DrmFourcc::Argb8888,
            &[DrmModifier::Linear],
        ) {
            Ok(buffer) => buffer,
            Err(err) => {
                debug!("failed to create cursor buffer: {}", err);
                return None;
            }
        };

        // if we fail to export a framebuffer for our buffer we can skip the rest
        let framebuffer = match cursor_state.framebuffer_exporter.add_framebuffer(
            self.surface.device_fd(),
            ExportBuffer::Allocator(&cursor_buffer),
            false,
        ) {
            Ok(Some(fb)) => fb,
            Ok(None) => {
                debug!(
                    "failed to export framebuffer for cursor plane {:?}: no framebuffer available",
                    plane_info.handle
                );
                return None;
            }
            Err(err) => {
                debug!(
                    "failed to export framebuffer for cursor plane {:?}: {}",
                    plane_info.handle, err
                );
                return None;
            }
        };

        let cursor_buffer_size = self.cursor_size.to_logical(1).to_buffer(1, Transform::Normal);
        let offscreen_buffer = match renderer.create_buffer(DrmFourcc::Argb8888, cursor_buffer_size) {
            Ok(buffer) => buffer,
            Err(err) => {
                debug!(
                    "failed to create offscreen buffer for cursor plane {:?}: {}",
                    plane_info.handle, err
                );
                return None;
            }
        };

        if let Err(err) = renderer.bind(offscreen_buffer) {
            debug!(
                "failed to bind cursor buffer for cursor plane {:?}: {}",
                plane_info.handle, err
            );
            return None;
        };

        // Try to claim the plane, if this fails we can not use it
        let plane_claim = match self.surface.claim_plane(plane_info.handle) {
            Some(claim) => claim,
            None => {
                trace!("failed to claim plane {:?}", plane_info.handle);
                return None;
            }
        };

        // save the renderer debug flags and disable all for the cursor plane
        let renderer_debug_flags = renderer.debug_flags();
        renderer.set_debug_flags(DebugFlags::empty());

        let mut render = || {
            let mut frame = renderer.render(self.cursor_size, output_transform, cms, output_profile)?;

            frame.clear(
                [0f32, 0f32, 0f32, 0f32],
                &[Rectangle::from_loc_and_size((0, 0), self.cursor_size)],
                output_profile, // doesn't matter, fully transparent
            )?;

            let src = element.src();
            let dst = Rectangle::from_loc_and_size((0, 0), element_geometry.size);
            element.draw(&mut frame, src, dst, &[dst])?;

            frame.finish()?;

            Ok::<(), <R as Renderer>::Error>(())
        };

        let render_res = render();

        // restore the renderer debug flags
        renderer.set_debug_flags(renderer_debug_flags);

        if let Err(err) = render_res {
            debug!("failed to render cursor element: {}", err);
            return None;
        }

        let copy_rect = Rectangle::from_loc_and_size((0, 0), cursor_buffer_size);
        let mapping = match renderer.copy_framebuffer(copy_rect, DrmFourcc::Abgr8888) {
            Ok(mapping) => mapping,
            Err(err) => {
                info!("failed to export cursor offscreen buffer: {}", err);
                return None;
            }
        };
        let data = match renderer.map_texture(&mapping) {
            Ok(data) => data,
            Err(err) => {
                info!("failed to map exported cursor offscreen buffer: {}", err);
                return None;
            }
        };

        if let Err(err) = cursor_buffer.write(data) {
            info!("failed to write cursor buffer; {}", err);
            return None;
        }

        let src = Rectangle::from_loc_and_size(Point::default(), cursor_buffer_size).to_f64();
        let dst = Rectangle::from_loc_and_size(cursor_plane_location, self.cursor_size);

        let config = Some(PlaneConfig {
            src,
            dst,
            transform: Transform::Normal,
            damage_clips: None,
            buffer: Owned::from(DrmScanoutBuffer {
                buffer: ScanoutBuffer::Cursor(cursor_buffer),
                fb: OwnedFramebuffer::new(DrmFramebuffer::Gbm(framebuffer)),
            }),
            plane_claim,
        });

        let plane_state = PlaneState {
            skip: false,
            element_state: Some((element.id().clone(), element.current_commit())),
            config,
        };

        let res = frame_state.test_state(&self.surface, plane_info.handle, plane_state, false);

        if res.is_ok() {
            cursor_state.previous_output_scale = Some(scale);
            cursor_state.previous_output_transform = Some(output_transform);
            output_damage.push(dst);
            Some(*plane_info)
        } else {
            info!("failed to test cursor plane {:?} state", plane_info.handle);
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "trace", skip_all)]
    fn try_assign_overlay_plane<'a, R, C, E>(
        &self,
        renderer: &mut R,
        element: &'a E,
        element_states: &mut IndexMap<
            Id,
            ElementFramebufferCache<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>,
        >,
        primary_plane_elements: &[&'a E],
        scale: Scale<f64>,
        frame_state: &mut Frame<A, F>,
        output_damage: &mut Vec<Rectangle<i32, Physical>>,
        output_transform: Transform,
        output_geometry: Rectangle<i32, Physical>,
    ) -> Result<Result<PlaneInfo, Option<RenderingReason>>, ExportBufferError>
    where
        R: Renderer,
        C: CMS,
        E: RenderElement<R, C>,
    {
        let element_id = element.id();

        // Check if we have a free plane, otherwise we can exit early
        if self.planes.overlay.is_empty()
            || self
                .planes
                .overlay
                .iter()
                .all(|plane| frame_state.is_assigned(plane.handle))
        {
            trace!(
                "skipping overlay planes for element {:?}, no free planes",
                element_id
            );
            return Ok(Err(None));
        }

        // We can only try to do direct scan-out for element that provide a underlying storage
        let underlying_storage = element
            .underlying_storage(renderer)
            .ok_or(ExportBufferError::NoUnderlyingStorage)?;

        let element_geometry = element.geometry(scale);

        let overlaps_with_primary_plane_element = primary_plane_elements.iter().any(|e| {
            let other_geometry = e.geometry(scale);
            other_geometry.overlaps(element_geometry)
        });

        let mut rendering_reason: Option<RenderingReason> = None;

        for plane in self.planes.overlay.iter() {
            // something is already assigned to our overlay plane
            if frame_state.is_assigned(plane.handle) {
                trace!(
                    "skipping plane {:?} with zpos {:?} for element {:?}, already has element assigned, skipping",
                    plane.handle,
                    plane.zpos,
                    element_id,
                );
                continue;
            }

            // test if the plane represents an underlay
            let is_underlay = self.planes.primary.zpos.unwrap_or_default() > plane.zpos.unwrap_or_default();

            if is_underlay && !element_is_opaque(element, scale) {
                trace!(
                    "skipping direct scan-out on plane plane {:?} with zpos {:?}, element {:?} is not opaque",
                    plane.handle,
                    plane.zpos,
                    element_id
                );
                continue;
            }

            // if the element overlaps with an element on
            // the primary plane and is not an underlay
            // we can not assign it to any overlay plane
            if overlaps_with_primary_plane_element && !is_underlay {
                trace!(
                    "skipping direct scan-out on plane plane {:?} with zpos {:?}, element {:?} overlaps with element on primary plane", plane.handle, plane.zpos, element_id,
                );
                return Ok(Err(None));
            }

            let overlaps_with_plane_underneath = self
                .planes
                .overlay
                .iter()
                .filter(|info| {
                    info.handle != plane.handle
                        && info.zpos.unwrap_or_default() <= plane.zpos.unwrap_or_default()
                })
                .any(|overlapping_plane| frame_state.overlaps(overlapping_plane.handle, element_geometry));

            // if we overlap we a plane below which already
            // has an element assigned we can not use the
            // plane for direct scan-out
            if overlaps_with_plane_underneath {
                trace!(
                    "skipping direct scan-out on plane {:?} with zpos {:?}, element {:?} geometry {:?} overlaps with plane underneath", plane.handle, plane.zpos, element_id, element_geometry,
                );
                continue;
            }

            match self.try_assign_plane(
                element,
                element_geometry,
                &underlying_storage,
                plane,
                element_states,
                scale,
                frame_state,
                output_damage,
                output_transform,
                output_geometry,
            )? {
                Ok(plane) => return Ok(Ok(plane)),
                Err(err) => rendering_reason = rendering_reason.or(err),
            }
        }

        Ok(Err(rendering_reason))
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "trace", skip_all)]
    fn try_assign_primary_plane<R, C, E>(
        &self,
        renderer: &mut R,
        element: &E,
        element_states: &mut IndexMap<
            Id,
            ElementFramebufferCache<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>,
        >,
        scale: Scale<f64>,
        frame_state: &mut Frame<A, F>,
        output_damage: &mut Vec<Rectangle<i32, Physical>>,
        output_transform: Transform,
        output_geometry: Rectangle<i32, Physical>,
    ) -> Result<Result<PlaneInfo, Option<RenderingReason>>, ExportBufferError>
    where
        R: Renderer,
        C: CMS,
        E: RenderElement<R, C>,
    {
        // We can only try to do direct scan-out for element that provide a underlying storage
        let underlying_storage = element
            .underlying_storage(renderer)
            .ok_or(ExportBufferError::NoUnderlyingStorage)?;

        // TODO: We should check if there is already an element assigned to the primary plane for completeness here

        trace!(
            "trying to assign element {:?} to primary plane {:?}",
            element.id(),
            self.planes.primary.handle
        );

        let element_geometry = element.geometry(scale);

        self.try_assign_plane(
            element,
            element_geometry,
            &underlying_storage,
            &self.planes.primary,
            element_states,
            scale,
            frame_state,
            output_damage,
            output_transform,
            output_geometry,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "trace", skip_all)]
    fn try_assign_plane<R, C, E>(
        &self,
        element: &E,
        element_geometry: Rectangle<i32, Physical>,
        underlying_storage: &UnderlyingStorage,
        plane: &PlaneInfo,
        element_states: &mut IndexMap<
            Id,
            ElementFramebufferCache<DrmFramebuffer<<F as ExportFramebuffer<A::Buffer>>::Framebuffer>>,
        >,
        scale: Scale<f64>,
        frame_state: &mut Frame<A, F>,
        output_damage: &mut Vec<Rectangle<i32, Physical>>,
        output_transform: Transform,
        output_geometry: Rectangle<i32, Physical>,
    ) -> Result<Result<PlaneInfo, Option<RenderingReason>>, ExportBufferError>
    where
        R: Renderer,
        C: CMS,
        E: RenderElement<R, C>,
    {
        let element_id = element.id();

        // First we try to find a state in our new states, this is important if
        // we got the same id multiple times. If we can't find it we use the previous
        // state if available
        let mut element_state = element_states
            .get(element_id)
            .or_else(|| self.element_states.get(element_id))
            .cloned()
            .unwrap_or_default();

        element_state.cleanup();

        let cached_fb = element_state.get(underlying_storage);

        if cached_fb.is_none() {
            trace!(
                "no cached fb, exporting new fb for element {:?} underlying storage {:?}",
                element_id,
                &underlying_storage
            );

            let fb = self
                .framebuffer_exporter
                .add_framebuffer(
                    self.surface.device_fd(),
                    ExportBuffer::from(underlying_storage),
                    plane.type_ == PlaneType::Primary,
                )
                .map_err(|err| {
                    trace!("failed to add framebuffer: {:?}", err);
                    ExportBufferError::ExportFailed
                })
                .and_then(|fb| {
                    fb.map(|fb| OwnedFramebuffer::new(DrmFramebuffer::Exporter(fb)))
                        .ok_or(ExportBufferError::Unsupported)
                });

            if fb.is_err() {
                trace!(
                    "could not import framebuffer for element {:?} underlying storage {:?}",
                    element_id,
                    &underlying_storage
                );
            }

            element_state.insert(underlying_storage, fb);
        } else {
            trace!(
                "using cached fb for element {:?} underlying storage {:?}",
                element_id,
                &underlying_storage
            );
        }

        element_states.insert(element_id.clone(), element_state.clone());
        let fb = element_state.get(underlying_storage).unwrap()?;

        let plane_claim = match self.surface.claim_plane(plane.handle) {
            Some(claim) => claim,
            None => {
                trace!(
                    "failed to claim plane {:?} for element {:?}",
                    plane.handle,
                    element_id
                );
                return Ok(Err(None));
            }
        };

        // Try to assign the element to a plane
        trace!("testing direct scan-out for element {:?} on plane {:?} with zpos {:?}: fb: {:?}, underlying storage: {:?}, element_geometry: {:?}", element_id, plane.handle, plane.zpos, &fb, &underlying_storage, element_geometry);

        let transform = apply_output_transform(
            apply_underlying_storage_transform(element.transform(), underlying_storage),
            output_transform,
        );

        let previous_state = self
            .pending_frame
            .as_ref()
            .map(|(state, _)| state)
            .unwrap_or(&self.current_frame);

        let previous_commit = previous_state.planes.get(&plane.handle).and_then(|state| {
            state.element_state.as_ref().and_then(
                |(id, counter)| {
                    if id == element_id {
                        Some(*counter)
                    } else {
                        None
                    }
                },
            )
        });

        let element_damage = element.damage_since(scale, previous_commit);

        let src = element.src();
        let dst = output_transform.transform_rect_in(element_geometry, &output_geometry.size);

        // We can only skip the plane update if we have no damage and if
        // the src/dst properties are unchanged. Also we can not skip if
        // the fb did change (this includes the case where we previously
        // had not assigned anything to the plane)
        let skip = element_damage.is_empty()
            && previous_state
                .plane_state(plane.handle)
                .map(|state| {
                    state
                        .config
                        .as_ref()
                        .map(|config| {
                            config.src == src
                                && config.dst == dst
                                && config.transform == transform
                                && config.buffer.fb == fb
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);

        let element_output_damage = element_damage
            .iter()
            .cloned()
            .map(|mut rect| {
                rect.loc += element_geometry.loc;
                rect
            })
            .collect::<Vec<_>>();

        let damage_clips = if element_damage.is_empty() {
            None
        } else {
            PlaneDamageClips::from_damage(self.surface.device_fd(), src, element_geometry, element_damage)
                .ok()
                .flatten()
        };

        let plane_state = PlaneState {
            skip,
            element_state: Some((element_id.clone(), element.current_commit())),
            config: Some(PlaneConfig {
                src,
                dst,
                transform,
                damage_clips,
                buffer: Owned::from(DrmScanoutBuffer {
                    fb,
                    buffer: ScanoutBuffer::from(underlying_storage.clone()),
                }),
                plane_claim,
            }),
        };

        let res = frame_state.test_state(&self.surface, plane.handle, plane_state, false);

        if res.is_ok() {
            output_damage.extend(element_output_damage);

            trace!(
                "successfully assigned element {:?} to plane {:?} with zpos {:?} for direct scan-out",
                element_id,
                plane.handle,
                plane.zpos,
            );

            Ok(Ok(*plane))
        } else {
            trace!(
                "skipping direct scan-out on plane {:?} with zpos {:?} for element {:?}, test failed",
                plane.handle,
                plane.zpos,
                element_id
            );

            Ok(Err(Some(RenderingReason::ScanoutFailed)))
        }
    }
}

fn apply_underlying_storage_transform(
    element_transform: Transform,
    storage: &UnderlyingStorage,
) -> Transform {
    match storage {
        UnderlyingStorage::Wayland(buffer) => {
            if buffer_y_inverted(buffer).unwrap_or(false) {
                match element_transform {
                    Transform::Normal => Transform::Flipped,
                    Transform::_90 => Transform::Flipped90,
                    Transform::_180 => Transform::Flipped180,
                    Transform::_270 => Transform::Flipped270,
                    Transform::Flipped => Transform::Normal,
                    Transform::Flipped90 => Transform::_90,
                    Transform::Flipped180 => Transform::_180,
                    Transform::Flipped270 => Transform::_270,
                }
            } else {
                element_transform
            }
        }
    }
}

fn apply_output_transform(transform: Transform, output_transform: Transform) -> Transform {
    match (transform, output_transform) {
        (Transform::Normal, output_transform) => output_transform,

        (Transform::_90, Transform::Normal) => Transform::_270,
        (Transform::_90, Transform::_90) => Transform::Normal,
        (Transform::_90, Transform::_180) => Transform::_90,
        (Transform::_90, Transform::_270) => Transform::_180,
        (Transform::_90, Transform::Flipped) => Transform::Flipped270,
        (Transform::_90, Transform::Flipped90) => Transform::Flipped,
        (Transform::_90, Transform::Flipped180) => Transform::Flipped90,
        (Transform::_90, Transform::Flipped270) => Transform::Flipped180,

        (Transform::_180, Transform::Normal) => Transform::_180,
        (Transform::_180, Transform::_90) => Transform::_270,
        (Transform::_180, Transform::_180) => Transform::Normal,
        (Transform::_180, Transform::_270) => Transform::_90,
        (Transform::_180, Transform::Flipped) => Transform::Flipped180,
        (Transform::_180, Transform::Flipped90) => Transform::Flipped270,
        (Transform::_180, Transform::Flipped180) => Transform::Flipped,
        (Transform::_180, Transform::Flipped270) => Transform::Flipped90,

        (Transform::_270, Transform::Normal) => Transform::_90,
        (Transform::_270, Transform::_90) => Transform::_180,
        (Transform::_270, Transform::_180) => Transform::_270,
        (Transform::_270, Transform::_270) => Transform::Normal,
        (Transform::_270, Transform::Flipped) => Transform::Flipped90,
        (Transform::_270, Transform::Flipped90) => Transform::Flipped180,
        (Transform::_270, Transform::Flipped180) => Transform::Flipped270,
        (Transform::_270, Transform::Flipped270) => Transform::Flipped,

        (Transform::Flipped, Transform::Normal) => Transform::Flipped,
        (Transform::Flipped, Transform::_90) => Transform::Flipped90,
        (Transform::Flipped, Transform::_180) => Transform::Flipped180,
        (Transform::Flipped, Transform::_270) => Transform::Flipped270,
        (Transform::Flipped, Transform::Flipped) => Transform::Normal,
        (Transform::Flipped, Transform::Flipped90) => Transform::_90,
        (Transform::Flipped, Transform::Flipped180) => Transform::_180,
        (Transform::Flipped, Transform::Flipped270) => Transform::_270,

        (Transform::Flipped90, Transform::Normal) => Transform::Flipped270,
        (Transform::Flipped90, Transform::_90) => Transform::Flipped,
        (Transform::Flipped90, Transform::_180) => Transform::Flipped90,
        (Transform::Flipped90, Transform::_270) => Transform::Flipped180,
        (Transform::Flipped90, Transform::Flipped) => Transform::_270,
        (Transform::Flipped90, Transform::Flipped90) => Transform::Normal,
        (Transform::Flipped90, Transform::Flipped180) => Transform::_90,
        (Transform::Flipped90, Transform::Flipped270) => Transform::_180,

        (Transform::Flipped180, Transform::Normal) => Transform::Flipped180,
        (Transform::Flipped180, Transform::_90) => Transform::Flipped270,
        (Transform::Flipped180, Transform::_180) => Transform::Flipped,
        (Transform::Flipped180, Transform::_270) => Transform::Flipped90,
        (Transform::Flipped180, Transform::Flipped) => Transform::_180,
        (Transform::Flipped180, Transform::Flipped90) => Transform::_270,
        (Transform::Flipped180, Transform::Flipped180) => Transform::Normal,
        (Transform::Flipped180, Transform::Flipped270) => Transform::_90,

        (Transform::Flipped270, Transform::Normal) => Transform::Flipped90,
        (Transform::Flipped270, Transform::_90) => Transform::Flipped180,
        (Transform::Flipped270, Transform::_180) => Transform::Flipped270,
        (Transform::Flipped270, Transform::_270) => Transform::Flipped,
        (Transform::Flipped270, Transform::Flipped) => Transform::_90,
        (Transform::Flipped270, Transform::Flipped90) => Transform::_180,
        (Transform::Flipped270, Transform::Flipped180) => Transform::_270,
        (Transform::Flipped270, Transform::Flipped270) => Transform::Normal,
    }
}

fn element_is_opaque<E: Element>(element: &E, scale: Scale<f64>) -> bool {
    let opaque_regions = element.opaque_regions(scale);
    let element_geometry = Rectangle::from_loc_and_size(Point::default(), element.geometry(scale).size);

    opaque_regions
        .iter()
        .fold([element_geometry].to_vec(), |geometry, opaque_region| {
            geometry
                .into_iter()
                .flat_map(|g| g.subtract_rect(*opaque_region))
                .collect::<Vec<_>>()
        })
        .is_empty()
}

struct OwnedFramebuffer<B: AsRef<framebuffer::Handle>>(Arc<B>);

impl<B: AsRef<framebuffer::Handle>> PartialEq for OwnedFramebuffer<B> {
    fn eq(&self, other: &Self) -> bool {
        AsRef::<framebuffer::Handle>::as_ref(&self) == AsRef::<framebuffer::Handle>::as_ref(&other)
    }
}

impl<B: AsRef<framebuffer::Handle> + std::fmt::Debug> std::fmt::Debug for OwnedFramebuffer<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OwnedFramebuffer").field(&self.0).finish()
    }
}

impl<B: AsRef<framebuffer::Handle>> OwnedFramebuffer<B> {
    fn new(buffer: B) -> Self {
        OwnedFramebuffer(Arc::new(buffer))
    }
}

impl<B: AsRef<framebuffer::Handle>> Clone for OwnedFramebuffer<B> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<B: AsRef<framebuffer::Handle>> AsRef<framebuffer::Handle> for OwnedFramebuffer<B> {
    fn as_ref(&self) -> &framebuffer::Handle {
        (*self.0).as_ref()
    }
}

/// Errors thrown by a [`DrmCompositor`]
#[derive(Debug, thiserror::Error)]
pub enum FrameError<
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
> {
    /// Failed to claim the primary plane
    #[error("Failed to claim the primary plane")]
    PrimaryPlaneClaimFailed,
    /// No supported pixel format for the given plane could be determined
    #[error("No supported plane buffer format found")]
    NoSupportedPlaneFormat,
    /// No supported pixel format for the given renderer could be determined
    #[error("No supported renderer buffer format found")]
    NoSupportedRendererFormat,
    /// The swapchain is exhausted, you need to call `frame_submitted`
    #[error("Failed to allocate a new buffer")]
    NoFreeSlotsError,
    /// Error accessing the drm device
    #[error("The underlying drm surface encountered an error: {0}")]
    DrmError(#[from] DrmError),
    /// Error during buffer allocation
    #[error("The underlying allocator encountered an error: {0}")]
    Allocator(#[source] A),
    /// Error during exporting the buffer as dmabuf
    #[error("Failed to export the allocated buffer as dmabuf: {0}")]
    AsDmabufError(#[source] B),
    /// Error during exporting a framebuffer
    #[error("The framebuffer export encountered an error: {0}")]
    FramebufferExport(#[source] F),
    /// No framebuffer available
    #[error("No framebuffer available")]
    NoFramebuffer,
    /// The frame is empty
    ///
    /// Possible reasons include not calling `render_frame` prior to
    /// `queue_frame` or trying to queue a frame without changes.
    #[error("No frame has been prepared or it does not contain any changes")]
    EmptyFrame,
}

/// Error returned from [`DrmCompositor::render_frame`]
#[derive(thiserror::Error)]
pub enum RenderFrameError<
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
    R: Renderer,
> {
    /// Preparing the frame encountered an error
    #[error(transparent)]
    PrepareFrame(#[from] FrameError<A, B, F>),
    /// Rendering the frame encountered en error
    #[error(transparent)]
    RenderFrame(#[from] OutputDamageTrackerError<R>),
}

impl<A, B, F, R> std::fmt::Debug for RenderFrameError<A, B, F, R>
where
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
    R: Renderer,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrepareFrame(arg0) => f.debug_tuple("PrepareFrame").field(arg0).finish(),
            Self::RenderFrame(arg0) => f.debug_tuple("RenderFrame").field(arg0).finish(),
        }
    }
}

impl<
        A: std::error::Error + Send + Sync + 'static,
        B: std::error::Error + Send + Sync + 'static,
        F: std::error::Error + Send + Sync + 'static,
    > From<FrameError<A, B, F>> for SwapBuffersError
{
    fn from(err: FrameError<A, B, F>) -> SwapBuffersError {
        match err {
            x @ FrameError::NoSupportedPlaneFormat
            | x @ FrameError::NoSupportedRendererFormat
            | x @ FrameError::PrimaryPlaneClaimFailed
            | x @ FrameError::NoFramebuffer => SwapBuffersError::ContextLost(Box::new(x)),
            x @ FrameError::NoFreeSlotsError | x @ FrameError::EmptyFrame => {
                SwapBuffersError::TemporaryFailure(Box::new(x))
            }
            FrameError::DrmError(err) => err.into(),
            FrameError::Allocator(err) => SwapBuffersError::ContextLost(Box::new(err)),
            FrameError::AsDmabufError(err) => SwapBuffersError::ContextLost(Box::new(err)),
            FrameError::FramebufferExport(err) => SwapBuffersError::ContextLost(Box::new(err)),
        }
    }
}
