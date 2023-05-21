//! ext-session-lock lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::wayland::compositor;
use crate::wayland::compositor::SurfaceAttributes;
use _session_lock::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use _session_lock::ext_session_lock_v1::{Error, ExtSessionLockV1, Request};
use wayland_protocols::ext::session_lock::v1::server as _session_lock;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::wayland::session_lock::surface::{ExtLockSurfaceUserData, LockSurface, LockSurfaceAttributes};
use crate::wayland::session_lock::{SessionLockHandler, SessionLockManagerState};

/// Surface role for ext-session-lock surfaces.
const LOCK_SURFACE_ROLE: &str = "ext_session_lock_surface_v1";

/// [`ExtSessionLockV1`] state.
#[derive(Debug)]
pub struct SessionLockState {
    pub(crate) lock_status: Arc<AtomicBool>,
}

impl SessionLockState {
    pub(crate) fn new() -> Self {
        Self {
            lock_status: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<D> Dispatch<ExtSessionLockV1, SessionLockState, D> for SessionLockManagerState
where
    D: Dispatch<ExtSessionLockV1, SessionLockState>,
    D: Dispatch<ExtSessionLockSurfaceV1, ExtLockSurfaceUserData>,
    D: SessionLockHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        lock: &ExtSessionLockV1,
        request: Request,
        data: &SessionLockState,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::GetLockSurface { id, surface, output } => {
                // Assign surface a role and ensure it never had one before.
                if compositor::get_role(&surface).is_some()
                    || compositor::give_role(&surface, LOCK_SURFACE_ROLE).is_err()
                {
                    lock.post_error(Error::Role, "Surface already has a role.");
                    return;
                }

                // Ensure output is not already locked.
                let lock_state = state.lock_state();
                if lock_state.locked_outputs.contains(&output) {
                    lock.post_error(Error::DuplicateOutput, "Output is already locked.");
                    return;
                }
                lock_state.locked_outputs.push(output.clone());

                // Ensure surface has no existing buffers attached.
                let has_buffer = compositor::with_states(&surface, |states| {
                    let pending = states.cached_state.pending::<SurfaceAttributes>();
                    let current = states.cached_state.current::<SurfaceAttributes>();
                    current.buffer.is_some() || pending.buffer.is_some()
                });
                if has_buffer {
                    lock.post_error(Error::AlreadyConstructed, "Surface has a buffer attached.");
                    return;
                }

                let data = ExtLockSurfaceUserData {
                    surface: surface.clone(),
                };
                let lock_surface = data_init.init(id, data);

                // Initialize surface data.
                compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(|| Mutex::new(LockSurfaceAttributes::default()))
                });

                // Add pre-commit hook for updating surface state.
                compositor::add_pre_commit_hook(&surface, |_dh, surface| {
                    compositor::with_states(surface, |states| {
                        let attributes = states.data_map.get::<Mutex<LockSurfaceAttributes>>();
                        let mut attributes = attributes.unwrap().lock().unwrap();

                        if let Some(state) = attributes.last_acked {
                            attributes.current = state;
                        }
                    });
                });

                // Call compositor handler.
                let lock_surface = LockSurface::new(surface, lock_surface);
                state.new_surface(lock_surface.clone(), output);

                // Send initial configure when the interface is bound.
                lock_surface.send_configure();
            }
            Request::UnlockAndDestroy => {
                // Ensure session is locked.
                if !data.lock_status.load(Ordering::Relaxed) {
                    lock.post_error(Error::InvalidUnlock, "Session is not locked.");
                }

                state.lock_state().locked_outputs.clear();
                state.unlock();
            }
            Request::Destroy => {
                // Ensure session is not locked.
                if data.lock_status.load(Ordering::Relaxed) {
                    lock.post_error(Error::InvalidDestroy, "Cannot destroy session lock while locked.");
                }
            }
            _ => unreachable!(),
        }
    }
}
