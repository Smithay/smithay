//! Implementation of the `zwp_virtual_keyboard_v1` protocol.
//!
//! A virtual keyboard is exposed as a regular input device instead of being
//! wired straight to the seat. Requests are converted into
//! [`VirtualKeyboardBackend`] [`InputEvent`]s and passed to
//! [`VirtualKeyboardHandler::process_virtual_keyboard_event`] so synthetic
//! input is handled the same way as a real one (making compositor bindings,
//! idle handling, and focus all behave properly).
//!
//! The device shows up (as [`InputEvent::DeviceAdded`]) only after the initial
//! keymap is sent since the keycodes don't mean anything without one. The
//! device is removed when the client destroys the keyboard, after releasing any
//! held keys (see [`InputEvent::DeviceRemoved`]).
//!
//! Since the keycodes belong to the client's keymap, not the seat, it must be
//! activated (see [`VirtualKeyboardDevice::keymap`]) before handling them. A
//! client may send the `no_keymap` format to use the existing seat keymap, in
//! which case it will be `None`.
//!
//! Requests with no [`InputEvent`] equivalent are [`InputEvent::Special`] (see
//! [`VirtualKeyboardSpecialEvent`]).
//!
//! ```
//! use smithay::backend::input::InputEvent;
//! use smithay::wayland::virtual_keyboard::{
//!     VirtualKeyboardManagerState, VirtualKeyboardBackend, VirtualKeyboardHandler,
//! };
//! # use smithay::reexports::wayland_server::Display;
//!
//! # struct State;
//!
//! smithay::delegate_dispatch2!(State);
//!
//! # let mut display = Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! impl VirtualKeyboardHandler for State {
//!     fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
//!         // process_input_event(event);
//!     }
//! }
//!
//! // The function decides which clients may create keyboards.
//! VirtualKeyboardManagerState::new::<State, _>(&display_handle, |_client| true);
//! ```

use std::os::unix::io::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::warn;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::{
    zwp_virtual_keyboard_manager_v1::{self, ZwpVirtualKeyboardManagerV1},
    zwp_virtual_keyboard_v1::{self, ZwpVirtualKeyboardV1},
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
    backend::GlobalId, protocol::wl_keyboard::KeymapFormat, protocol::wl_seat::WlSeat,
};
use xkbcommon::xkb;

use crate::backend::input::{InputEvent, InputTime, KeyState};
use crate::wayland::{Dispatch2, GlobalData, GlobalDispatch2};

const MANAGER_VERSION: u32 = 1;

mod input;

pub use input::{
    VirtualKeyboardBackend, VirtualKeyboardDevice, VirtualKeyboardKeyEvent, VirtualKeyboardSpecialEvent,
};

/// Handler for virtual keyboard input events.
pub trait VirtualKeyboardHandler {
    /// Handle an event from a virtual keyboard.
    ///
    /// Process it the same way as any other
    /// [`InputBackend`](crate::backend::input::InputBackend).
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>);
}

/// State of wp misc virtual keyboard protocol
#[derive(Debug)]
pub struct VirtualKeyboardManagerState {
    global: GlobalId,
}

/// Data associated with a VirtualKeyboardManager global.
#[allow(missing_debug_implementations)]
pub struct VirtualKeyboardManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl VirtualKeyboardManagerState {
    /// Initialize a virtual keyboard manager global.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwpVirtualKeyboardManagerV1, VirtualKeyboardManagerGlobalData>,
        D: Dispatch<ZwpVirtualKeyboardManagerV1, GlobalData>,
        D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData>,
        D: VirtualKeyboardHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = VirtualKeyboardManagerGlobalData {
            filter: Box::new(filter),
        };
        let global = display.create_global::<D, ZwpVirtualKeyboardManagerV1, _>(MANAGER_VERSION, data);

        Self { global }
    }

    /// Get the id of ZwpVirtualKeyboardManagerV1 global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch2<ZwpVirtualKeyboardManagerV1, D> for VirtualKeyboardManagerGlobalData
where
    D: Dispatch<ZwpVirtualKeyboardManagerV1, GlobalData>,
    D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData>,
    D: VirtualKeyboardHandler,
    D: 'static,
{
    fn bind(
        &self,
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpVirtualKeyboardManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<ZwpVirtualKeyboardManagerV1, D> for GlobalData
where
    D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData>,
    D: VirtualKeyboardHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        _resource: &ZwpVirtualKeyboardManagerV1,
        request: zwp_virtual_keyboard_manager_v1::Request,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_manager_v1::Request::CreateVirtualKeyboard { seat, id } => {
                data_init.init(
                    id,
                    VirtualKeyboardUserData {
                        data: Arc::new(VirtualKeyboardData {
                            seat,
                            has_keymap: AtomicBool::new(false),
                            keymap: Mutex::new(None),
                            pressed_keys: Mutex::new(Vec::new()),
                            modifiers_set: AtomicBool::new(false),
                        }),
                    },
                );
            }
            _ => unreachable!(),
        }
    }
}

/// User data for ZwpVirtualKeyboardV1
#[derive(Debug)]
pub struct VirtualKeyboardUserData {
    data: Arc<VirtualKeyboardData>,
}

#[derive(Debug)]
struct VirtualKeyboardData {
    seat: WlSeat,
    /// Whether a keymap request was accepted. Even if set, there may still be
    /// no keymap if `no_keymap` was explicitly requested.
    has_keymap: AtomicBool,
    keymap: Mutex<Option<Arc<str>>>,
    pressed_keys: Mutex<Vec<u32>>,
    modifiers_set: AtomicBool,
}

impl VirtualKeyboardUserData {
    fn device(&self, keyboard: &ZwpVirtualKeyboardV1) -> VirtualKeyboardDevice {
        VirtualKeyboardDevice {
            keyboard: keyboard.clone(),
            data: self.data.clone(),
        }
    }
}

impl<D> Dispatch2<ZwpVirtualKeyboardV1, D> for VirtualKeyboardUserData
where
    D: VirtualKeyboardHandler + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        virtual_keyboard: &ZwpVirtualKeyboardV1,
        request: zwp_virtual_keyboard_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_v1::Request::Keymap { format, fd, size } => {
                let keymap = if format == KeymapFormat::NoKeymap as u32 {
                    // Use the current seat keymap to process keycodes.
                    None
                } else if format == KeymapFormat::XkbV1 as u32 {
                    match compile_keymap(fd, size as usize) {
                        Some(keymap) => Some(Arc::from(keymap)),
                        // Client will get a `no_keymap` error on the next `key`
                        // request if no other keymap requests have succeeded
                        // yet.
                        None => return,
                    }
                } else {
                    // TODO: uncomment this once zwp_virtual_keyboard_v1 is updated
                    // virtual_keyboard.post_error(
                    //     zwp_virtual_keyboard_v1::Error::InvalidKeymapFormat,
                    //     "invalid keymap format",
                    // );
                    warn!("Client sent an unsupported keymap format: {format}");
                    return;
                };

                *self.data.keymap.lock().unwrap() = keymap;
                let added = !self.data.has_keymap.swap(true, Ordering::Relaxed);

                let device = self.device(virtual_keyboard);
                if added {
                    state.process_virtual_keyboard_event(InputEvent::DeviceAdded { device });
                } else {
                    state.process_virtual_keyboard_event(InputEvent::Special(
                        VirtualKeyboardSpecialEvent::KeymapChanged { device },
                    ));
                }
            }
            zwp_virtual_keyboard_v1::Request::Key {
                time,
                key,
                state: key_state,
            } => {
                if !self.data.has_keymap.load(Ordering::Relaxed) {
                    virtual_keyboard.post_error(
                        zwp_virtual_keyboard_v1::Error::NoKeymap,
                        "`key` sent before keymap.",
                    );
                    return;
                }

                // In wlroots, this is passed through if it isn't 0 or 1, but
                // smithay key states are enums, so we need to choose pressed or
                // released (repeated isn't available until wl_keyboard v10).
                let key_state = match key_state {
                    0 => KeyState::Released,
                    // Repeated is sent zero or more times after Pressed and
                    // before Released.
                    // 2 => KeyState::Repeated,
                    _ => KeyState::Pressed,
                };

                // Tracked so `destroyed` can release held keys.
                {
                    let mut pressed = self.data.pressed_keys.lock().unwrap();
                    match key_state {
                        KeyState::Pressed => {
                            if !pressed.contains(&key) {
                                pressed.push(key);
                            }
                        }
                        KeyState::Released => pressed.retain(|pressed| *pressed != key),
                    }
                }

                state.process_virtual_keyboard_event(InputEvent::Keyboard {
                    event: VirtualKeyboardKeyEvent {
                        device: self.device(virtual_keyboard),
                        time,
                        key,
                        state: key_state,
                    },
                });
            }
            zwp_virtual_keyboard_v1::Request::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                if !self.data.has_keymap.load(Ordering::Relaxed) {
                    virtual_keyboard.post_error(
                        zwp_virtual_keyboard_v1::Error::NoKeymap,
                        "`modifiers` sent before keymap.",
                    );
                    return;
                }

                // Tracked so `destroyed` can release set modifiers.
                let any = (mods_depressed | mods_latched | mods_locked | group) != 0;
                self.data.modifiers_set.store(any, Ordering::Relaxed);

                state.process_virtual_keyboard_event(InputEvent::Special(
                    VirtualKeyboardSpecialEvent::Modifiers {
                        device: self.device(virtual_keyboard),
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        group,
                    },
                ));
            }
            zwp_virtual_keyboard_v1::Request::Destroy => {
                // no-op; the device is removed in `destroyed`
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, virtual_keyboard: &ZwpVirtualKeyboardV1) {
        // The device only exists once a keymap request was accepted.
        if !self.data.has_keymap.load(Ordering::Relaxed) {
            return;
        }

        let device = self.device(virtual_keyboard);

        // A client may destroy the keyboard, or die, with keys still down.
        // Release them here like how libinput does with an unplugged keyboard.
        let pressed = std::mem::take(&mut *self.data.pressed_keys.lock().unwrap());
        let time = InputTime::now().millis();
        for key in pressed.into_iter().rev() {
            state.process_virtual_keyboard_event(InputEvent::Keyboard {
                event: VirtualKeyboardKeyEvent {
                    device: device.clone(),
                    time,
                    key,
                    state: KeyState::Released,
                },
            });
        }

        // Also clear set modifiers.
        if self.data.modifiers_set.swap(false, Ordering::Relaxed) {
            state.process_virtual_keyboard_event(InputEvent::Special(
                VirtualKeyboardSpecialEvent::Modifiers {
                    device: device.clone(),
                    mods_depressed: 0,
                    mods_latched: 0,
                    mods_locked: 0,
                    group: 0,
                },
            ));
        }

        state.process_virtual_keyboard_event(InputEvent::DeviceRemoved { device });
    }
}

/// Compile a client-supplied `xkb_v1` keymap, normalizing it to
/// `XKB_KEYMAP_FORMAT_TEXT_V1`, and returning it if valid.
fn compile_keymap(fd: OwnedFd, size: usize) -> Option<String> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    // SAFETY: we can map the keymap into the memory.
    let keymap = match unsafe {
        xkb::Keymap::new_from_fd(
            &context,
            fd,
            size,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
    } {
        Ok(Some(keymap)) => keymap,
        Ok(None) => {
            warn!("Failed to compile virtual keyboard keymap");
            return None;
        }
        Err(err) => {
            warn!("Failed to map virtual keyboard keymap: {err:?}");
            return None;
        }
    };

    Some(keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
}
