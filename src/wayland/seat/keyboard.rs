use std::fmt;

use tracing::{error, instrument, trace, warn};
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::{
        wl_keyboard::{self, KeyState as WlKeyState, WlKeyboard},
        wl_surface::WlSurface,
    },
    Client, Dispatch, DisplayHandle, Resource,
};

use super::WaylandFocus;
use crate::{
    backend::input::{KeyState, Keycode},
    input::{
        keyboard::{KeyboardHandle, KeyboardTarget, KeysymHandle, ModifiersState},
        Seat, SeatHandler, SeatState,
    },
    utils::{iter::new_locked_obj_iter_from_vec, Serial},
    wayland::{input_method::InputMethodSeat, text_input::TextInputSeat},
};

impl<D> KeyboardHandle<D>
where
    D: SeatHandler + 'static,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    /// Check if client of given resource currently has keyboard focus
    pub fn client_of_object_has_focus(&self, id: &ObjectId) -> bool {
        self.arc
            .internal
            .lock()
            .unwrap()
            .focus
            .as_ref()
            .map(|f| f.0.same_client_as(id))
            .unwrap_or(false)
    }

    /// Return all raw [`WlKeyboard`] instances for a particular [`Client`]
    pub fn client_keyboards<'a>(&'a self, client: &Client) -> impl Iterator<Item = WlKeyboard> + 'a {
        let guard = self.arc.known_kbds.lock().unwrap();

        new_locked_obj_iter_from_vec(guard, client.id())
    }

    /// Register a new keyboard to this handler
    ///
    /// The keymap will automatically be sent to it
    ///
    /// This should be done first, before anything else is done with this keyboard.
    #[instrument(parent = &self.arc.span, skip(self))]
    pub(crate) fn new_kbd(&self, kbd: WlKeyboard) {
        trace!("Sending keymap to client");

        // prepare a tempfile with the keymap, to send it to the client
        let keymap_file = self.arc.keymap.lock().unwrap();
        let ret = keymap_file.send(&kbd);

        if let Err(e) = ret {
            warn!(
                err = ?e,
                "Failed write keymap to client in a tempfile"
            );
            return;
        };

        let guard = self.arc.internal.lock().unwrap();
        if kbd.version() >= 4 {
            kbd.repeat_info(guard.repeat_rate, guard.repeat_delay);
        }
        if let Some((focused, serial)) = guard.focus.as_ref() {
            if focused.same_client_as(&kbd.id()) {
                let serialized = guard.mods_state.serialized;
                let keys = serialize_pressed_keys(guard.pressed_keys.iter().copied());
                kbd.enter((*serial).into(), &focused.wl_surface().unwrap(), keys);
                // Modifiers must be send after enter event.
                kbd.modifiers(
                    (*serial).into(),
                    serialized.depressed,
                    serialized.latched,
                    serialized.locked,
                    serialized.layout_effective,
                );
            }
        }
        self.arc.known_kbds.lock().unwrap().push(kbd.downgrade());
    }
}

impl<D: SeatHandler + 'static> KeyboardHandle<D> {
    /// Attempt to retrieve a [`KeyboardHandle`] from an existing resource
    ///
    /// May return `None` for a valid `WlKeyboard` that was created without
    /// the keyboard capability.
    pub fn from_resource(seat: &WlKeyboard) -> Option<Self> {
        seat.data::<KeyboardUserData<D>>()?.handle.clone()
    }
}

/// User data for keyboard
pub struct KeyboardUserData<D: SeatHandler> {
    pub(crate) handle: Option<KeyboardHandle<D>>,
}

impl<D: SeatHandler> fmt::Debug for KeyboardUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyboardUserData")
            .field("handle", &self.handle)
            .finish()
    }
}

impl<D> Dispatch<WlKeyboard, KeyboardUserData<D>, D> for SeatState<D>
where
    D: 'static + Dispatch<WlKeyboard, KeyboardUserData<D>>,
    D: SeatHandler,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlKeyboard,
        _request: wl_keyboard::Request,
        _data: &KeyboardUserData<D>,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client_id: ClientId, keyboard: &WlKeyboard, data: &KeyboardUserData<D>) {
        if let Some(ref handle) = data.handle {
            handle
                .arc
                .known_kbds
                .lock()
                .unwrap()
                .retain(|k| k.id() != keyboard.id())
        }
    }
}

pub(crate) fn for_each_focused_kbds<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(WlKeyboard),
) {
    if let Some(keyboard) = seat.get_keyboard() {
        let inner = keyboard.arc.known_kbds.lock().unwrap();
        for kbd in &*inner {
            let Ok(kbd) = kbd.upgrade() else {
                continue;
            };

            if kbd.id().same_client_as(&surface.id()) {
                f(kbd.clone())
            }
        }
    }
}

/// Serialize keycodes for the `WlKeyboard` interface
pub fn serialize_pressed_keys(keys: impl Iterator<Item = Keycode>) -> Vec<u8> {
    keys.flat_map(|key| (key.raw() - 8).to_ne_bytes()).collect()
}

impl<D: SeatHandler + 'static> KeyboardTarget<D> for WlSurface {
    fn enter(&self, seat: &Seat<D>, state: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        *seat.get_keyboard().unwrap().arc.last_enter.lock().unwrap() = Some(serial);
        for_each_focused_kbds(seat, self, |kbd| {
            kbd.enter(
                serial.into(),
                self,
                serialize_pressed_keys(keys.iter().map(|h| h.raw_code())),
            )
        });

        let text_input = seat.text_input();
        let input_method = seat.input_method();

        if input_method.has_instance() {
            input_method.deactivate_input_method(state);
        }

        // NOTE: Always set focus regardless whether the client actually has the
        // text-input global bound due to clients doing lazy global binding.
        text_input.set_focus(Some(self.clone()));

        // Only notify on `enter` once we have an actual IME.
        if input_method.has_instance() {
            text_input.enter();
        }
    }

    fn leave(&self, seat: &Seat<D>, state: &mut D, serial: Serial) {
        *seat.get_keyboard().unwrap().arc.last_enter.lock().unwrap() = None;
        for_each_focused_kbds(seat, self, |kbd| kbd.leave(serial.into(), self));
        let text_input = seat.text_input();
        let input_method = seat.input_method();

        if input_method.has_instance() {
            input_method.deactivate_input_method(state);
            text_input.leave();
        }

        text_input.set_focus(None);
    }

    fn key(
        &self,
        seat: &Seat<D>,
        _data: &mut D,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        for_each_focused_kbds(seat, self, |kbd| {
            kbd.key(serial.into(), time, key.raw_code().raw() - 8, state.into())
        })
    }

    fn modifiers(&self, seat: &Seat<D>, _data: &mut D, modifiers: ModifiersState, serial: Serial) {
        for_each_focused_kbds(seat, self, |kbd| {
            let modifiers = modifiers.serialized;
            kbd.modifiers(
                serial.into(),
                modifiers.depressed,
                modifiers.latched,
                modifiers.locked,
                modifiers.layout_effective,
            );
        })
    }
}

impl From<KeyState> for WlKeyState {
    #[inline]
    fn from(state: KeyState) -> WlKeyState {
        match state {
            KeyState::Pressed => WlKeyState::Pressed,
            KeyState::Released => WlKeyState::Released,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unknown KeyState {0:?}")]
pub struct UnknownKeyState(WlKeyState);

impl TryFrom<WlKeyState> for KeyState {
    type Error = UnknownKeyState;
    #[inline]
    fn try_from(state: WlKeyState) -> Result<Self, Self::Error> {
        match state {
            WlKeyState::Pressed => Ok(KeyState::Pressed),
            WlKeyState::Released => Ok(KeyState::Released),
            x => Err(UnknownKeyState(x)),
        }
    }
}
