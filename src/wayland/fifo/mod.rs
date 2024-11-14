//! Utilities for handling the `wp-fifo` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation create the [`FifoManagerState`] and store it inside your `State` struct.
//!
//! ```
//! use smithay::delegate_fifo;
//! use smithay::wayland::compositor;
//! use smithay::wayland::fifo::FifoManagerState;
//!
//! # struct State { fifo_manager_state: FifoManagerState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the compositor state
//! let fifo_manager_state = FifoManagerState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // insert the FifoManagerState into your state
//! // ..
//!
//! delegate_fifo!(State);
//!
//! // You're now ready to go!
//! ```
//!
//! ### Use the fifo state
//!
//! Whenever the client commits a surface content update requesting to wait on a previously set barrier
//! through [`wp_fifo_v1::Request::WaitBarrier`] the implementation will place a [`Blocker`](crate::wayland::compositor::Blocker)
//! on the surface.
//! A surface content update requesting to set a barrier through [`wp_fifo_v1::Request::SetBarrier`] will update the barrier
//! a later commit can use to wait on.
//!
//! It is your responsibility to query for a set barrier on a surface and signal it to allow clients to make forward progress.
//!
//! You can query the current barrier and signal it as shown in the following example:
//!
//! ```no_run
//! # use wayland_server::{backend::ObjectId, protocol::wl_surface, Resource};
//! use smithay::wayland::compositor;
//! use smithay::wayland::fifo::FifoBarrierCachedState;
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let dh = display.handle();
//! # let surface = wl_surface::WlSurface::from_id(&dh, ObjectId::null()).unwrap();
//! compositor::with_states(&surface, |states| {
//!     let fifo_barrier = states
//!         .cached_state
//!         .get::<FifoBarrierCachedState>()
//!         .current()
//!         .barrier
//!         .take();
//!     
//!     if let Some(fifo_barrier) = fifo_barrier {
//!         fifo_barrier.signal();
//!         // ..signal blocker cleared
//!     }
//! });
//! ```
//!
//! ### Unmanaged mode
//!
//! If for some reason the integrated solution for fifo does not suit your needs
//! you can create an unmanaged version with [`FifoManagerState::unmanaged`].
//! In this case it is your responsibility to listen for surface commits and place blockers
//! accordingly to the spec.
//!
//! The double buffered fifo state can be retrieved like shown in the following example:
//!
//! ```no_run
//! # let surface: wayland_server::protocol::wl_surface::WlSurface = todo!();
//! use smithay::wayland::compositor;
//! use smithay::wayland::fifo::FifoCachedState;
//!
//! let fifo_state = compositor::with_states(&surface, |states| {
//!     *states.cached_state.get::<FifoCachedState>().pending()
//! });
//! ```
use std::cell::RefCell;

use wayland_protocols::wp::fifo::v1::server::{
    wp_fifo_manager_v1::{self, WpFifoManagerV1},
    wp_fifo_v1::{self, WpFifoV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, DataInit, Dispatch, DisplayHandle, GlobalDispatch,
    New, Resource, Weak,
};

use crate::wayland::compositor::{add_blocker, add_pre_commit_hook};

use super::compositor::{is_sync_subsurface, with_states, Barrier, Cacheable};

/// State for the [`WpFifoManagerV1`] global
#[derive(Debug)]
pub struct FifoManagerState {
    global: GlobalId,
    is_managed: bool,
}

impl FifoManagerState {
    /// Create a new [`WpFifoManagerV1`] global
    //
    /// The id provided by [`FifoManagerState::global`] may be used to
    /// remove or disable this global in the future.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WpFifoManagerV1, bool>,
        D: Dispatch<WpFifoManagerV1, bool>,
        D: 'static,
    {
        Self::new_internal::<D>(display, true)
    }

    /// Create a new unmanaged [`WpFifoManagerV1`] global
    //
    /// The id provided by [`FifoManagerState::global`] may be used to
    /// remove or disable this global in the future.
    pub fn unmanaged<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WpFifoManagerV1, bool>,
        D: Dispatch<WpFifoManagerV1, bool>,
        D: 'static,
    {
        Self::new_internal::<D>(display, false)
    }

    fn new_internal<D>(display: &DisplayHandle, is_managed: bool) -> Self
    where
        D: GlobalDispatch<WpFifoManagerV1, bool>,
        D: Dispatch<WpFifoManagerV1, bool>,
        D: 'static,
    {
        let global = display.create_global::<D, WpFifoManagerV1, _>(1, is_managed);

        Self { global, is_managed }
    }

    /// Returns the id of the [`WpFifoManagerV1`] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    /// Returns if this [`FifoManagerState`] operates in managed mode.
    pub fn is_managed(&self) -> bool {
        self.is_managed
    }
}

impl<D> GlobalDispatch<WpFifoManagerV1, bool, D> for FifoManagerState
where
    D: GlobalDispatch<WpFifoManagerV1, bool>,
    D: Dispatch<WpFifoManagerV1, bool>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WpFifoManagerV1>,
        global_data: &bool,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, *global_data);
    }
}

impl<D> Dispatch<WpFifoManagerV1, bool, D> for FifoManagerState
where
    D: Dispatch<WpFifoManagerV1, bool>,
    D: Dispatch<WpFifoV1, Weak<WlSurface>>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WpFifoManagerV1,
        request: wp_fifo_manager_v1::Request,
        data: &bool,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let is_managed = *data;

        match request {
            wp_fifo_manager_v1::Request::GetFifo { id, surface } => {
                let (is_initial, has_active_fifo) = with_states(&surface, |states| {
                    let marker = states.data_map.get::<RefCell<FifoMarker>>();
                    (
                        marker.is_none(),
                        marker.map(|m| m.borrow().0.is_some()).unwrap_or(false),
                    )
                });

                // The protocol mandates that only a single fifo object is associated with a surface at all times
                if has_active_fifo {
                    surface.post_error(
                        wp_fifo_manager_v1::Error::AlreadyExists,
                        "the surface has already a fifo object associated",
                    );
                    return;
                }

                // Make sure we do not install the hook more then once in case the surface is being reused
                if is_managed && is_initial {
                    add_pre_commit_hook::<D, _>(&surface, |_, _, surface| {
                        let fifo_barrier = with_states(surface, |states| {
                            let fifo_state = *states.cached_state.get::<FifoCachedState>().pending();

                            // The pending state will contain any previously set barrier on this surface
                            // In case this commit updates the barrier with `set_barrier`, but also mandates to
                            // wait for a previously set barrier it is important to first retrieve the previously
                            // set barrier to not overwrite it with our own.
                            let fifo_barrier = fifo_state
                                .wait_barrier
                                .then(|| {
                                    states
                                        .cached_state
                                        .get::<FifoBarrierCachedState>()
                                        .pending()
                                        .barrier
                                        .take()
                                })
                                .flatten();

                            // If requested set the barrier for this commit.
                            // The barrier will be available for the next commit requesting to wait on it
                            // in the pending state.
                            // The barrier will also be either put in the current state in case this commit
                            // is not blocked or into a transaction otherwise eventually ending in the current
                            // state when it is unblocked.
                            if fifo_state.set_barrier {
                                states
                                    .cached_state
                                    .get::<FifoBarrierCachedState>()
                                    .pending()
                                    .barrier = Some(Barrier::new(false));
                            }

                            fifo_barrier
                        });

                        if let Some(barrier) = fifo_barrier {
                            // If multiple consecutive commits only call wait_barrier, but not set_barrier
                            // we might end up with the same barrier in multiple commits. It could happen
                            // that the barrier is already signaled in which case there is no need to
                            // further delay this commit
                            //
                            // In addition the spec also defines that the constraint must be ignored for
                            // sync subsurfaces
                            let skip = barrier.is_signaled() || is_sync_subsurface(surface);
                            if !skip {
                                add_blocker(surface, barrier);
                            }
                        }
                    });
                }

                let fifo: WpFifoV1 = data_init.init(id, surface.downgrade());

                with_states(&surface, |states| {
                    states
                        .data_map
                        .get_or_insert(|| RefCell::new(FifoMarker(None)))
                        .borrow_mut()
                        .0 = Some(fifo);
                });
            }
            wp_fifo_manager_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }
}

// Internal marker to track if a fifo object is currently associated with a surface.
// Used to realize the `AlreadyExists` protocol check.
struct FifoMarker(Option<WpFifoV1>);

impl<D> Dispatch<WpFifoV1, Weak<WlSurface>, D> for FifoManagerState
where
    D: Dispatch<WpFifoV1, Weak<WlSurface>>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        resource: &WpFifoV1,
        request: wp_fifo_v1::Request,
        data: &Weak<WlSurface>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_fifo_v1::Request::SetBarrier => {
                let Ok(surface) = data.upgrade() else {
                    resource.post_error(
                        wp_fifo_v1::Error::SurfaceDestroyed as u32,
                        "the surface associated with this fifo object has been destroyed".to_string(),
                    );
                    return;
                };
                with_states(&surface, move |states| {
                    states.cached_state.get::<FifoCachedState>().pending().set_barrier = true;
                });
            }
            wp_fifo_v1::Request::WaitBarrier => {
                let Ok(surface) = data.upgrade() else {
                    resource.post_error(
                        wp_fifo_v1::Error::SurfaceDestroyed as u32,
                        "the surface associated with this fifo object has been destroyed".to_string(),
                    );
                    return;
                };
                with_states(&surface, move |states| {
                    states
                        .cached_state
                        .get::<FifoCachedState>()
                        .pending()
                        .wait_barrier = true;
                });
            }
            wp_fifo_v1::Request::Destroy => {
                if let Ok(surface) = data.upgrade() {
                    with_states(&surface, |states| {
                        states
                            .data_map
                            .get::<RefCell<FifoMarker>>()
                            .unwrap()
                            .borrow_mut()
                            .0 = None;
                    });
                }
            }
            _ => unreachable!(),
        }
    }
}

/// State for the [`WpFifoV1`] object
#[derive(Debug, Default, Copy, Clone)]
pub struct FifoCachedState {
    /// The content update requested a barrier to be set
    pub set_barrier: bool,

    /// The content update requested to wait on a previously
    /// set barrier
    pub wait_barrier: bool,
}

impl Cacheable for FifoCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        std::mem::take(self)
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// Fifo barrier per surface state
#[derive(Debug, Default)]
pub struct FifoBarrierCachedState {
    /// The barrier set for the current content update
    pub barrier: Option<Barrier>,
}

impl Cacheable for FifoBarrierCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        Self {
            barrier: self.barrier.clone(),
        }
    }

    fn merge_into(mut self, into: &mut Self, _dh: &DisplayHandle) {
        let Some(barrier) = self.barrier.take() else {
            return;
        };

        if into.barrier.as_ref() == Some(&barrier) || barrier.is_signaled() {
            return;
        }

        if let Some(barrier) = into.barrier.replace(barrier) {
            barrier.signal();
        }
    }
}

/// Macro used to delegate [`WpFifoManagerV1`] events
#[macro_export]
macro_rules! delegate_fifo {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fifo::v1::server::wp_fifo_manager_v1::WpFifoManagerV1: bool
        ] => $crate::wayland::fifo::FifoManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fifo::v1::server::wp_fifo_manager_v1::WpFifoManagerV1: bool
        ] => $crate::wayland::fifo::FifoManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fifo::v1::server::wp_fifo_v1::WpFifoV1: $crate::reexports::wayland_server::Weak<$crate::reexports::wayland_server::protocol::wl_surface::WlSurface>
        ] => $crate::wayland::fifo::FifoManagerState);
    };
}
