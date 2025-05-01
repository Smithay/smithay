use std::sync::{Arc, Mutex};

use wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface, Dispatch, Resource};
use wl_input_method::input_method::xx::server::xx_input_method_v1::XxInputMethodV1;
use wl_input_method::input_method::xx::server::xx_input_popup_surface_v2::{self, XxInputPopupSurfaceV2};

use crate::utils::{
    alive_tracker::{AliveTracker, IsAlive},
    Logical, Point, Rectangle, Serial, SERIAL_COUNTER,
};

use super::{positioner::PositionerState, InputMethodHandler, InputMethodManagerState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImPopupLocation {
    /// Area for the positioner, relative to parent
    pub anchor: Rectangle<i32, Logical>,
    /// Geometry of the popup surface relative to parent.
    pub geometry: Rectangle<i32, Logical>,
}

/// A handle to an input method popup surface
#[derive(Debug, Clone)]
pub struct PopupSurface {
    /// Input method controlling this popup
    input_method: XxInputMethodV1,
    /// The surface role for the input method popup
    pub surface_role: XxInputPopupSurfaceV2,
    /// Surface containing the popup
    surface: WlSurface,
    /// Surface containing the text input. This surface doesn't change within the lifetime of the popup.
    parent: PopupParent,
    /// Computes the position of the popup according to provided rules
    pub positioner: PositionerState,
    /// Tracks configures and serials
    configure_tracker: Arc<Mutex<ConfigureTracker<PopupSurfaceState>>>,
    /// The compositor-assigned state acknowledged by client.
    state: Arc<Mutex<PopupSurfaceState>>,
    /// The compositor-assigned state, not sent to client yet
    state_pending: Option<PopupSurfaceState>,
}

impl PopupSurface {
    /// Creates a new popup surface.
    /// Anchor is the anchor position relative to parent. Geometry is the popup position relative to parent.
    pub(crate) fn new(
        input_method: XxInputMethodV1,
        surface_role: XxInputPopupSurfaceV2,
        surface: WlSurface,
        parent: PopupParent,
        anchor: Rectangle<i32, Logical>,
        geometry: Rectangle<i32, Logical>,
        positioner: PositionerState,
        configure_tracker: Arc<Mutex<ConfigureTracker<PopupSurfaceState>>>,
        state: Arc<Mutex<PopupSurfaceState>>,
    ) -> Self {
        Self {
            input_method,
            surface_role,
            configure_tracker,
            state,
            state_pending: Some(PopupSurfaceState {
                position: ImPopupLocation { anchor, geometry },
                configured: false,
                repositioned: false,
            }),
            surface,
            parent,
            positioner,
        }
    }

    /// Is the input method popup surface referred by this handle still alive?
    #[inline]
    pub fn alive(&self) -> bool {
        // TODO other things to check? This may not sufice.
        let role_data: &InputMethodPopupSurfaceUserData = self.surface_role.data().unwrap();
        self.surface.alive() && role_data.alive_tracker.alive()
    }

    /// Access to the underlying `wl_surface` of this popup
    #[inline]
    pub fn wl_surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Access to the parent surface associated with this popup
    pub fn get_parent(&self) -> &PopupParent {
        &self.parent
    }

    /// Access the input method using this popup
    pub fn input_method(&self) -> &XxInputMethodV1 {
        &self.input_method
    }

    /// Used to access the location of an input popup surface relative to the parent
    pub fn location(&self) -> Point<i32, Logical> {
        self.state.lock().unwrap().position.geometry.loc
    }

    /// Set position information that should take effect when mapping.
    /// Updates pending state.
    pub fn set_position(&mut self, position: ImPopupLocation) {
        let pending = &mut self.state_pending;
        if pending.is_none() {
            *pending = Some(self.state.lock().unwrap().clone());
        }
        pending.as_mut().unwrap().position = position;
    }

    /// `true` if the surface sent a
    /// configure sequence since creating the popup object.
    ///
    /// Calls [`compositor::with_states`] internally.
    pub fn is_initial_configure_sent(&self) -> bool {
        self.state.lock().unwrap().configured
    }

    /// Send a configure event to this popup surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    pub fn send_pending_configure(&mut self) {
        let new_state = {
            let state = &mut self.state_pending;
            if let Some(state) = state.as_mut() {
                // FIXME: store configured on input_method.done
                state.configured = true;
                state.clone()
            } else {
                // there's nothing to update
                return;
            }
        };

        // TODO: there's too much locking here but too early to optimize...
        let sent_state = {
            let tracker = self.configure_tracker.lock().unwrap();
            tracker.last_sent_state().cloned()
        }
        .unwrap_or_else(|| self.state.lock().unwrap().clone());

        // start_configure should be sent on any server-side change. Other events should follow with more granularity.
        if new_state != sent_state {
            let mut tracker = self.configure_tracker.lock().unwrap();
            let serial = tracker.assign_serial(new_state.clone());

            let ImPopupLocation { anchor, geometry } = new_state.position.clone();
            let relative_to_popup = anchor.loc - geometry.loc;
            self.surface_role.start_configure(
                geometry.size.w as u32,
                geometry.size.h as u32,
                relative_to_popup.x,
                relative_to_popup.y,
                anchor.size.w as u32,
                anchor.size.h as u32,
                serial.into(),
            );
        }
    }
}

impl std::cmp::PartialEq for PopupSurface {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.surface_role == other.surface_role
    }
}

/// Compositor-defined state
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupSurfaceState {
    /// Positioning information
    position: ImPopupLocation,
    repositioned: bool,
    /// Already issued a configure sequence
    configured: bool,
}

impl PopupSurfaceState {
    /// Creates an initial state with uninitialized values. The values are never read in normal protocol usage.
    pub fn new_uninit() -> Self {
        PopupSurfaceState {
            position: ImPopupLocation {
                anchor: Default::default(),
                geometry: Default::default(),
            },
            configured: false,
            repositioned: false,
        }
    }
}

/// Parent surface and location for the IME popup.
#[derive(Debug, Clone)]
pub struct PopupParent {
    /// The surface over which the IME popup is shown.
    pub surface: WlSurface,
    /// The location of the parent surface relative to TODO.
    pub location: Rectangle<i32, Logical>,
}

/// User data of XxInputPopupSurfaceV2 object
#[derive(Debug)]
pub struct InputMethodPopupSurfaceUserData {
    pub(super) alive_tracker: AliveTracker,
    pub(super) surface: WlSurface,
    pub(super) configure_tracker: Arc<Mutex<ConfigureTracker<PopupSurfaceState>>>,
    /// State acknowledged by client.
    pub(super) state: Arc<Mutex<PopupSurfaceState>>,
}

/// Tracks states updated via configure sequences involving serials
#[derive(Debug)]
pub struct ConfigureTracker<State> {
    /// The serial of the last acked configure
    configure_serial: Option<Serial>,

    /// An ordered sequence of the configures the server has sent out to the client waiting to be
    /// acknowledged by the client. All pending configures that are older than
    /// the acknowledged one will be discarded during processing
    /// layer_surface.ack_configure.
    /// The newest configure has the highest index.
    pending_configures: Vec<(State, Serial)>,

    /// Holds the last server_pending state that has been acknowledged by the
    /// client. This state should be cloned to the current during a commit.
    last_acked: Option<State>,
}

impl<State> Default for ConfigureTracker<State> {
    fn default() -> Self {
        Self {
            configure_serial: None,
            pending_configures: Vec::new(),
            last_acked: None,
        }
    }
}

impl<State: Clone> ConfigureTracker<State> {
    /// Assigns a new pending state and returns its serial
    fn assign_serial(&mut self, state: State) -> Serial {
        let serial = SERIAL_COUNTER.next_serial();
        self.pending_configures.push((state, serial));
        serial
    }

    /// Marks that the user accepted the serial and returns the state associated with it.
    /// If the serial is not currently pending, returns None.
    fn ack_serial(&mut self, serial: Serial) -> Option<State> {
        let (state, _) = self
            .pending_configures
            .iter()
            .find(|(_, c_serial)| *c_serial == serial)
            .cloned()?;

        self.pending_configures.retain(|(_, c_serial)| *c_serial > serial);
        self.last_acked = Some(state.clone());
        Some(state)
    }

    /// Last pending state
    fn last_sent_state(&self) -> Option<&State> {
        self.pending_configures.last().map(|(s, _)| s)
    }
}

impl<D> Dispatch<XxInputPopupSurfaceV2, InputMethodPopupSurfaceUserData, D> for InputMethodManagerState
where
    D: InputMethodHandler,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        popup: &XxInputPopupSurfaceV2,
        request: xx_input_popup_surface_v2::Request,
        data: &InputMethodPopupSurfaceUserData,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        use xx_input_popup_surface_v2::Request;
        match request {
            Request::AckConfigure { serial } => {
                let surface = &data.surface;

                let serial = Serial::from(serial);
                let client_state = data.configure_tracker.lock().unwrap().ack_serial(serial);

                let client_state = match client_state {
                    Some(state) => state,
                    None => {
                        popup.post_error(
                            xx_input_popup_surface_v2::Error::InvalidSerial,
                            format!("Serial {} is not awaiting ack", <u32>::from(serial)),
                        );
                        return;
                    }
                };
                *data.state.lock().unwrap() = client_state.clone();
                state.popup_ack_configure(surface, serial, client_state);
            }
            Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        _object: &XxInputPopupSurfaceV2,
        data: &InputMethodPopupSurfaceUserData,
    ) {
        data.alive_tracker.destroy_notify();
    }
}
