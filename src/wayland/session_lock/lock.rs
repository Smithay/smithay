//! ext-session-lock lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::wayland::compositor::SurfaceAttributes;
use crate::wayland::compositor::{self, BufferAssignment};
use _session_lock::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use _session_lock::ext_session_lock_v1::{Error, ExtSessionLockV1, Request};
use wayland_protocols::ext::session_lock::v1::server::{self as _session_lock};
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::wayland::Dispatch2;
use crate::wayland::session_lock::surface::{ExtLockSurfaceUserData, LockSurface, LockSurfaceAttributes};
use crate::wayland::session_lock::{LockStatus, SessionLockHandler};

/// Surface role for ext-session-lock surfaces.
const LOCK_SURFACE_ROLE: &str = "ext_session_lock_surface_v1";

/// [`ExtSessionLockV1`] state.
#[derive(Debug)]
pub struct SessionLockState {
    pub(super) done: Arc<AtomicBool>,
    locked_outputs: Mutex<Vec<WlOutput>>,
}

impl SessionLockState {
    pub(super) fn new() -> Self {
        Self {
            done: Arc::new(AtomicBool::new(false)),
            locked_outputs: Default::default(),
        }
    }
}

impl<D> Dispatch2<ExtSessionLockV1, D> for SessionLockState
where
    D: Dispatch<ExtSessionLockSurfaceV1, ExtLockSurfaceUserData>,
    D: SessionLockHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        lock: &ExtSessionLockV1,
        request: Request,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::GetLockSurface { id, surface, output } => {
                // Assign surface a role and ensure it never had one before.
                if compositor::give_role(&surface, LOCK_SURFACE_ROLE).is_err() {
                    lock.post_error(Error::Role, "Surface already has a role.");
                    return;
                }

                // Ensure output is not already locked.
                let mut locked_outputs = self.locked_outputs.lock().unwrap();
                if locked_outputs.contains(&output) {
                    lock.post_error(Error::DuplicateOutput, "Output is already locked.");
                    return;
                }
                locked_outputs.push(output.clone());
                drop(locked_outputs);

                // Ensure surface has no existing buffers attached.
                let has_buffer = compositor::with_states(&surface, |states| {
                    let cached = &states.cached_state;
                    let mut guard = cached.get::<SurfaceAttributes>();
                    let pending = matches!(guard.pending().buffer, Some(BufferAssignment::NewBuffer(_)));
                    let current = matches!(guard.current().buffer, Some(BufferAssignment::NewBuffer(_)));
                    pending || current
                });
                if has_buffer {
                    lock.post_error(Error::AlreadyConstructed, "Surface has a buffer attached.");
                    return;
                }

                let data = ExtLockSurfaceUserData {
                    surface: surface.downgrade(),
                    done: Arc::clone(&self.done),
                };
                let lock_surface = data_init.init(id, data);

                // Initialize surface data.
                compositor::with_states(&surface, |states| {
                    let inserted = states.data_map.insert_if_missing_threadsafe(|| {
                        Mutex::new(LockSurfaceAttributes::new(lock_surface.clone()))
                    });

                    if !inserted {
                        let mut attributes = states
                            .data_map
                            .get::<Mutex<LockSurfaceAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap();
                        attributes.surface = lock_surface.clone();
                    }
                });

                // Add pre-commit hook for updating surface state.
                compositor::add_pre_commit_hook::<D, _>(&surface, LockSurface::pre_commit_hook);

                if !self.done.load(Ordering::Acquire) {
                    // Call compositor handler.
                    let lock_surface = LockSurface::new(lock.clone(), surface, lock_surface);
                    state.new_surface(lock_surface.clone(), output);

                    // Send initial configure when the interface is bound.
                    lock_surface.send_configure();
                }
            }
            Request::UnlockAndDestroy => {
                // Ensure session is locked, and with the same lock instance.
                if !state.lock_state().lock_status.lock().unwrap().is_locked_by(lock) {
                    lock.post_error(Error::InvalidUnlock, "Session is not locked.");
                } else {
                    *state.lock_state().lock_status.lock().unwrap() = LockStatus::Unlocked;
                    state.unlock();
                }
            }
            Request::Destroy => {
                // Ensure session is not locked.
                if state.lock_state().lock_status.lock().unwrap().is_locked_by(lock) {
                    lock.post_error(Error::InvalidDestroy, "Cannot destroy session lock while locked.");
                }
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client: wayland_server::backend::ClientId, lock: &ExtSessionLockV1) {
        let mut lock_status = state.lock_state().lock_status.lock().unwrap();
        if lock_status.is_locked_by(lock) {
            // The client has disconnected without unlocking the session, so reset our state.  It
            // is up to the compositor's policy to decide whether it is allowed for another client
            // to connect and take over the session-locker responsibility.
            *lock_status = LockStatus::Defunct;
        }
    }
}
