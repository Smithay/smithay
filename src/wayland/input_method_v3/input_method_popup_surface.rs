use std::cmp::PartialEq;
use std::sync::{Arc, Mutex};

use wayland_protocols_experimental::input_method::v1::server::xx_input_method_v1::XxInputMethodV1;
use wayland_protocols_experimental::input_method::v1::server::xx_input_popup_surface_v2::{
    self, XxInputPopupSurfaceV2,
};
use wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface, Dispatch, Resource};

use crate::input::SeatHandler;
use crate::utils::{
    alive_tracker::{AliveTracker, IsAlive},
    Logical, Point, Rectangle, Serial,
};

use super::{
    configure_tracker::ConfigureTracker,
    positioner::{PositionerState, PositionerUserData},
    InputMethodHandler, InputMethodManagerState, InputMethodUserData,
};

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
    /// The surface role for the input method popup
    pub surface_role: XxInputPopupSurfaceV2,
    /// Surface containing the popup
    surface: WlSurface,
    /// Surface containing the text input. This surface doesn't change within the lifetime of the popup.
    parent: PopupParent,
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
        init: impl FnOnce(InputMethodPopupSurfaceUserData) -> XxInputPopupSurfaceV2,
        input_method: XxInputMethodV1,
        parent: PopupParent,
        surface: WlSurface,
        anchor: Rectangle<i32, Logical>,
        geometry: Rectangle<i32, Logical>,
        positioner_data: PositionerState,
    ) -> Self {
        let configure_tracker = Arc::new(Mutex::new(Default::default()));
        let state = Arc::new(Mutex::new(PopupSurfaceState::new_uninit()));

        let instance = InputMethodPopupSurfaceUserData::new(
            input_method.clone(),
            surface.clone(),
            configure_tracker.clone(),
            state.clone(),
            Mutex::new(positioner_data),
        );
        let surface_role = init(instance);
        Self {
            surface_role,
            configure_tracker,
            state,
            state_pending: Some(PopupSurfaceState {
                position: ImPopupLocation { anchor, geometry },
                configured: false,
                repositioned: None,
            }),
            surface,
            parent,
        }
    }

    /// Returns a copy of the positioner. That can be used to calculate a new position.
    pub fn positioner(&self) -> PositionerState {
        let role_data: &InputMethodPopupSurfaceUserData = self.surface_role.data().unwrap();
        *role_data.positioner.lock().unwrap()
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
        let role_data: &InputMethodPopupSurfaceUserData = self.surface_role.data().unwrap();
        &role_data.input_method
    }

    /// Used to access the location of an input popup surface relative to the parent
    pub fn location(&self) -> Point<i32, Logical> {
        self.state.lock().unwrap().position.geometry.loc
    }

    /// `true` if the surface sent a
    /// configure sequence since creating the popup object.
    pub fn is_initial_configure_sent(&self) -> bool {
        self.state.lock().unwrap().configured
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

    /// Adds the repositioned token to pending state.
    pub fn set_repositioned(&mut self, token: u32) {
        let pending = &mut self.state_pending;
        if pending.is_none() {
            *pending = Some(self.state.lock().unwrap().clone());
        }
        pending.as_mut().unwrap().repositioned = Some(token);
    }

    /// Send a configure event to this popup surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    /// Call this from input_method.done
    pub fn send_pending_configure(&mut self) {
        let new_state = {
            let state = &mut self.state_pending;
            if let Some(state) = state.as_mut() {
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
            tracker.last_pending_state().cloned()
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

            if let (Some(new), sent) = (new_state.repositioned, sent_state.repositioned) {
                if Some(new) != sent {
                    self.surface_role.repositioned(new);
                }
            }
        }
    }
}

impl PartialEq for PopupSurface {
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
    /// Token to send to the client, if any
    ///
    /// The protocol doesn't mandate the lifecycle for this token, so this holds the last state and update events are sent on detected changes.
    repositioned: Option<u32>,
    /// Already issued a configure sequence
    configured: bool,
}

impl PopupSurfaceState {
    /// Creates an initial state with uninitialized values. The values are never read in normal protocol usage.
    fn new_uninit() -> Self {
        PopupSurfaceState {
            position: ImPopupLocation {
                anchor: Default::default(),
                geometry: Default::default(),
            },
            configured: false,
            repositioned: None,
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

/// Data accessible from XxInputPopupSurfaceV2 object
#[derive(Debug)]
pub struct InputMethodPopupSurfaceUserData {
    /// Input method controlling this popup
    input_method: XxInputMethodV1,
    pub(super) alive_tracker: AliveTracker,
    pub(super) surface: WlSurface,
    pub(super) configure_tracker: Arc<Mutex<ConfigureTracker<PopupSurfaceState>>>,
    /// State acknowledged by client.
    pub(super) state: Arc<Mutex<PopupSurfaceState>>,
    // State supplied by client.
    /// Computes the position of the popup according to provided rules
    pub(super) positioner: Mutex<PositionerState>,
}

impl InputMethodPopupSurfaceUserData {
    fn new(
        input_method: XxInputMethodV1,
        surface: WlSurface,
        configure_tracker: Arc<Mutex<ConfigureTracker<PopupSurfaceState>>>,
        popup_state: Arc<Mutex<PopupSurfaceState>>,
        positioner: Mutex<PositionerState>,
    ) -> Self {
        Self {
            input_method,
            alive_tracker: AliveTracker::default(),
            surface,
            configure_tracker,
            state: popup_state,
            positioner,
        }
    }
}

impl<D> Dispatch<XxInputPopupSurfaceV2, InputMethodPopupSurfaceUserData, D> for InputMethodManagerState
where
    D: InputMethodHandler + SeatHandler,
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
            Request::Reposition { positioner, token } => {
                let im: &InputMethodUserData<D> = data.input_method.data().unwrap();
                let popup = {
                    let positioner: &PositionerUserData = positioner.data().unwrap();
                    let positioner = *positioner.inner.lock().unwrap();
                    let mut inner = im.handle.inner.lock().unwrap();
                    // This request comes to an input_method object, so an empty instance is a bug.
                    let instance = inner.instance.as_mut().unwrap();
                    let cursor = instance.cursor_rectangle;
                    let popup = instance
                        .popup_handles
                        .iter_mut()
                        .find(|h| h.surface_role == *popup)
                        .expect("This popup not tracked by its input method");
                    let parent_surface = popup.get_parent().surface.clone();
                    // This locks input method instance. The geometry callback is going to be limited here. The lock can be released and reacquired for .set_position, but it's less readable, so better do it when the need comes.
                    let popup_geometry = state.popup_geometry(&parent_surface, &cursor, &positioner);
                    *data.positioner.lock().unwrap() = positioner;

                    popup.set_repositioned(token);
                    popup.set_position(ImPopupLocation {
                        anchor: cursor,
                        geometry: popup_geometry,
                    });
                    popup.clone()
                };

                state.popup_repositioned(popup);

                im.handle.done();
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
