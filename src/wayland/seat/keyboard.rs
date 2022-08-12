use slog::{error, trace, warn};
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::{
        wl_keyboard::{self, KeyState as WlKeyState, KeymapFormat, WlKeyboard},
        wl_surface::WlSurface,
    },
    Dispatch, DisplayHandle, Resource,
};
use xkbcommon::xkb;

use crate::{
    backend::input::KeyState,
    input::{
        keyboard::{KeyboardHandle, KeyboardHandler, KeysymHandle, ModifiersState},
        Seat, SeatHandler, SeatState,
    },
    utils::IsAlive,
    wayland::Serial,
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
                    let (dep, la, lo, gr) = serialize_modifiers(&guard.state);
                    let keys = serialize_pressed_keys(guard.pressed_keys.clone());
                    kbd.enter((*serial).into(), surface, keys);
                    // Modifiers must be send after enter event.
                    kbd.modifiers((*serial).into(), dep, la, lo, gr);
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

fn serialize_modifiers(state: &xkb::State) -> (u32, u32, u32, u32) {
    let mods_depressed = state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
    let mods_latched = state.serialize_mods(xkb::STATE_MODS_LATCHED);
    let mods_locked = state.serialize_mods(xkb::STATE_MODS_LOCKED);
    let layout_locked = state.serialize_layout(xkb::STATE_LAYOUT_LOCKED);

    (mods_depressed, mods_latched, mods_locked, layout_locked)
}

impl<D: SeatHandler + 'static> KeyboardHandler<D> for WlSurface {
    fn enter(&mut self, seat: &Seat<D>, _data: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        with_focused_kbds(seat, self, |kbd| {
            kbd.enter(
                serial.into(),
                self,
                serialize_pressed_keys(keys.iter().map(|h| h.raw_code() - 8).collect()),
            )
        })
    }

    fn leave(&mut self, seat: &Seat<D>, _data: &mut D, serial: Serial) {
        with_focused_kbds(seat, self, |kbd| kbd.leave(serial.into(), self))
    }

    fn key(
        &mut self,
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

    fn modifiers(
        &mut self,
        seat: &Seat<D>,
        _data: &mut D,
        state: &xkb::State,
        _modifiers: ModifiersState,
        serial: Serial,
    ) {
        with_focused_kbds(seat, self, |kbd| {
            let (de, la, lo, gr) = serialize_modifiers(state);
            kbd.modifiers(serial.into(), de, la, lo, gr)
        })
    }

    fn is_alive(&self) -> bool {
        IsAlive::alive(self)
    }
    fn same_handler_as(&self, other: &dyn KeyboardHandler<D>) -> bool {
        if let Some(other_surface) = other.as_any().downcast_ref::<WlSurface>() {
            self == other_surface
        } else {
            false
        }
    }
    fn clone_handler(&self) -> Box<dyn KeyboardHandler<D>> {
        Box::new(self.clone())
    }
    fn as_any<'a>(&'a self) -> &dyn std::any::Any {
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
