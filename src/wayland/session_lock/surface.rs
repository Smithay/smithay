//! ext-session-lock surface.

use std::sync::Mutex;

use crate::utils::{IsAlive, Logical, Serial, Size, SERIAL_COUNTER};
use crate::wayland::compositor;
use _session_lock::ext_session_lock_surface_v1::{Error, ExtSessionLockSurfaceV1, Request};
use wayland_protocols::ext::session_lock::v1::server as _session_lock;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::wayland::session_lock::{SessionLockHandler, SessionLockManagerState};

/// User data for ext-session-lock surfaces.
#[derive(Debug)]
pub struct ExtLockSurfaceUserData {
    pub(crate) surface: WlSurface,
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
                // Find configure for this serial.
                let serial = Serial::from(serial);
                let configure = compositor::with_states(&data.surface, |states| {
                    let surface_data = states.data_map.get::<Mutex<LockSurfaceAttributes>>();
                    surface_data.unwrap().lock().unwrap().ack_configure(serial)
                });

                match configure {
                    Some(configure) => state.ack_configure(data.surface.clone(), configure),
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
}

/// Attributes for ext-session-lock surfaces.
#[derive(Default, Debug)]
pub struct LockSurfaceAttributes {
    /// The serial of the last acked configure
    pub configure_serial: Option<Serial>,

    /// Holds the pending state as set by the server.
    pub server_pending: Option<LockSurfaceState>,

    /// Holds the configures the server has sent out to the client waiting to be
    /// acknowledged by the client. All pending configures that are older than
    /// the acknowledged one will be discarded during processing
    /// layer_surface.ack_configure.
    pub pending_configures: Vec<LockSurfaceConfigure>,

    /// Holds the last server_pending state that has been acknowledged by the
    /// client. This state should be cloned to the current during a commit.
    pub last_acked: Option<LockSurfaceState>,

    /// Holds the current state of the layer after a successful commit.
    pub current: LockSurfaceState,
}

impl LockSurfaceAttributes {
    fn ack_configure(&mut self, serial: Serial) -> Option<LockSurfaceConfigure> {
        let configure = self
            .pending_configures
            .iter()
            .find(|configure| configure.serial == serial)
            .cloned()?;

        self.pending_configures
            .retain(|configure| configure.serial > serial);
        self.last_acked = Some(configure.state);
        self.configure_serial = Some(serial);

        Some(configure)
    }
}

/// Handle for a ext-session-lock surface.
#[derive(Clone, Debug)]
pub struct LockSurface {
    shell_surface: ExtSessionLockSurfaceV1,
    surface: WlSurface,
}

impl PartialEq for LockSurface {
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
    pub fn alive(&self) -> bool {
        self.surface.alive()
    }

    /// Get the current pending configure state.
    pub fn get_pending_state(&self, attributes: &mut LockSurfaceAttributes) -> Option<LockSurfaceState> {
        let server_pending = match attributes.server_pending.take() {
            Some(state) => state,
            None => return None,
        };

        // Get the last pending state.
        let pending = attributes.pending_configures.last();
        let last_state = pending.map(|c| &c.state).or(attributes.last_acked.as_ref());

        // Check if last state matches pending state.
        match last_state {
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
            let current = attributes.current;
            let server_pending = attributes.server_pending.get_or_insert(current);

            f(server_pending)
        })
    }

    /// Get the current pending state.
    #[allow(unused)]
    pub fn current_state(&self) -> LockSurfaceState {
        compositor::with_states(&self.surface, |states| {
            states
                .data_map
                .get::<Mutex<LockSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .current
        })
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
