//! Implementation of wp_content_type protocol
//!
//! ### Example
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};
//! use smithay::{
//!     delegate_content_type, delegate_compositor,
//!     wayland::compositor::{self, CompositorState, CompositorClientState, CompositorHandler},
//!     wayland::content_type::{ContentTypeSurfaceCachedState, ContentTypeState},
//! };
//!
//! pub struct State {
//!     compositor_state: CompositorState,
//! };
//! struct ClientState { compositor_state: CompositorClientState }
//! impl wayland_server::backend::ClientData for ClientState {}
//!
//! delegate_content_type!(State);
//! delegate_compositor!(State);
//!
//! impl CompositorHandler for State {
//!    fn compositor_state(&mut self) -> &mut CompositorState {
//!        &mut self.compositor_state
//!    }
//!
//!    fn client_compositor_state<'a>(&self, client: &'a wayland_server::Client) -> &'a CompositorClientState {
//!        &client.get_data::<ClientState>().unwrap().compositor_state    
//!    }
//!
//!    fn commit(&mut self, surface: &WlSurface) {
//!        compositor::with_states(&surface, |states| {
//!            let current = states.cached_state.current::<ContentTypeSurfaceCachedState>();
//!            dbg!(current.content_type());
//!        });
//!    }
//! }
//!
//! let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! let compositor_state = CompositorState::new::<State>(&display.handle());
//! ContentTypeState::new::<State>(&display.handle());
//!
//! let state = State {
//!     compositor_state,
//! };
//! ```

use std::sync::{
    atomic::{self, AtomicBool},
    Mutex,
};

use wayland_protocols::wp::content_type::v1::server::{
    wp_content_type_manager_v1::WpContentTypeManagerV1,
    wp_content_type_v1::{self, WpContentTypeV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch, Resource,
    Weak,
};

use super::compositor::Cacheable;

mod dispatch;

/// Data associated with WlSurface
/// Represents the client pending state
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::content_type::ContentTypeSurfaceCachedState;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     let current = states.cached_state.current::<ContentTypeSurfaceCachedState>();
///     dbg!(current.content_type());
/// });
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ContentTypeSurfaceCachedState {
    content_type: wp_content_type_v1::Type,
}

impl ContentTypeSurfaceCachedState {
    /// This informs the compositor that the client believes it is displaying buffers matching this content type.
    pub fn content_type(&self) -> &wp_content_type_v1::Type {
        &self.content_type
    }
}

impl Default for ContentTypeSurfaceCachedState {
    fn default() -> Self {
        Self {
            content_type: wp_content_type_v1::Type::None,
        }
    }
}

impl Cacheable for ContentTypeSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

#[derive(Debug)]
struct ContentTypeSurfaceData {
    is_resource_attached: AtomicBool,
}

impl ContentTypeSurfaceData {
    fn new() -> Self {
        Self {
            is_resource_attached: AtomicBool::new(false),
        }
    }

    fn set_is_resource_attached(&self, is_attached: bool) {
        self.is_resource_attached
            .store(is_attached, atomic::Ordering::Release)
    }

    fn is_resource_attached(&self) -> bool {
        self.is_resource_attached.load(atomic::Ordering::Acquire)
    }
}

/// User data of `WpContentTypeV1` object
#[derive(Debug)]
pub struct ContentTypeUserData(Mutex<Weak<WlSurface>>);

impl ContentTypeUserData {
    fn new(surface: WlSurface) -> Self {
        Self(Mutex::new(surface.downgrade()))
    }

    fn wl_surface(&self) -> Option<WlSurface> {
        self.0.lock().unwrap().upgrade().ok()
    }
}

/// Delegate type for [WpContentTypeManagerV1] global.
#[derive(Debug)]
pub struct ContentTypeState {
    global: GlobalId,
}

impl ContentTypeState {
    /// Regiseter new [WpContentTypeManagerV1] global
    pub fn new<D>(display: &DisplayHandle) -> ContentTypeState
    where
        D: GlobalDispatch<WpContentTypeManagerV1, ()>
            + Dispatch<WpContentTypeManagerV1, ()>
            + Dispatch<WpContentTypeV1, ContentTypeUserData>
            + 'static,
    {
        let global = display.create_global::<D, WpContentTypeManagerV1, _>(1, ());

        ContentTypeState { global }
    }

    /// Returns the WpContentTypeManagerV1 global id
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Macro to delegate implementation of the wp content type protocol
#[macro_export]
macro_rules! delegate_content_type {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __WpContentTypeManagerV1 =
            $crate::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_manager_v1::WpContentTypeManagerV1;
        type __WpContentTypeV1 =
            $crate::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_v1::WpContentTypeV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpContentTypeManagerV1: ()
            ] => $crate::wayland::content_type::ContentTypeState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpContentTypeManagerV1: ()
            ] => $crate::wayland::content_type::ContentTypeState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpContentTypeV1: $crate::wayland::content_type::ContentTypeUserData
            ] => $crate::wayland::content_type::ContentTypeState
        );
    };
}
