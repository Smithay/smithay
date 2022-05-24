//! Utilities for handling the `wp_viewporter` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation, use the [`viewporter_init`]
//! method provided by this module.
//!
//! ```
//! use smithay::wayland::viewporter::viewporter_init;
//!
//! # let mut display = wayland_server::Display::new();
//! // Call the init function:
//! viewporter_init(&mut display);
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
//!     states.cached_state.current::<ViewportCachedState>();
//! });
//! ```
//!
//! Before accessing the state you should call [`ensure_viewport_valid`]
//! to ensure the viewport is valid.
//!
//! Note: If you already hand over buffer management to smithay by using
//! [`on_commit_buffer_handler`](crate::backend::renderer::utils::on_commit_buffer_handler)
//! the implementation will already call [`ensure_viewport_valid`] for you.

use std::{cell::RefCell, ops::Deref as _};

use wayland_protocols::viewporter::server::{wp_viewport, wp_viewporter};
use wayland_server::{protocol::wl_surface, Display, Filter, Global, Main};

use crate::utils::{Logical, Rectangle, Size};

use super::compositor::{self, with_states, Cacheable, SurfaceData};

/// Create new [`wp_viewporter`](wayland_protocols::viewporter::server::wp_viewporter) global.
///
/// It returns the global handle, in case you wish to remove these global from
/// the event loop in the future.
pub fn viewporter_init(display: &mut Display) -> Global<wp_viewporter::WpViewporter> {
    display.create_global(
        1,
        Filter::new(move |(new_viewporter, _), _, _| {
            implement_viewporter(new_viewporter);
        }),
    )
}

fn implement_viewporter(viewporter: Main<wp_viewporter::WpViewporter>) -> wp_viewporter::WpViewporter {
    viewporter.quick_assign(move |_, request, _| match request {
        wp_viewporter::Request::GetViewport { id, surface } => {
            implement_viewport(id, surface);
        }
        wp_viewporter::Request::Destroy => {
            // All is already handled by our destructor
        }
        _ => unreachable!(),
    });
    viewporter.deref().clone()
}

#[derive(Default)]
struct Viewport(Option<Main<wp_viewport::WpViewport>>);

impl std::ops::Deref for Viewport {
    type Target = Option<Main<wp_viewport::WpViewport>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Viewport {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn implement_viewport(viewport: Main<wp_viewport::WpViewport>, surface: wl_surface::WlSurface) {
    let already_has_viewport = with_states(&surface, |states| {
        states
            .data_map
            .insert_if_missing(|| RefCell::new(Viewport::default()));
        states
            .data_map
            .get::<RefCell<Viewport>>()
            .unwrap()
            .borrow()
            .is_some()
    })
    .unwrap_or_default();

    if already_has_viewport {
        surface.as_ref().post_error(
            wp_viewporter::Error::ViewportExists as u32,
            "the surface already has a viewport object associated".to_string(),
        );
        return;
    }

    compositor::add_commit_hook(&surface, viewport_commit_hook);

    viewport.quick_assign(move |viewport, request, _| match request {
        wp_viewport::Request::Destroy => {
            let _ = with_states(&surface, |states| {
                states
                    .data_map
                    .get::<RefCell<Viewport>>()
                    .unwrap()
                    .borrow_mut()
                    .take();
                *states.cached_state.pending::<ViewportCachedState>() = ViewportCachedState::default();
            });
        }
        wp_viewport::Request::SetSource { x, y, width, height } => {
            // If all of x, y, width and height are -1.0, the source rectangle is unset instead.
            // Any other set of values where width or height are zero or negative,
            // or x or y are negative, raise the bad_value protocol error.
            let is_unset = x == -1.0 && y == -1.0 && width == -1.0 && height == -1.0;
            let is_valid_src = x >= 0.0 && y >= 0.0 && width > 0.0 && height > 0.0;

            if !is_unset && !is_valid_src {
                viewport.as_ref().post_error(
                    wp_viewport::Error::BadValue as u32,
                    "negative or zero values in width or height or negative values in x or y".to_string(),
                );
                return;
            }

            let res = with_states(&surface, |states| {
                let mut viewport_state = states.cached_state.pending::<ViewportCachedState>();
                let src = if is_unset {
                    None
                } else {
                    Some(Rectangle::from_loc_and_size((x, y), (width, height)))
                };
                viewport_state.src = src;
            });

            // If the wl_surface associated with the wp_viewport is destroyed,
            // all wp_viewport requests except 'destroy' raise the protocol error no_surface.
            if res.is_err() {
                viewport.as_ref().post_error(
                    wp_viewport::Error::NoSurface as u32,
                    "the wl_surface was destroyed".to_string(),
                );
            }
        }
        wp_viewport::Request::SetDestination { width, height } => {
            // If width is -1 and height is -1, the destination size is unset instead.
            // Any other pair of values for width and height that contains zero or
            // negative values raises the bad_value protocol error.
            let is_unset = width == -1 && height == -1;
            let is_valid_size = width > 0 && height > 0;

            if !is_unset && !is_valid_size {
                viewport.as_ref().post_error(
                    wp_viewport::Error::BadValue as u32,
                    "negative or zero values in width or height".to_string(),
                );
                return;
            }

            let res = with_states(&surface, |states| {
                let mut viewport_state = states.cached_state.pending::<ViewportCachedState>();
                let size = if is_unset {
                    None
                } else {
                    Some(Size::from((width, height)))
                };
                viewport_state.dst = size;
            });

            // If the wl_surface associated with the wp_viewport is destroyed,
            // all wp_viewport requests except 'destroy' raise the protocol error no_surface.
            if res.is_err() {
                viewport.as_ref().post_error(
                    wp_viewport::Error::NoSurface as u32,
                    "the wl_surface was destroyed".to_string(),
                );
            }
        }
        _ => unreachable!(),
    });
}

fn viewport_commit_hook(surface: &wl_surface::WlSurface) {
    let _ = with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing(|| RefCell::new(Viewport::default()));
        let viewport = states.data_map.get::<RefCell<Viewport>>().unwrap().borrow();
        if let Some(viewport) = &**viewport {
            let viewport_state = states.cached_state.pending::<ViewportCachedState>();

            // If src_width or src_height are not integers and destination size is not set,
            // the bad_size protocol error is raised when the surface state is applied.
            let src_size = viewport_state.src.map(|src| src.size);
            if viewport_state.dst.is_none()
                && src_size != src_size.map(|s| Size::from((s.w as i32, s.h as i32)).to_f64())
            {
                viewport.as_ref().post_error(
                    wp_viewport::Error::BadSize as u32,
                    "destination size is not integer".to_string(),
                );
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
        .insert_if_missing(|| RefCell::new(Viewport::default()));
    let viewport = states.data_map.get::<RefCell<Viewport>>().unwrap().borrow();

    if let Some(viewport) = &**viewport {
        let state = states.cached_state.pending::<ViewportCachedState>();

        let buffer_rect = Rectangle::from_loc_and_size((0.0, 0.0), buffer_size.to_f64());
        let src = state.src.unwrap_or(buffer_rect);
        let valid = buffer_rect.contains_rect(src);
        if !valid {
            viewport.as_ref().post_error(
                wp_viewport::Error::OutOfBuffer as u32,
                "source rectangle extends outside of the content area".to_string(),
            );
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
    fn commit(&mut self) -> Self {
        ViewportCachedState {
            src: self.src,
            dst: self.dst,
        }
    }

    fn merge_into(self, into: &mut Self) {
        into.src = self.src;
        into.dst = self.dst;
    }
}
