//! Utilities for handling the `wp_viewporter` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation, create [`ViewporterState`], store it in your `State` struct and
//! implement the required traits, as shown in this example:
//!
//! ```
//! use smithay::wayland::viewporter::ViewporterState;
//! use smithay::delegate_viewporter;
//! # use smithay::delegate_compositor;
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! # use smithay::reexports::wayland_server::{Client, protocol::wl_surface::WlSurface};
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! // Create the viewporter state:
//! let viewporter_state = ViewporterState::new::<State>(
//!     &display.handle(), // the display
//! );
//!
//! // implement Dispatch for the Viewporter types
//! delegate_viewporter!(State);
//!
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &WlSurface) {}
//! # }
//! # delegate_compositor!(State);
//!
//! // You're now ready to go!
//! ```
//!
//! ### Use the viewport state
//!
//! The [`viewport state`](ViewportCachedState) is double-buffered and
//! can be accessed by using the [`with_states`] function
//!
//! ```no_compile
//! let viewport = with_states(surface, |states| {
//!     states.cached_state.get::<ViewportCachedState>().current()
//! });
//! ```
//!
//! Before accessing the state you should call [`ensure_viewport_valid`]
//! to ensure the viewport is valid.
//!
//! Note: If you already hand over buffer management to smithay by using
//! [`on_commit_buffer_handler`](crate::backend::renderer::utils::on_commit_buffer_handler)
//! the implementation will already call [`ensure_viewport_valid`] for you.

use std::sync::Mutex;

use tracing::trace;
use wayland_protocols::wp::viewporter::server::{wp_viewport, wp_viewporter};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface, Dispatch, DisplayHandle, GlobalDispatch, Resource, Weak,
};

use crate::utils::{Client, Logical, Rectangle, Size};

use super::compositor::{self, with_states, Cacheable, CompositorHandler, SurfaceData};

/// State of the wp_viewporter Global
#[derive(Debug)]
pub struct ViewporterState {
    global: GlobalId,
}

pub(crate) type ViewporterSurfaceState = Mutex<Option<ViewportMarker>>;

impl ViewporterState {
    /// Create new [`wp_viewporter`] global.
    ///
    /// It returns the viewporter state, which you can drop to remove these global from
    /// the event loop in the future.
    pub fn new<D>(display: &DisplayHandle) -> ViewporterState
    where
        D: GlobalDispatch<wp_viewporter::WpViewporter, ()>
            + Dispatch<wp_viewporter::WpViewporter, ()>
            + Dispatch<wp_viewport::WpViewport, ViewportState>
            + 'static,
    {
        ViewporterState {
            global: display.create_global::<D, wp_viewporter::WpViewporter, ()>(1, ()),
        }
    }

    /// Returns the viewporter global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<wp_viewporter::WpViewporter, (), D> for ViewporterState
where
    D: GlobalDispatch<wp_viewporter::WpViewporter, ()>,
    D: Dispatch<wp_viewporter::WpViewporter, ()>,
    D: Dispatch<wp_viewport::WpViewport, ViewportState>,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<wp_viewporter::WpViewporter>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<wp_viewporter::WpViewporter, (), D> for ViewporterState
where
    D: GlobalDispatch<wp_viewporter::WpViewporter, ()>,
    D: Dispatch<wp_viewporter::WpViewporter, ()>,
    D: Dispatch<wp_viewport::WpViewport, ViewportState>,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &wp_viewporter::WpViewporter,
        request: <wp_viewporter::WpViewporter as wayland_server::Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_viewporter::Request::GetViewport { id, surface } => {
                let already_has_viewport = with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<ViewporterSurfaceState>()
                        .map(|v| v.lock().unwrap().is_some())
                        .unwrap_or(false)
                });

                if already_has_viewport {
                    surface.post_error(
                        wp_viewporter::Error::ViewportExists as u32,
                        "the surface already has a viewport object associated".to_string(),
                    );
                    return;
                }

                let viewport = data_init.init(
                    id,
                    ViewportState {
                        surface: surface.downgrade(),
                    },
                );
                let initial = with_states(&surface, |states| {
                    let inserted = states
                        .data_map
                        .insert_if_missing_threadsafe::<ViewporterSurfaceState, _>(|| {
                            Mutex::new(Some(ViewportMarker(viewport.downgrade())))
                        });

                    // if we did not insert the marker it will be None as
                    // checked in already_has_viewport and we have to update
                    // it now
                    if !inserted {
                        *states
                            .data_map
                            .get::<ViewporterSurfaceState>()
                            .unwrap()
                            .lock()
                            .unwrap() = Some(ViewportMarker(viewport.downgrade()));
                    }

                    inserted
                });

                // only add the pre-commit hook once for the surface
                if initial {
                    compositor::add_pre_commit_hook::<D, _>(&surface, viewport_pre_commit_hook);
                }
            }
            wp_viewporter::Request::Destroy => {
                // All is already handled by our destructor
            }
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<wp_viewport::WpViewport, ViewportState, D> for ViewportState
where
    D: GlobalDispatch<wp_viewporter::WpViewporter, ()>,
    D: Dispatch<wp_viewporter::WpViewporter, ()>,
    D: Dispatch<wp_viewport::WpViewport, ViewportState>,
    D: CompositorHandler,
{
    fn request(
        state: &mut D,
        client: &wayland_server::Client,
        resource: &wp_viewport::WpViewport,
        request: <wp_viewport::WpViewport as wayland_server::Resource>::Request,
        data: &ViewportState,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_viewport::Request::Destroy => {
                if let Ok(surface) = data.surface.upgrade() {
                    with_states(&surface, |states| {
                        states
                            .data_map
                            .get::<ViewporterSurfaceState>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .take();
                        *states.cached_state.get::<ViewportCachedState>().pending() =
                            ViewportCachedState::default();
                    });
                }
            }
            wp_viewport::Request::SetSource { x, y, width, height } => {
                // If all of x, y, width and height are -1.0, the source rectangle is unset instead.
                // Any other set of values where width or height are zero or negative,
                // or x or y are negative, raise the bad_value protocol error.
                let is_unset = x == -1.0 && y == -1.0 && width == -1.0 && height == -1.0;
                let is_valid_src = x >= 0.0 && y >= 0.0 && width > 0.0 && height > 0.0;

                if !is_unset && !is_valid_src {
                    resource.post_error(
                        wp_viewport::Error::BadValue as u32,
                        "negative or zero values in width or height or negative values in x or y".to_string(),
                    );
                    return;
                }

                // If the wl_surface associated with the wp_viewport is destroyed,
                // all wp_viewport requests except 'destroy' raise the protocol error no_surface.
                let Ok(surface) = data.surface.upgrade() else {
                    resource.post_error(
                        wp_viewport::Error::NoSurface as u32,
                        "the wl_surface was destroyed".to_string(),
                    );
                    return;
                };

                with_states(&surface, |states| {
                    let mut guard = states.cached_state.get::<ViewportCachedState>();
                    let viewport_state = guard.pending();
                    let src = if is_unset {
                        None
                    } else {
                        let src = Rectangle::new((x, y).into(), (width, height).into());
                        trace!(surface = ?surface, src = ?src, "setting surface viewport src");
                        Some(src)
                    };
                    viewport_state.src = src;
                });
            }
            wp_viewport::Request::SetDestination { width, height } => {
                // If width is -1 and height is -1, the destination size is unset instead.
                // Any other pair of values for width and height that contains zero or
                // negative values raises the bad_value protocol error.
                let is_unset = width == -1 && height == -1;
                let is_valid_size = width > 0 && height > 0;

                if !is_unset && !is_valid_size {
                    resource.post_error(
                        wp_viewport::Error::BadValue as u32,
                        "negative or zero values in width or height".to_string(),
                    );
                    return;
                }

                // If the wl_surface associated with the wp_viewport is destroyed,
                // all wp_viewport requests except 'destroy' raise the protocol error no_surface.
                let Ok(surface) = data.surface.upgrade() else {
                    resource.post_error(
                        wp_viewport::Error::NoSurface as u32,
                        "the wl_surface was destroyed".to_string(),
                    );
                    return;
                };

                with_states(&surface, |states| {
                    let mut guard = states.cached_state.get::<ViewportCachedState>();
                    let viewport_state = guard.pending();
                    let size = if is_unset {
                        None
                    } else {
                        let client_scale = state.client_compositor_state(client).client_scale();
                        let dst = Size::<_, Client>::from((width, height))
                            .to_f64()
                            .to_logical(client_scale)
                            .to_i32_round();
                        trace!(surface = ?surface, size = ?dst, "setting surface viewport destination size");
                        Some(dst)
                    };
                    viewport_state.dst = size;
                });
            }
            _ => unreachable!(),
        }
    }
}

/// State of a single viewport attached to a surface
#[derive(Debug)]
pub struct ViewportState {
    surface: Weak<wl_surface::WlSurface>,
}

pub(crate) struct ViewportMarker(Weak<wp_viewport::WpViewport>);

fn viewport_pre_commit_hook<D: 'static>(
    _state: &mut D,
    _dh: &DisplayHandle,
    surface: &wl_surface::WlSurface,
) {
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe::<ViewporterSurfaceState, _>(|| Mutex::new(None));
        let viewport = states
            .data_map
            .get::<ViewporterSurfaceState>()
            .unwrap()
            .lock()
            .unwrap();
        if let Some(viewport) = &*viewport {
            let mut guard = states.cached_state.get::<ViewportCachedState>();
            let viewport_state = guard.pending();

            // If src_width or src_height are not integers and destination size is not set,
            // the bad_size protocol error is raised when the surface state is applied.
            let src_size = viewport_state.src.map(|src| src.size);
            if viewport_state.dst.is_none()
                && src_size != src_size.map(|s| Size::from((s.w as i32, s.h as i32)).to_f64())
            {
                if let Ok(viewport) = viewport.0.upgrade() {
                    viewport.post_error(
                        wp_viewport::Error::BadSize as u32,
                        "destination size is not integer".to_string(),
                    );
                }
            }
        }
    });
}

/// Ensures that the viewport, if any, is valid accordingly to the protocol specification.
///
/// If the viewport violates any protocol checks a protocol error will be raised and `false`
/// is returned.
pub fn ensure_viewport_valid(states: &SurfaceData, buffer_size: Size<i32, Logical>) -> bool {
    states
        .data_map
        .insert_if_missing_threadsafe::<ViewporterSurfaceState, _>(|| Mutex::new(None));
    let viewport = states
        .data_map
        .get::<ViewporterSurfaceState>()
        .unwrap()
        .lock()
        .unwrap();

    if let Some(viewport) = &*viewport {
        let mut guard = states.cached_state.get::<ViewportCachedState>();
        let state = guard.current();

        let buffer_rect = Rectangle::from_size(buffer_size.to_f64());
        let src = state.src.unwrap_or(buffer_rect);
        let valid = buffer_rect.contains_rect(src);
        if !valid {
            if let Ok(viewport) = viewport.0.upgrade() {
                viewport.post_error(
                    wp_viewport::Error::OutOfBuffer as u32,
                    format!(
                        "source rectangle x={},y={},w={},h={} extends outside of the content area x={},y={},w={},h={}", 
                        src.loc.x, src.loc.y, src.size.w, src.size.h,
                        buffer_rect.loc.x, buffer_rect.loc.y, buffer_rect.size.w, buffer_rect.size.h),
                );
            }
        }
        valid
    } else {
        true
    }
}

/// Represents the double-buffered viewport
/// state of a [`WlSurface`](wl_surface::WlSurface)
#[derive(Debug, Default, Clone, Copy)]
pub struct ViewportCachedState {
    /// Defines the source [`Rectangle`] of the [`WlSurface`](wl_surface::WlSurface) in [`Logical`]
    /// coordinates used for cropping.
    pub src: Option<Rectangle<f64, Logical>>,
    /// Defines the destination [`Size`] of the [`WlSurface`](wl_surface::WlSurface) in [`Logical`]
    /// coordinates used for scaling.
    pub dst: Option<Size<i32, Logical>>,
}

impl ViewportCachedState {
    /// Gets the actual size the [`WlSurface`](wl_surface::WlSurface) should have on screen in
    /// [`Logical`] coordinates.
    ///
    /// This will return the destination size if set or the size of the source rectangle.
    /// If both are unset `None` is returned.
    pub fn size(&self) -> Option<Size<i32, Logical>> {
        self.dst.or_else(|| {
            self.src
                .map(|src| Size::from((src.size.w as i32, src.size.h as i32)))
        })
    }
}

impl Cacheable for ViewportCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        ViewportCachedState {
            src: self.src,
            dst: self.dst,
        }
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        into.src = self.src;
        into.dst = self.dst;
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_viewporter {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::viewporter::server::wp_viewporter::WpViewporter: ()
        ] => $crate::wayland::viewporter::ViewporterState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::viewporter::server::wp_viewporter::WpViewporter: ()
        ] => $crate::wayland::viewporter::ViewporterState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::viewporter::server::wp_viewport::WpViewport: $crate::wayland::viewporter::ViewportState
        ] => $crate::wayland::viewporter::ViewportState);
    };
}
