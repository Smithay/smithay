//! ext-session-lock surface.

use std::sync::Mutex;

use crate::backend::renderer::buffer_dimensions;
use crate::utils::{IsAlive, Logical, Serial, Size, SERIAL_COUNTER};
use crate::wayland::compositor::{self, BufferAssignment, Cacheable, SurfaceAttributes};
use crate::wayland::viewporter::{ViewportCachedState, ViewporterSurfaceState};
use _session_lock::ext_session_lock_surface_v1::{Error, ExtSessionLockSurfaceV1, Request};
use tracing::trace_span;
use wayland_protocols::ext::session_lock::v1::server::{self as _session_lock, ext_session_lock_surface_v1};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource, Weak};

use crate::wayland::session_lock::{SessionLockHandler, SessionLockManagerState};

/// User data for ext-session-lock surfaces.
#[derive(Debug)]
pub struct ExtLockSurfaceUserData {
    // `LockSurfaceAttributes` stored in the surface `data_map` contains a
    // `ExtSessionLockSurfaceV1`. So this reference needs to be weak to avoid a
    // cycle.
    pub(crate) surface: Weak<WlSurface>,
}

impl<D> Dispatch<ExtSessionLockSurfaceV1, ExtLockSurfaceUserData, D> for SessionLockManagerState
where
    D: Dispatch<ExtSessionLockSurfaceV1, ExtLockSurfaceUserData>,
    D: SessionLockHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        lock_surface: &ExtSessionLockSurfaceV1,
        request: Request,
        data: &ExtLockSurfaceUserData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::AckConfigure { serial } => {
                let Ok(surface) = data.surface.upgrade() else {
                    return;
                };

                // Find configure for this serial.
                let serial = Serial::from(serial);
                let configure = compositor::with_states(&surface, |states| {
                    let surface_data = states.data_map.get::<Mutex<LockSurfaceAttributes>>();
                    surface_data.unwrap().lock().unwrap().ack_configure(serial)
                });

                match configure {
                    Some(configure) => state.ack_configure(surface.clone(), configure),
                    None => lock_surface.post_error(
                        Error::InvalidSerial,
                        format!("wrong configure serial: {}", <u32>::from(serial)),
                    ),
                }
            }
            Request::Destroy => (),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: wayland_server::backend::ClientId,
        _resource: &ExtSessionLockSurfaceV1,
        data: &ExtLockSurfaceUserData,
    ) {
        if let Ok(surface) = data.surface.upgrade() {
            compositor::with_states(&surface, |states| {
                let mut attributes = states
                    .data_map
                    .get::<Mutex<LockSurfaceAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();
                attributes.reset();

                let mut guard = states.cached_state.get::<LockSurfaceCachedState>();
                *guard.pending() = Default::default();
                *guard.current() = Default::default();
            });
        }
    }
}

/// Data associated with session lock surface
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::session_lock::LockSurfaceData;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     states.data_map.get::<LockSurfaceData>();
/// });
/// ```
pub type LockSurfaceData = Mutex<LockSurfaceAttributes>;

/// Attributes for ext-session-lock surfaces.
#[derive(Debug)]
pub struct LockSurfaceAttributes {
    pub(crate) surface: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,

    /// Holds the pending state as set by the server.
    pub server_pending: Option<LockSurfaceState>,

    /// Holds the configures the server has sent out to the client waiting to be
    /// acknowledged by the client. All pending configures that are older than
    /// the acknowledged one will be discarded during processing
    /// layer_surface.ack_configure.
    pub pending_configures: Vec<LockSurfaceConfigure>,

    /// Holds the last configure that has been acknowledged by the client. This state should be
    /// cloned to the current during a commit. Note that this state can be newer than the last
    /// acked state at the time of the last commit.
    pub last_acked: Option<LockSurfaceConfigure>,
}

impl LockSurfaceAttributes {
    pub(crate) fn new(surface: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1) -> Self {
        Self {
            surface,
            server_pending: None,
            pending_configures: vec![],
            last_acked: None,
        }
    }

    fn ack_configure(&mut self, serial: Serial) -> Option<LockSurfaceConfigure> {
        let configure = self
            .pending_configures
            .iter()
            .find(|configure| configure.serial == serial)
            .cloned()?;

        self.pending_configures
            .retain(|configure| configure.serial > serial);
        self.last_acked = Some(configure);

        Some(configure)
    }

    fn reset(&mut self) {
        self.server_pending = None;
        self.pending_configures = Vec::new();
        self.last_acked = None;
    }

    fn current_server_state(&self) -> Option<&LockSurfaceState> {
        self.pending_configures
            .last()
            .map(|c| &c.state)
            .or(self.last_acked.as_ref().map(|c| &c.state))
    }
}

/// Handle for a ext-session-lock surface.
#[derive(Clone, Debug)]
pub struct LockSurface {
    shell_surface: ExtSessionLockSurfaceV1,
    surface: WlSurface,
}

impl PartialEq for LockSurface {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.surface == other.surface
    }
}

impl LockSurface {
    pub(crate) fn new(surface: WlSurface, shell_surface: ExtSessionLockSurfaceV1) -> Self {
        Self {
            surface,
            shell_surface,
        }
    }

    /// Check if the surface is still alive.
    #[inline]
    pub fn alive(&self) -> bool {
        self.surface.alive()
    }

    /// Get the current pending configure state.
    pub fn get_pending_state(&self, attributes: &mut LockSurfaceAttributes) -> Option<LockSurfaceState> {
        let server_pending = attributes.server_pending.take()?;

        // Check if last state matches pending state.
        match attributes.current_server_state() {
            Some(state) if state == &server_pending => None,
            _ => Some(server_pending),
        }
    }

    /// Send a configure to the surface.
    ///
    /// You can manipulate the client's state using
    /// [`LockSurface::with_pending_state`].
    pub fn send_configure(&self) {
        compositor::with_states(&self.surface, |states| {
            // Get surface attributes.
            let attributes = states.data_map.get::<Mutex<LockSurfaceAttributes>>();
            let mut attributes = attributes.unwrap().lock().unwrap();

            // Create our new configure event.
            let pending = match self.get_pending_state(&mut attributes) {
                Some(pending) => pending,
                None => return,
            };
            let configure = LockSurfaceConfigure::new(pending);

            // Extract client configure state.
            let (width, height) = configure.state.size.unwrap_or_default().into();
            let serial = configure.serial;

            // Update pending state.
            attributes.pending_configures.push(configure);

            // Send configure to the client.
            self.shell_surface.configure(serial.into(), width, height);
        })
    }

    /// Access the underlying [`WlSurface`].
    #[inline]
    pub fn wl_surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Manipulate this surface's pending state.
    pub fn with_pending_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut LockSurfaceState) -> T,
    {
        compositor::with_states(&self.surface, |states| {
            let attributes = states.data_map.get::<Mutex<LockSurfaceAttributes>>();
            let mut attributes = attributes.unwrap().lock().unwrap();

            // Ensure pending state is initialized.
            if attributes.server_pending.is_none() {
                attributes.server_pending =
                    Some(attributes.current_server_state().cloned().unwrap_or_default());
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
    }

    /// Provides access to the current committed cached state.
    pub fn with_cached_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&LockSurfaceCachedState) -> T,
    {
        compositor::with_states(&self.surface, |states| {
            let mut guard = states.cached_state.get::<LockSurfaceCachedState>();
            f(guard.current())
        })
    }

    /// Provides access to the current committed state.
    ///
    /// This is the state that the client last acked before making the current commit.
    pub fn with_committed_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(Option<&LockSurfaceState>) -> T,
    {
        self.with_cached_state(move |state| f(state.last_acked.as_ref().map(|c| &c.state)))
    }

    /// Handles the role specific commit error checking
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn pre_commit_hook<D: 'static>(_state: &mut D, _dh: &DisplayHandle, surface: &WlSurface) {
        let _span = trace_span!("session-lock-surface pre-commit", surface = %surface.id()).entered();

        compositor::with_states(surface, |states| {
            let role = states.data_map.get::<LockSurfaceData>().unwrap().lock().unwrap();

            let Some(last_acked) = role.last_acked else {
                role.surface.post_error(
                    ext_session_lock_surface_v1::Error::CommitBeforeFirstAck,
                    "Committed before the first ack_configure.",
                );
                return;
            };
            let LockSurfaceConfigure { state, serial: _ } = &last_acked;

            let mut guard_layer = states.cached_state.get::<LockSurfaceCachedState>();
            let pending = guard_layer.pending();

            // The presence of last_acked always follows the buffer assignment because the
            // surface is not allowed to attach a buffer without acking the initial configure.
            let had_buffer_before = pending.last_acked.is_some();

            let mut guard_surface = states.cached_state.get::<SurfaceAttributes>();
            let surface_attrs = guard_surface.pending();
            let has_buffer = match &surface_attrs.buffer {
                Some(BufferAssignment::NewBuffer(buffer)) => {
                    // Verify buffer size.
                    if let Some(buf_size) = buffer_dimensions(buffer) {
                        let viewport = states
                            .data_map
                            .get::<ViewporterSurfaceState>()
                            .map(|v| v.lock().unwrap());
                        let surface_size = if let Some(dest) = viewport.as_ref().and_then(|_| {
                            let mut guard = states.cached_state.get::<ViewportCachedState>();
                            let viewport_state = guard.pending();
                            viewport_state.dst
                        }) {
                            Size::from((dest.w as u32, dest.h as u32))
                        } else {
                            let scale = surface_attrs.buffer_scale;
                            let transform = surface_attrs.buffer_transform.into();
                            let surface_size = buf_size.to_logical(scale, transform);

                            Size::from((surface_size.w as u32, surface_size.h as u32))
                        };

                        if Some(surface_size) != state.size {
                            role.surface.post_error(
                                ext_session_lock_surface_v1::Error::DimensionsMismatch,
                                "Surface dimensions do not match acked configure.",
                            );
                            return;
                        }
                    }

                    true
                }
                Some(BufferAssignment::Removed) => {
                    role.surface.post_error(
                        ext_session_lock_surface_v1::Error::NullBuffer,
                        "Surface attached a NULL buffer.",
                    );
                    return;
                }
                None => had_buffer_before,
            };

            if has_buffer {
                // The surface remains, or became mapped, track the last acked state.
                pending.last_acked = Some(last_acked);
            } else {
                // The surface remains unmapped, meaning that it's in the initial configure stage.
                pending.last_acked = None;
            }

            // Lock surfaces aren't allowed to attach a null buffer, and therefore to unmap.
        });
    }
}

/// State of an ext-session-lock surface.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct LockSurfaceState {
    /// The suggested size of the surface.
    pub size: Option<Size<u32, Logical>>,
}

/// A configure message for ext-session-lock surfaces.
#[derive(Debug, Copy, Clone)]
pub struct LockSurfaceConfigure {
    /// The state associated with this configure.
    pub state: LockSurfaceState,

    /// A serial number to track acknowledgment from the client.
    pub serial: Serial,
}

impl LockSurfaceConfigure {
    fn new(state: LockSurfaceState) -> Self {
        Self {
            serial: SERIAL_COUNTER.next_serial(),
            state,
        }
    }
}

/// Represents the client pending state
#[derive(Debug, Default, Copy, Clone)]
pub struct LockSurfaceCachedState {
    /// Configure last acknowledged by the client at the time of the commit.
    ///
    /// Reset to `None` when the surface unmaps.
    pub last_acked: Option<LockSurfaceConfigure>,
}

impl Cacheable for LockSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }
    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}
