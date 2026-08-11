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
//! use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
//! use smithay::wayland::session_lock::{
//!     LockSurface, SessionLockManagerState, SessionLockHandler, SessionLocker,
//! };
//!
//! # struct State { session_lock_state: SessionLockManagerState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the compositor state
//! let session_lock_state = SessionLockManagerState::new::<State, _>(&display.handle(), |_| true);
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
//!         // Call `confirmation.lock()` after a cleared frame was presented on all outputs.
//!
//!         // Dropping `confirmation` will cancel the locking.
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
//!
//! smithay::delegate_dispatch2!(State);
//!
//! // You're now ready to go!
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use _session_lock::ext_session_lock_manager_v1::{ExtSessionLockManagerV1, Request};
use _session_lock::ext_session_lock_v1::ExtSessionLockV1;
use wayland_protocols::ext::session_lock::v1::server as _session_lock;
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::wayland::{Dispatch2, GlobalData, GlobalDispatch2};

mod lock;
mod surface;

pub use lock::SessionLockState;
pub use surface::{
    ExtLockSurfaceUserData, LockSurface, LockSurfaceAttributes, LockSurfaceCachedState, LockSurfaceConfigure,
    LockSurfaceData, LockSurfaceState,
};

const MANAGER_VERSION: u32 = 1;

#[derive(Debug)]
enum LockStatus {
    /// The session is unlocked.
    Unlocked,
    /// The session has been locked, and is owned by the specified instance.
    Locked(ExtSessionLockV1),
    /// The session was locked, but the locking client disconnected without unlocking.
    Defunct,
}

impl LockStatus {
    fn is_locked_by(&self, session_lock: &ExtSessionLockV1) -> bool {
        matches!(self, Self::Locked(owning_session_lock) if owning_session_lock == session_lock)
    }
}

/// State of the [`ExtSessionLockManagerV1`] Global.
#[derive(Debug)]
pub struct SessionLockManagerState {
    lock_status: Arc<Mutex<LockStatus>>,
}

impl SessionLockManagerState {
    /// Create new [`ExtSessionLockManagerV1`] global.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ExtSessionLockManagerV1, SessionLockManagerGlobalData>,
        D: Dispatch<ExtSessionLockManagerV1, GlobalData>,
        D: Dispatch<ExtSessionLockV1, SessionLockState>,
        D: SessionLockHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = SessionLockManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, ExtSessionLockManagerV1, _>(MANAGER_VERSION, data);

        Self {
            lock_status: Arc::new(Mutex::new(LockStatus::Unlocked)),
        }
    }
}

#[allow(missing_debug_implementations)]
#[doc(hidden)]
pub struct SessionLockManagerGlobalData {
    /// Filter whether the clients can view global.
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl<D> GlobalDispatch2<ExtSessionLockManagerV1, D> for SessionLockManagerGlobalData
where
    D: Dispatch<ExtSessionLockManagerV1, GlobalData>,
    D: SessionLockHandler,
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<ExtSessionLockManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, GlobalData);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<ExtSessionLockManagerV1, D> for GlobalData
where
    D: Dispatch<ExtSessionLockV1, SessionLockState>,
    D: SessionLockHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _manager: &ExtSessionLockManagerV1,
        request: Request,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::Lock { id } => {
                let lock_state = SessionLockState::new();
                let done = Arc::clone(&lock_state.done);
                let lock = data_init.init(id, lock_state);
                let lock_status = Arc::clone(&state.lock_state().lock_status);
                state.lock(SessionLocker::new(lock, lock_status, done));
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
    ///
    /// Note that this may be called for surfaces created on different [`SessionLocker`] instances
    /// if more than one client is racing to lock the screen, so you should match [`LockSurface`]
    /// instances with [`SessionLocker`] instances using the underlying [`ExtSessionLockV1`]
    /// instance of both.
    ///
    /// Once you have dropped the [`SessionLocker`] instances that will not be locking the session,
    /// this function will no longer be called for surfaces created by those locker instances.
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
    lock_status: Arc<Mutex<LockStatus>>,
    done: Arc<AtomicBool>,
}

impl Drop for SessionLocker {
    fn drop(&mut self) {
        // If the session wasn't locked, we notify clients about the failure.
        if let Some(lock) = self.lock.take() {
            lock.finished();
            self.done.store(true, Ordering::Release);
        }
    }
}

impl SessionLocker {
    fn new(lock: ExtSessionLockV1, lock_status: Arc<Mutex<LockStatus>>, done: Arc<AtomicBool>) -> Self {
        Self {
            lock: Some(lock),
            lock_status,
            done,
        }
    }

    /// Get the underlying [`ExtSessionLockV1`]
    pub fn ext_session_lock(&self) -> &ExtSessionLockV1 {
        self.lock.as_ref().unwrap()
    }

    /// Notify the client that the session lock was successful.
    pub fn lock(mut self) {
        if let Some(lock) = self.lock.take() {
            *self.lock_status.lock().unwrap() = LockStatus::Locked(lock.clone());
            lock.locked();
        }
    }
}
