//! Image capture source protocol
//!
//! This module implements the `ext-image-capture-source-v1` protocol, which provides
//! opaque image capture source objects that can be created from outputs, toplevels,
//! or custom compositor-defined sources.
//!
//! ## Architecture
//!
//! This implementation is **modular**. Each source type (output, toplevel, custom) has
//! its own independent state and handler:
//!
//! - [`ImageCaptureSourceState`] + [`ImageCaptureSourceHandler`]: Core source handling
//! - [`OutputCaptureSourceState`] + [`OutputCaptureSourceHandler`]: Output capture sources
//! - [`ToplevelCaptureSourceState`] + [`ToplevelCaptureSourceHandler`]: Toplevel capture sources
//!
//! Compositors only implement the source types they need. Custom source types (like
//! workspace capture) can be implemented by compositors using [`ImageCaptureSource::new()`]
//! directly.
//!
//! ## How to use it
//!
//! ### Output Capture Only
//!
//! ```no_run
//! use smithay::delegate_image_capture_source;
//! use smithay::delegate_output_capture_source;
//! use smithay::output::Output;
//! use smithay::wayland::image_capture_source::{
//!     ImageCaptureSourceState, ImageCaptureSourceHandler, ImageCaptureSource,
//!     OutputCaptureSourceState, OutputCaptureSourceHandler,
//! };
//!
//! pub struct State {
//!     image_capture_source: ImageCaptureSourceState,
//!     output_capture_source: OutputCaptureSourceState,
//! }
//!
//! impl ImageCaptureSourceHandler for State {
//!     fn source_destroyed(&mut self, source: ImageCaptureSource) {
//!         // Optional: clean up compositor-side state
//!     }
//! }
//!
//! impl OutputCaptureSourceHandler for State {
//!     fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
//!         &mut self.output_capture_source
//!     }
//!
//!     fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
//!         source.user_data().insert_if_missing(|| output.downgrade());
//!     }
//! }
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let image_capture_source = ImageCaptureSourceState::new();
//! let output_capture_source = OutputCaptureSourceState::new::<State>(&display_handle);
//!
//! delegate_image_capture_source!(State);
//! delegate_output_capture_source!(State);
//! ```
//!
//! ### With Toplevel Capture
//!
//! ```no_run
//! use smithay::delegate_image_capture_source;
//! use smithay::delegate_output_capture_source;
//! use smithay::delegate_toplevel_capture_source;
//! use smithay::output::Output;
//! use smithay::wayland::image_capture_source::{
//!     ImageCaptureSourceState, ImageCaptureSourceHandler, ImageCaptureSource,
//!     OutputCaptureSourceState, OutputCaptureSourceHandler,
//!     ToplevelCaptureSourceState, ToplevelCaptureSourceHandler,
//! };
//! use smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle;
//!
//! pub struct State {
//!     image_capture_source: ImageCaptureSourceState,
//!     output_capture_source: OutputCaptureSourceState,
//!     toplevel_capture_source: ToplevelCaptureSourceState,
//! }
//!
//! impl ImageCaptureSourceHandler for State {
//!     fn source_destroyed(&mut self, source: ImageCaptureSource) {
//!         // Optional cleanup
//!     }
//! }
//!
//! impl OutputCaptureSourceHandler for State {
//!     fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
//!         &mut self.output_capture_source
//!     }
//!
//!     fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
//!         source.user_data().insert_if_missing(|| output.downgrade());
//!     }
//! }
//!
//! impl ToplevelCaptureSourceHandler for State {
//!     fn toplevel_capture_source_state(&mut self) -> &mut ToplevelCaptureSourceState {
//!         &mut self.toplevel_capture_source
//!     }
//!
//!     fn toplevel_source_created(&mut self, source: ImageCaptureSource, toplevel: &ForeignToplevelHandle) {
//!         source.user_data().insert_if_missing(|| toplevel.downgrade());
//!     }
//! }
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let image_capture_source = ImageCaptureSourceState::new();
//! let output_capture_source = OutputCaptureSourceState::new::<State>(&display_handle);
//! let toplevel_capture_source = ToplevelCaptureSourceState::new::<State>(&display_handle);
//!
//! delegate_image_capture_source!(State);
//! delegate_output_capture_source!(State);
//! delegate_toplevel_capture_source!(State);
//! ```
//!
//! ### Custom Capture Sources
//!
//! Compositors can implement custom source types (e.g., workspace capture) by handling
//! their own protocol and creating sources directly:
//!
//! ```ignore
//! // In your custom protocol's create_source request handler:
//! fn handle_workspace_create_source(&mut self, workspace: &WorkspaceHandle, source_id: New<ExtImageCaptureSourceV1>) {
//!     let source = ImageCaptureSource::new();
//!     source.user_data().insert_if_missing(|| MyWorkspaceData::from(workspace));
//!     data_init.init(source_id, ImageCaptureSourceData { source });
//! }
//! ```

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use wayland_protocols::ext::image_capture_source::v1::server::{
    ext_foreign_toplevel_image_capture_source_manager_v1::{
        self, ExtForeignToplevelImageCaptureSourceManagerV1,
    },
    ext_image_capture_source_v1::{self, ExtImageCaptureSourceV1},
    ext_output_image_capture_source_manager_v1::{self, ExtOutputImageCaptureSourceManagerV1},
};
use wayland_server::{
    backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};

use crate::output::Output;
use crate::utils::user_data::UserDataMap;
use crate::wayland::foreign_toplevel_list::ForeignToplevelHandle;

// ============================================================================
// Core types
// ============================================================================

/// Counter for generating unique source IDs.
static NEXT_SOURCE_ID: AtomicUsize = AtomicUsize::new(1);

/// Inner state for an image capture source.
#[derive(Debug)]
struct ImageCaptureSourceInner {
    id: usize,
    alive: AtomicBool,
    instances: Mutex<Vec<Weak<ExtImageCaptureSourceV1>>>,
}

/// Weak reference to an [`ImageCaptureSource`].
#[derive(Debug, Clone)]
pub struct ImageCaptureSourceWeakHandle {
    inner: std::sync::Weak<(ImageCaptureSourceInner, UserDataMap)>,
}

impl ImageCaptureSourceWeakHandle {
    /// Attempt to upgrade to a strong [`ImageCaptureSource`] reference.
    pub fn upgrade(&self) -> Option<ImageCaptureSource> {
        Some(ImageCaptureSource {
            inner: self.inner.upgrade()?,
        })
    }
}

/// An opaque handle to an image capture source.
///
/// This represents a capturable resource. The actual type of resource
/// (output, toplevel, or custom) is determined by what the compositor stores
/// in [`Self::user_data()`].
#[derive(Debug, Clone)]
pub struct ImageCaptureSource {
    inner: Arc<(ImageCaptureSourceInner, UserDataMap)>,
}

impl PartialEq for ImageCaptureSource {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for ImageCaptureSource {}

impl std::hash::Hash for ImageCaptureSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

impl ImageCaptureSource {
    /// Create a new image capture source.
    ///
    /// Used internally when sources are created from outputs or toplevels.
    /// Compositors implementing custom source types can also use this directly.
    pub fn new() -> Self {
        Self {
            inner: Arc::new((
                ImageCaptureSourceInner {
                    id: NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed),
                    alive: AtomicBool::new(true),
                    instances: Mutex::new(Vec::new()),
                },
                UserDataMap::new(),
            )),
        }
    }

    /// Get the unique identifier for this capture source.
    pub fn id(&self) -> usize {
        self.inner.0.id
    }

    /// Check if this capture source is still valid.
    pub fn alive(&self) -> bool {
        self.inner.0.alive.load(Ordering::Acquire)
    }

    /// Access the [`UserDataMap`] for storing compositor-specific data.
    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }

    /// Create a weak reference to this source.
    pub fn downgrade(&self) -> ImageCaptureSourceWeakHandle {
        ImageCaptureSourceWeakHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Retrieve an [`ImageCaptureSource`] from an existing protocol resource.
    pub fn from_resource(resource: &ExtImageCaptureSourceV1) -> Option<Self> {
        resource
            .data::<ImageCaptureSourceData>()
            .map(|d| d.source.clone())
    }

    /// Add a protocol resource instance to this source.
    pub fn add_instance(&self, resource: &ExtImageCaptureSourceV1) {
        self.inner.0.instances.lock().unwrap().push(resource.downgrade());
    }

    /// Mark this source as destroyed.
    fn mark_destroyed(&self) {
        self.inner.0.alive.store(false, Ordering::Release);
    }
}

impl Default for ImageCaptureSource {
    fn default() -> Self {
        Self::new()
    }
}

/// User data associated with an [`ExtImageCaptureSourceV1`] resource.
#[derive(Debug)]
pub struct ImageCaptureSourceData {
    /// The capture source this resource represents.
    pub source: ImageCaptureSource,
}

// ============================================================================
// Core state and handler
// ============================================================================

/// Core state for image capture source handling.
///
/// This handles the [`ExtImageCaptureSourceV1`] resource itself. You must also
/// use [`OutputCaptureSourceState`] and/or [`ToplevelCaptureSourceState`] to
/// actually create sources.
#[derive(Debug, Default)]
pub struct ImageCaptureSourceState;

impl ImageCaptureSourceState {
    /// Create a new [`ImageCaptureSourceState`].
    pub fn new() -> Self {
        Self
    }
}

/// Core handler for image capture sources.
///
/// This is required by both [`OutputCaptureSourceHandler`] and
/// [`ToplevelCaptureSourceHandler`].
pub trait ImageCaptureSourceHandler:
    Dispatch<ExtImageCaptureSourceV1, ImageCaptureSourceData> + 'static
{
    /// Called when a capture source is destroyed by the client.
    ///
    /// Use this to clean up any compositor-side state associated with the source.
    fn source_destroyed(&mut self, source: ImageCaptureSource) {
        let _ = source;
    }
}

// Dispatch for the capture source resource
impl<D> Dispatch<ExtImageCaptureSourceV1, ImageCaptureSourceData, D> for ImageCaptureSourceState
where
    D: ImageCaptureSourceHandler,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ExtImageCaptureSourceV1,
        request: ext_image_capture_source_v1::Request,
        _data: &ImageCaptureSourceData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_image_capture_source_v1::Request::Destroy => {
                // Cleanup is handled in the `destroyed` callback
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        _resource: &ExtImageCaptureSourceV1,
        data: &ImageCaptureSourceData,
    ) {
        data.source.mark_destroyed();
        state.source_destroyed(data.source.clone());
    }
}

// ============================================================================
// Output capture source manager
// ============================================================================

/// Data for the output capture source manager global.
#[allow(missing_debug_implementations)]
pub struct OutputCaptureSourceGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

/// State for the output image capture source manager.
///
/// This binds the [`ExtOutputImageCaptureSourceManagerV1`] global, allowing
/// clients to create capture sources from outputs.
#[derive(Debug)]
pub struct OutputCaptureSourceState {
    global: GlobalId,
}

impl OutputCaptureSourceState {
    /// Register the output capture source manager global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: OutputCaptureSourceHandler,
    {
        Self::new_with_filter::<D, _>(display, |_| true)
    }

    /// Register the output capture source manager global with a client filter.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: OutputCaptureSourceHandler,
        F: Fn(&Client) -> bool + Send + Sync + 'static,
    {
        let global = display.create_global::<D, ExtOutputImageCaptureSourceManagerV1, _>(
            1,
            OutputCaptureSourceGlobalData {
                filter: Box::new(filter),
            },
        );

        Self { global }
    }

    /// Get the [`GlobalId`] of this manager.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Handler for output capture sources.
///
/// Implement this to enable output capture. Requires [`ImageCaptureSourceHandler`].
pub trait OutputCaptureSourceHandler:
    ImageCaptureSourceHandler
    + GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, OutputCaptureSourceGlobalData>
    + Dispatch<ExtOutputImageCaptureSourceManagerV1, ()>
{
    /// Returns a mutable reference to the [`OutputCaptureSourceState`].
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState;

    /// Called when a capture source is created from an output.
    ///
    /// Use [`ImageCaptureSource::user_data()`] to store your representation
    /// of this output.
    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        let _ = (source, output);
    }
}

impl<D> GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, OutputCaptureSourceGlobalData, D>
    for OutputCaptureSourceState
where
    D: OutputCaptureSourceHandler,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _global_data: &OutputCaptureSourceGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &OutputCaptureSourceGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ExtOutputImageCaptureSourceManagerV1, (), D> for OutputCaptureSourceState
where
    D: OutputCaptureSourceHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ExtOutputImageCaptureSourceManagerV1,
        request: ext_output_image_capture_source_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_output_image_capture_source_manager_v1::Request::CreateSource { source, output } => {
                let Some(output_inner) = Output::from_resource(&output) else {
                    resource.post_error(0u32, "invalid output");
                    return;
                };

                let capture_source = ImageCaptureSource::new();

                let source_resource = data_init.init(
                    source,
                    ImageCaptureSourceData {
                        source: capture_source.clone(),
                    },
                );

                capture_source.add_instance(&source_resource);
                state.output_source_created(capture_source, &output_inner);
            }
            ext_output_image_capture_source_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

// ============================================================================
// Toplevel capture source manager
// ============================================================================

/// Data for the toplevel capture source manager global.
#[allow(missing_debug_implementations)]
pub struct ToplevelCaptureSourceGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

/// State for the foreign toplevel image capture source manager.
///
/// This binds the [`ExtForeignToplevelImageCaptureSourceManagerV1`] global,
/// allowing clients to create capture sources from foreign toplevels.
///
/// This implementation uses Smithay's [`ForeignToplevelHandle`]. Compositors
/// with custom foreign-toplevel implementations should handle the global
/// themselves and use [`ImageCaptureSource::new()`] directly.
#[derive(Debug)]
pub struct ToplevelCaptureSourceState {
    global: GlobalId,
}

impl ToplevelCaptureSourceState {
    /// Register the toplevel capture source manager global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: ToplevelCaptureSourceHandler,
    {
        Self::new_with_filter::<D, _>(display, |_| true)
    }

    /// Register the toplevel capture source manager global with a client filter.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: ToplevelCaptureSourceHandler,
        F: Fn(&Client) -> bool + Send + Sync + 'static,
    {
        let global = display.create_global::<D, ExtForeignToplevelImageCaptureSourceManagerV1, _>(
            1,
            ToplevelCaptureSourceGlobalData {
                filter: Box::new(filter),
            },
        );

        Self { global }
    }

    /// Get the [`GlobalId`] of this manager.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Handler for toplevel capture sources.
///
/// Implement this to enable toplevel capture using Smithay's
/// [`ForeignToplevelHandle`]. Requires [`ImageCaptureSourceHandler`].
///
/// Compositors with custom foreign-toplevel implementations should NOT use
/// this. Instead, handle the protocol directly and use [`ImageCaptureSource::new()`].
pub trait ToplevelCaptureSourceHandler:
    ImageCaptureSourceHandler
    + GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ToplevelCaptureSourceGlobalData>
    + Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()>
{
    /// Returns a mutable reference to the [`ToplevelCaptureSourceState`].
    fn toplevel_capture_source_state(&mut self) -> &mut ToplevelCaptureSourceState;

    /// Called when a capture source is created from a foreign toplevel.
    ///
    /// Use [`ImageCaptureSource::user_data()`] to store your representation
    /// of this toplevel.
    fn toplevel_source_created(&mut self, source: ImageCaptureSource, toplevel: &ForeignToplevelHandle) {
        let _ = (source, toplevel);
    }
}

impl<D> GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ToplevelCaptureSourceGlobalData, D>
    for ToplevelCaptureSourceState
where
    D: ToplevelCaptureSourceHandler,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _global_data: &ToplevelCaptureSourceGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &ToplevelCaptureSourceGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, (), D> for ToplevelCaptureSourceState
where
    D: ToplevelCaptureSourceHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: ext_foreign_toplevel_image_capture_source_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_foreign_toplevel_image_capture_source_manager_v1::Request::CreateSource {
                source,
                toplevel_handle,
            } => {
                let Some(handle) = ForeignToplevelHandle::from_resource(&toplevel_handle) else {
                    resource.post_error(0u32, "invalid toplevel handle");
                    return;
                };

                if handle.is_closed() {
                    resource.post_error(0u32, "toplevel has been closed");
                    return;
                }

                let capture_source = ImageCaptureSource::new();

                let source_resource = data_init.init(
                    source,
                    ImageCaptureSourceData {
                        source: capture_source.clone(),
                    },
                );

                capture_source.add_instance(&source_resource);
                state.toplevel_source_created(capture_source, &handle);
            }
            ext_foreign_toplevel_image_capture_source_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

// ============================================================================
// Delegate macros
// ============================================================================

/// Delegate core image capture source handling to [`ImageCaptureSourceState`].
///
/// You must implement [`ImageCaptureSourceHandler`] to use this.
#[macro_export]
macro_rules! delegate_image_capture_source {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        const _: () = {
            use $crate::reexports::wayland_protocols::ext::image_capture_source::v1::server::{
                ext_image_capture_source_v1::ExtImageCaptureSourceV1,
            };
            use $crate::reexports::wayland_server::delegate_dispatch;
            use $crate::wayland::image_capture_source::{
                ImageCaptureSourceData, ImageCaptureSourceState,
            };

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCaptureSourceV1: ImageCaptureSourceData] => ImageCaptureSourceState
            );
        };
    };
}

/// Delegate output capture source management to [`OutputCaptureSourceState`].
///
/// You must implement [`OutputCaptureSourceHandler`] to use this.
#[macro_export]
macro_rules! delegate_output_capture_source {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        const _: () = {
            use $crate::reexports::wayland_protocols::ext::image_capture_source::v1::server::{
                ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
            };
            use $crate::reexports::wayland_server::{delegate_dispatch, delegate_global_dispatch};
            use $crate::wayland::image_capture_source::{
                OutputCaptureSourceGlobalData, OutputCaptureSourceState,
            };

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtOutputImageCaptureSourceManagerV1: OutputCaptureSourceGlobalData] => OutputCaptureSourceState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtOutputImageCaptureSourceManagerV1: ()] => OutputCaptureSourceState
            );
        };
    };
}

/// Delegate toplevel capture source management to [`ToplevelCaptureSourceState`].
///
/// You must implement [`ToplevelCaptureSourceHandler`] to use this.
///
/// This uses Smithay's [`ForeignToplevelHandle`]. Compositors with custom
/// foreign-toplevel implementations should handle the protocol directly instead.
#[macro_export]
macro_rules! delegate_toplevel_capture_source {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        const _: () = {
            use $crate::reexports::wayland_protocols::ext::image_capture_source::v1::server::{
                ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
            };
            use $crate::reexports::wayland_server::{delegate_dispatch, delegate_global_dispatch};
            use $crate::wayland::image_capture_source::{
                ToplevelCaptureSourceGlobalData, ToplevelCaptureSourceState,
            };

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtForeignToplevelImageCaptureSourceManagerV1: ToplevelCaptureSourceGlobalData] => ToplevelCaptureSourceState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtForeignToplevelImageCaptureSourceManagerV1: ()] => ToplevelCaptureSourceState
            );
        };
    };
}
