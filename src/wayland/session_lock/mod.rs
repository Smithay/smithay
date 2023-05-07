//! Utilities for handling the `ext-session-lock` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation create the [`SessionLockManagerState`] and
//! implement the [`SessionLockHandler`], as shown in this example:
//!
//! ```
//! use smithay::delegate_session_lock;
//! use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
//! use smithay::wayland::session_lock::{SessionLockManagerState, SessionLockHandler, SessionLocker};
//! use smithay::wayland::session_lock::surface::LockSurface;
//!
//! # struct State { session_lock_state: SessionLockManagerState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the compositor state
//! let session_lock_state = SessionLockManagerState::new::<State>(&display.handle());
//!
//! // Insert the SessionLockManagerState into your state.
//!
//! // Implement the necessary trait.
//! impl SessionLockHandler for State {
//!     fn lock_state(&mut self) -> &mut SessionLockManagerState {
//!         &mut self.session_lock_state
//!     }
//!
//!     fn lock(&mut self, _confirmation: SessionLocker) {
//!         // Lock and clear the screen.
//!
//!         // Call `SessionLocker::lock` after the cleared frame was presented.
//!     }
//!
//!     fn unlock(&mut self) {
//!         // Remove session lock.
//!     }
//!
//!     fn new_surface(&mut self, _surface: LockSurface, _output: WlOutput) {
//!         // Display `LockSurface` on `WlOutput`.
//!     }
//! }
//! delegate_session_lock!(State);
//!
//! // You're now ready to go!
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use _session_lock::ext_session_lock_manager_v1::{ExtSessionLockManagerV1, Request};
use _session_lock::ext_session_lock_v1::ExtSessionLockV1;
use wayland_protocols::ext::session_lock::v1::server as _session_lock;
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::wayland::session_lock::lock::SessionLockState;
use crate::wayland::session_lock::surface::{LockSurface, LockSurfaceConfigure};

pub mod lock;
pub mod surface;

const MANAGER_VERSION: u32 = 1;

/// State of the [`ExtSessionLockManagerV1`] Global.
#[derive(Debug)]
pub struct SessionLockManagerState {
    pub(crate) locked_outputs: Vec<WlOutput>,
}

impl SessionLockManagerState {
    /// Create new [`ExtSessionLockManagerV1`] global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ExtSessionLockManagerV1, ()>,
        D: Dispatch<ExtSessionLockManagerV1, ()>,
        D: Dispatch<ExtSessionLockV1, SessionLockState>,
        D: SessionLockHandler,
        D: 'static,
    {
        display.create_global::<D, ExtSessionLockManagerV1, _>(MANAGER_VERSION, ());

        Self {
            locked_outputs: Vec::new(),
        }
    }
}

impl<D> GlobalDispatch<ExtSessionLockManagerV1, (), D> for SessionLockManagerState
where
    D: GlobalDispatch<ExtSessionLockManagerV1, ()>,
    D: Dispatch<ExtSessionLockManagerV1, ()>,
    D: Dispatch<ExtSessionLockV1, SessionLockState>,
    D: SessionLockHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<ExtSessionLockManagerV1>,
        _manager_state: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }
}

impl<D> Dispatch<ExtSessionLockManagerV1, (), D> for SessionLockManagerState
where
    D: GlobalDispatch<ExtSessionLockManagerV1, ()>,
    D: Dispatch<ExtSessionLockManagerV1, ()>,
    D: Dispatch<ExtSessionLockV1, SessionLockState>,
    D: SessionLockHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _manager: &ExtSessionLockManagerV1,
        request: Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::Lock { id } => {
                let lock_state = SessionLockState::new();
                let lock_status = lock_state.lock_status.clone();
                let lock = data_init.init(id, lock_state);
                state.lock(SessionLocker::new(lock, lock_status));
            }
            Request::Destroy => (),
            _ => unreachable!(),
        }
    }
}

/// Handler trait for ext-session-lock.
pub trait SessionLockHandler {
    /// Session lock state.
    fn lock_state(&mut self) -> &mut SessionLockManagerState;

    /// Handle compositor locking requests.
    ///
    /// The [`SessionLocker`] parameter is used to confirm once the session was
    /// locked and no more client data is accessible using the
    /// [`SessionLocker::lock`] method.
    ///
    /// If locking was not possible, dropping the [`SessionLocker`] will
    /// automatically notify the requesting client about the failure.
    fn lock(&mut self, confirmation: SessionLocker);

    /// Handle compositor lock removal.
    fn unlock(&mut self);

    /// Add a new lock surface for an output.
    fn new_surface(&mut self, surface: LockSurface, output: WlOutput);

    /// A surface has acknowledged a configure serial.
    fn ack_configure(&mut self, _surface: WlSurface, _configure: LockSurfaceConfigure) {}
}

/// Manage session locking.
///
/// See [`SessionLockHandler::lock`] for more detail.
#[derive(Debug)]
pub struct SessionLocker {
    lock: Option<ExtSessionLockV1>,
    lock_status: Arc<AtomicBool>,
}

impl Drop for SessionLocker {
    fn drop(&mut self) {
        // If the session wasn't locked, we notify clients about the failure.
        if let Some(lock) = self.lock.take() {
            lock.finished();
        }
    }
}

impl SessionLocker {
    fn new(lock: ExtSessionLockV1, lock_status: Arc<AtomicBool>) -> Self {
        Self {
            lock: Some(lock),
            lock_status,
        }
    }

    /// Notify the client that the session lock was successful.
    pub fn lock(mut self) {
        if let Some(lock) = self.lock.take() {
            self.lock_status.store(true, Ordering::Relaxed);
            lock.locked();
        }
    }
}

#[allow(missing_docs)]
#[macro_export]
macro_rules! delegate_session_lock {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1: ()
        ] => $crate::wayland::session_lock::SessionLockManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1: ()
        ] => $crate::wayland::session_lock::SessionLockManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1: $crate::wayland::session_lock::lock::SessionLockState
        ] => $crate::wayland::session_lock::SessionLockManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1: $crate::wayland::session_lock::surface::ExtLockSurfaceUserData
        ] => $crate::wayland::session_lock::SessionLockManagerState);
    };
}
