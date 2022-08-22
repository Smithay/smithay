use slog::{error, trace, warn};
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::{
        wl_keyboard::{self, KeyState as WlKeyState, KeymapFormat, WlKeyboard},
        wl_surface::WlSurface,
    },
    Dispatch, DisplayHandle, Resource,
};

use crate::{
    backend::input::KeyState,
    input::{
        keyboard::{KeyboardHandle, KeyboardTarget, KeysymHandle, ModifiersState},
        Seat, SeatHandler, SeatState,
    },
    utils::IsAlive,
    utils::Serial,
};

impl<D: SeatHandler + 'static> KeyboardHandle<D> {
    /// Check if client of given resource currently has keyboard focus
    pub fn client_of_object_has_focus(&self, id: &ObjectId) -> bool {
        self.arc
            .internal
            .lock()
            .unwrap()
            .focus
            .as_ref()
            .and_then(|f| f.0.as_any().downcast_ref::<WlSurface>())
            .map(|s| s.id().same_client_as(id))
            .unwrap_or(false)
    }

    /// Register a new keyboard to this handler
    ///
    /// The keymap will automatically be sent to it
    ///
    /// This should be done first, before anything else is done with this keyboard.
    pub(crate) fn new_kbd(&self, kbd: WlKeyboard) {
        trace!(self.arc.logger, "Sending keymap to client");

        // prepare a tempfile with the keymap, to send it to the client
        let ret = self.arc.keymap.with_fd(kbd.version() >= 7, |fd, size| {
            kbd.keymap(KeymapFormat::XkbV1, fd, size as u32);
        });

        if let Err(e) = ret {
            warn!(self.arc.logger,
                "Failed write keymap to client in a tempfile";
                "err" => format!("{:?}", e)
            );
            return;
        };

        let guard = self.arc.internal.lock().unwrap();
        if kbd.version() >= 4 {
            kbd.repeat_info(guard.repeat_rate, guard.repeat_delay);
        }
        if let Some((focused, serial)) = guard.focus.as_ref() {
            if let Some(surface) = focused.as_any().downcast_ref::<WlSurface>() {
                if surface.id().same_client_as(&kbd.id()) {
                    let serialized = guard.mods_state.serialized;
                    let keys = serialize_pressed_keys(guard.pressed_keys.clone());
                    kbd.enter((*serial).into(), surface, keys);
                    // Modifiers must be send after enter event.
                    kbd.modifiers(
                        (*serial).into(),
                        serialized.depressed,
                        serialized.latched,
                        serialized.locked,
                        serialized.layout_locked,
                    );
                }
            }
        }
        self.arc.known_kbds.lock().unwrap().push(kbd);
    }
}

/// User data for keyboard
#[derive(Debug)]
pub struct KeyboardUserData<D> {
    pub(crate) handle: Option<KeyboardHandle<D>>,
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

    fn destroyed(_state: &mut D, _client_id: ClientId, object_id: ObjectId, data: &KeyboardUserData<D>) {
        if let Some(ref handle) = data.handle {
            handle
                .arc
                .known_kbds
                .lock()
                .unwrap()
                .retain(|k| k.id() != object_id)
        }
    }
}

fn with_focused_kbds<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(WlKeyboard),
) {
    if let Some(keyboard) = seat.get_keyboard() {
        let inner = keyboard.arc.known_kbds.lock().unwrap();
        for kbd in &*inner {
            if kbd.id().same_client_as(&surface.id()) {
                f(kbd.clone())
            }
        }
    }
}

fn serialize_pressed_keys(keys: Vec<u32>) -> Vec<u8> {
    let serialized = unsafe { ::std::slice::from_raw_parts(keys.as_ptr() as *const u8, keys.len() * 4) };
    serialized.into()
}

impl<D: SeatHandler + 'static> KeyboardTarget<D> for WlSurface {
    fn enter(&self, seat: &Seat<D>, _data: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        with_focused_kbds(seat, self, |kbd| {
            kbd.enter(
                serial.into(),
                self,
                serialize_pressed_keys(keys.iter().map(|h| h.raw_code() - 8).collect()),
            )
        })
    }

    fn leave(&self, seat: &Seat<D>, _data: &mut D, serial: Serial) {
        with_focused_kbds(seat, self, |kbd| kbd.leave(serial.into(), self))
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
        with_focused_kbds(seat, self, |kbd| {
            kbd.key(serial.into(), time, key.raw_code() - 8, state.into())
        })
    }

    fn modifiers(&self, seat: &Seat<D>, _data: &mut D, modifiers: ModifiersState, serial: Serial) {
        with_focused_kbds(seat, self, |kbd| {
            let modifiers = modifiers.serialized;
            kbd.modifiers(
                serial.into(),
                modifiers.depressed,
                modifiers.latched,
                modifiers.locked,
                modifiers.layout_locked,
            );
        })
    }

    fn is_alive(&self) -> bool {
        IsAlive::alive(self)
    }
    fn same_handler_as(&self, other: &dyn KeyboardTarget<D>) -> bool {
        if let Some(other_surface) = other.as_any().downcast_ref::<WlSurface>() {
            self == other_surface
        } else {
            false
        }
    }
    fn clone_handler(&self) -> Box<dyn KeyboardTarget<D>> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl From<KeyState> for WlKeyState {
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
    fn try_from(state: WlKeyState) -> Result<Self, Self::Error> {
        match state {
            WlKeyState::Pressed => Ok(KeyState::Pressed),
            WlKeyState::Released => Ok(KeyState::Released),
            x => Err(UnknownKeyState(x)),
        }
    }
}