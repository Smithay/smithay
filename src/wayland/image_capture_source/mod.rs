//! Image capture source protocol
//!
//! This module implements the `ext-image-capture-source-v1` protocol, which provides
//! opaque image capture source objects that can be created from outputs or toplevels.
//! These source objects are used by the `ext-image-copy-capture-v1` protocol to
//! specify what should be captured.
//!
//! ## Design Philosophy
//!
//! This implementation is designed to be **extensible**. The [`ImageCaptureSource`] type
//! is an opaque handle that compositors can associate with their own data via the
//! [`UserDataMap`](crate::utils::user_data::UserDataMap). This allows downstream
//! compositors to:
//!
//! - Define custom capture source types (e.g., workspace capture)
//! - Store their own representation of what each source captures
//! - Integrate with their existing surface/output abstractions
//!
//! ## How to use it
//!
//! ### Basic Usage (Output capture only)
//!
//! ```no_run
//! use smithay::delegate_image_capture_source;
//! use smithay::output::Output;
//! use smithay::wayland::image_capture_source::{
//!     ImageCaptureSourceState, ImageCaptureSourceHandler, ImageCaptureSource,
//! };
//!
//! pub struct State {
//!     image_capture_source: ImageCaptureSourceState,
//! }
//!
//! impl ImageCaptureSourceHandler for State {
//!     fn image_capture_source_state(&mut self) -> &mut ImageCaptureSourceState {
//!         &mut self.image_capture_source
//!     }
//!
//!     fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
//!         // Store your representation of the output in the source's user_data
//!         source.user_data().insert_if_missing(|| output.downgrade());
//!     }
//! }
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! // Create with output capture only
//! let state = ImageCaptureSourceState::new::<State>(&display_handle);
//!
//! delegate_image_capture_source!(State);
//! ```
//!
//! ### With Toplevel Capture
//!
//! ```no_run
//! use smithay::delegate_image_capture_source;
//! use smithay::output::Output;
//! use smithay::wayland::image_capture_source::{
//!     ImageCaptureSourceState, ImageCaptureSourceHandler, ImageCaptureSource,
//! };
//! use smithay::wayland::foreign_toplevel_list::{ForeignToplevelListState, ForeignToplevelHandle};
//!
//! pub struct State {
//!     image_capture_source: ImageCaptureSourceState,
//!     foreign_toplevel_list: ForeignToplevelListState,
//! }
//!
//! impl ImageCaptureSourceHandler for State {
//!     fn image_capture_source_state(&mut self) -> &mut ImageCaptureSourceState {
//!         &mut self.image_capture_source
//!     }
//!
//!     fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
//!         source.user_data().insert_if_missing(|| output.downgrade());
//!     }
//!
//!     fn toplevel_source_created(&mut self, source: ImageCaptureSource, toplevel: &ForeignToplevelHandle) {
//!         // Store the toplevel handle or your own surface representation
//!         source.user_data().insert_if_missing(|| toplevel.downgrade());
//!     }
//! }
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! // Create with both output and toplevel capture
//! let state = ImageCaptureSourceState::new_with_toplevel_capture::<State>(&display_handle);
//!
//! delegate_image_capture_source!(State);
//! ```
//!
//! ### Custom Capture Sources (e.g., Workspace Capture)
//!
//! Compositors can register custom source manager globals for protocol extensions:
//!
//! ```ignore
//! // In your compositor, handle your custom protocol's create_source request:
//! fn handle_workspace_create_source(&mut self, workspace: &WorkspaceHandle, source_id: New<ExtImageCaptureSourceV1>) {
//!     // Create the source through Smithay
//!     let source = self.image_capture_source_state().create_source();
//!
//!     // Store your custom data
//!     source.user_data().insert_if_missing(|| MyWorkspaceData::from(workspace));
//!
//!     // Initialize the protocol resource
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

/// Counter for generating unique source IDs.
static NEXT_SOURCE_ID: AtomicUsize = AtomicUsize::new(1);

/// Handler trait for the image capture source protocol.
///
/// Implement this on your compositor's state type, then call the
/// [`delegate_image_capture_source!`] macro.
///
/// The callback methods (`output_source_created`, `toplevel_source_created`, `source_destroyed`)
/// allow your compositor to associate custom data with each capture source via
/// [`ImageCaptureSource::user_data()`].
pub trait ImageCaptureSourceHandler:
    GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ImageCaptureSourceGlobalData>
    + GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ImageCaptureSourceGlobalData>
    + Dispatch<ExtOutputImageCaptureSourceManagerV1, ()>
    + Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()>
    + Dispatch<ExtImageCaptureSourceV1, ImageCaptureSourceData>
    + 'static
{
    /// Returns a mutable reference to the [`ImageCaptureSourceState`] delegate type.
    fn image_capture_source_state(&mut self) -> &mut ImageCaptureSourceState;

    /// Called when a capture source is created from an output.
    ///
    /// Use [`ImageCaptureSource::user_data()`] to store your representation of this
    /// output for use during capture operations.
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
    ///     source.user_data().insert_if_missing(|| output.downgrade());
    /// }
    /// ```
    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        let _ = (source, output);
    }

    /// Called when a capture source is created from a foreign toplevel.
    ///
    /// This is only called if toplevel capture is enabled via
    /// [`ImageCaptureSourceState::new_with_toplevel_capture()`].
    ///
    /// Use [`ImageCaptureSource::user_data()`] to store your representation of this
    /// toplevel for use during capture operations.
    fn toplevel_source_created(&mut self, source: ImageCaptureSource, toplevel: &ForeignToplevelHandle) {
        let _ = (source, toplevel);
    }

    /// Called when a capture source is destroyed by the client.
    ///
    /// Use this to clean up any compositor-side state associated with the source.
    fn source_destroyed(&mut self, source: ImageCaptureSource) {
        let _ = source;
    }
}

/// Inner state for an image capture source.
#[derive(Debug)]
struct ImageCaptureSourceInner {
    /// Unique identifier for this source instance.
    id: usize,
    /// Whether this source is still valid (not destroyed).
    alive: AtomicBool,
    /// Protocol resource instances for this source.
    instances: Mutex<Vec<Weak<ExtImageCaptureSourceV1>>>,
}

/// Weak reference to an [`ImageCaptureSource`].
///
/// Can be upgraded to a strong reference if the source is still alive.
#[derive(Debug, Clone)]
pub struct ImageCaptureSourceWeakHandle {
    inner: std::sync::Weak<(ImageCaptureSourceInner, UserDataMap)>,
}

impl ImageCaptureSourceWeakHandle {
    /// Attempt to upgrade to a strong [`ImageCaptureSource`] reference.
    ///
    /// Returns `None` if the source has been destroyed.
    pub fn upgrade(&self) -> Option<ImageCaptureSource> {
        Some(ImageCaptureSource {
            inner: self.inner.upgrade()?,
        })
    }
}

/// An opaque handle to an image capture source.
///
/// This handle represents a capturable resource. The actual type of resource
/// (output, toplevel, or custom) is determined by what the compositor stores
/// in [`Self::user_data()`].
///
/// This can be used with the `ext-image-copy-capture-v1` protocol to create
/// capture sessions.
///
/// ## Extensibility
///
/// Unlike a closed enum, this design allows compositors to define their own
/// capture source types by storing custom data in [`Self::user_data()`].
/// For example, a compositor could store workspace handles for workspace
/// capture without any changes to Smithay.
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
    /// This is called internally when sources are created from outputs or toplevels.
    /// Compositors implementing custom source types can also use this to create
    /// sources for their custom protocols.
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
    ///
    /// This ID is stable for the lifetime of the source and can be used
    /// as a key in compositor-side data structures.
    pub fn id(&self) -> usize {
        self.inner.0.id
    }

    /// Check if this capture source is still valid.
    ///
    /// A source becomes invalid when all protocol resources referencing it
    /// have been destroyed.
    pub fn alive(&self) -> bool {
        self.inner.0.alive.load(Ordering::Acquire)
    }

    /// Access the [`UserDataMap`] for storing compositor-specific data.
    ///
    /// Use this to associate your own representation of the capture target
    /// (output, toplevel, workspace, etc.) with this source.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Store output reference
    /// source.user_data().insert_if_missing(|| output.downgrade());
    ///
    /// // Later, retrieve it
    /// if let Some(weak_output) = source.user_data().get::<WeakOutput>() {
    ///     if let Some(output) = weak_output.upgrade() {
    ///         // Use the output for capture
    ///     }
    /// }
    /// ```
    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }

    /// Create a weak reference to this source.
    ///
    /// Useful for storing references without preventing cleanup.
    pub fn downgrade(&self) -> ImageCaptureSourceWeakHandle {
        ImageCaptureSourceWeakHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Attempt to retrieve an [`ImageCaptureSource`] from an existing protocol resource.
    pub fn from_resource(resource: &ExtImageCaptureSourceV1) -> Option<Self> {
        resource
            .data::<ImageCaptureSourceData>()
            .map(|d| d.source.clone())
    }

    /// Add a protocol resource instance to this source.
    fn add_instance(&self, resource: &ExtImageCaptureSourceV1) {
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

/// Data associated with an image capture source manager global.
#[allow(missing_debug_implementations)]
pub struct ImageCaptureSourceGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

impl std::fmt::Debug for ImageCaptureSourceGlobalData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageCaptureSourceGlobalData")
            .finish_non_exhaustive()
    }
}

/// User data associated with an [`ExtImageCaptureSourceV1`] resource.
#[derive(Debug)]
pub struct ImageCaptureSourceData {
    /// The capture source this resource represents.
    pub source: ImageCaptureSource,
}

/// State of the image capture source protocol.
#[derive(Debug)]
pub struct ImageCaptureSourceState {
    output_manager_global: GlobalId,
    toplevel_manager_global: Option<GlobalId>,
}

impl ImageCaptureSourceState {
    /// Register a new [`ExtOutputImageCaptureSourceManagerV1`] global.
    ///
    /// This enables clients to create capture sources from outputs only.
    /// Use [`Self::new_with_toplevel_capture`] to also enable toplevel capture.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: ImageCaptureSourceHandler,
    {
        Self::new_with_filter::<D, _>(display, |_| true)
    }

    /// Register a new [`ExtOutputImageCaptureSourceManagerV1`] global with a client filter.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: ImageCaptureSourceHandler,
        F: Fn(&Client) -> bool + Clone + Send + Sync + 'static,
    {
        let output_manager_global = display.create_global::<D, ExtOutputImageCaptureSourceManagerV1, _>(
            1,
            ImageCaptureSourceGlobalData {
                filter: Box::new(filter),
            },
        );

        Self {
            output_manager_global,
            toplevel_manager_global: None,
        }
    }

    /// Register both output and toplevel capture manager globals.
    ///
    /// This enables clients to create capture sources from both outputs and
    /// foreign toplevels. Requires the `ForeignToplevelListState` to be active.
    pub fn new_with_toplevel_capture<D>(display: &DisplayHandle) -> Self
    where
        D: ImageCaptureSourceHandler,
    {
        Self::new_with_toplevel_capture_and_filter::<D, _>(display, |_| true)
    }

    /// Register both output and toplevel capture manager globals with a client filter.
    pub fn new_with_toplevel_capture_and_filter<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: ImageCaptureSourceHandler,
        F: Fn(&Client) -> bool + Clone + Send + Sync + 'static,
    {
        let filter_clone = filter.clone();

        let output_manager_global = display.create_global::<D, ExtOutputImageCaptureSourceManagerV1, _>(
            1,
            ImageCaptureSourceGlobalData {
                filter: Box::new(filter),
            },
        );

        let toplevel_manager_global = display
            .create_global::<D, ExtForeignToplevelImageCaptureSourceManagerV1, _>(
                1,
                ImageCaptureSourceGlobalData {
                    filter: Box::new(filter_clone),
                },
            );

        Self {
            output_manager_global,
            toplevel_manager_global: Some(toplevel_manager_global),
        }
    }

    /// Get the [`GlobalId`] of the output image capture source manager.
    pub fn output_manager_global(&self) -> GlobalId {
        self.output_manager_global.clone()
    }

    /// Get the [`GlobalId`] of the foreign toplevel image capture source manager, if enabled.
    pub fn toplevel_manager_global(&self) -> Option<GlobalId> {
        self.toplevel_manager_global.clone()
    }

    /// Create a new [`ImageCaptureSource`] for use with custom capture source protocols.
    ///
    /// This is useful when implementing protocol extensions like workspace capture.
    /// After creating the source, store your custom data in its [`UserDataMap`](ImageCaptureSource::user_data)
    /// and use it to initialize the protocol resource.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // In your custom protocol's create_source request handler:
    /// let source = state.image_capture_source_state().create_source();
    /// source.user_data().insert_if_missing(|| MyWorkspaceData::from(workspace));
    /// data_init.init(source_id, ImageCaptureSourceData { source });
    /// ```
    pub fn create_source(&self) -> ImageCaptureSource {
        ImageCaptureSource::new()
    }
}

// GlobalDispatch for output capture source manager
impl<D> GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ImageCaptureSourceGlobalData, D>
    for ImageCaptureSourceState
where
    D: ImageCaptureSourceHandler,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _global_data: &ImageCaptureSourceGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &ImageCaptureSourceGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

// GlobalDispatch for toplevel capture source manager
impl<D> GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ImageCaptureSourceGlobalData, D>
    for ImageCaptureSourceState
where
    D: ImageCaptureSourceHandler,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _global_data: &ImageCaptureSourceGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &ImageCaptureSourceGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

// Dispatch for output capture source manager
impl<D> Dispatch<ExtOutputImageCaptureSourceManagerV1, (), D> for ImageCaptureSourceState
where
    D: ImageCaptureSourceHandler,
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

                // Initialize the protocol resource
                let source_resource = data_init.init(
                    source,
                    ImageCaptureSourceData {
                        source: capture_source.clone(),
                    },
                );

                // Track this resource instance
                capture_source.add_instance(&source_resource);

                // Notify the compositor
                state.output_source_created(capture_source, &output_inner);
            }
            ext_output_image_capture_source_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

// Dispatch for toplevel capture source manager
impl<D> Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, (), D> for ImageCaptureSourceState
where
    D: ImageCaptureSourceHandler,
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

                // Initialize the protocol resource
                let source_resource = data_init.init(
                    source,
                    ImageCaptureSourceData {
                        source: capture_source.clone(),
                    },
                );

                // Track this resource instance
                capture_source.add_instance(&source_resource);

                // Notify the compositor
                state.toplevel_source_created(capture_source, &handle);
            }
            ext_foreign_toplevel_image_capture_source_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

// Dispatch for the capture source itself
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
        // Mark the source as destroyed
        data.source.mark_destroyed();

        // Notify the compositor
        state.source_destroyed(data.source.clone());
    }
}

/// Macro to delegate implementation of the image capture source protocol to [`ImageCaptureSourceState`].
///
/// You must also implement [`ImageCaptureSourceHandler`] to use this.
#[macro_export]
macro_rules! delegate_image_capture_source {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        const _: () = {
            use $crate::reexports::wayland_protocols::ext::image_capture_source::v1::server::{
                ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
                ext_image_capture_source_v1::ExtImageCaptureSourceV1,
                ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
            };
            use $crate::reexports::wayland_server::{delegate_dispatch, delegate_global_dispatch};
            use $crate::wayland::image_capture_source::{
                ImageCaptureSourceData, ImageCaptureSourceGlobalData, ImageCaptureSourceState,
            };

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtOutputImageCaptureSourceManagerV1: ImageCaptureSourceGlobalData] => ImageCaptureSourceState
            );
            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtForeignToplevelImageCaptureSourceManagerV1: ImageCaptureSourceGlobalData] => ImageCaptureSourceState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtOutputImageCaptureSourceManagerV1: ()] => ImageCaptureSourceState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtForeignToplevelImageCaptureSourceManagerV1: ()] => ImageCaptureSourceState
            );
            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtImageCaptureSourceV1: ImageCaptureSourceData] => ImageCaptureSourceState
            );
        };
    };
}
