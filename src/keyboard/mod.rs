//! Utilities for keyboard handling
//!
//! This module provides utilities for keyboardand keymap handling: keymap interpretation
//! and forwarding keystrokes to clients using xkbcommon.
//!
//! You can first create a `KbdHandle` using the `create_keyboard_handler` function in this module.
//! The handle you obtained can be cloned to access this keyboard state from different places. It is
//! expected that such a context is created for each keyboard the compositor has access to.
//!
//! This handle gives you 3 main way to interact with the keymap handling:
//!
//! - send the keymap information to a client using the `KbdHandle::send_keymap` method.
//! - set the current focus for this keyboard: designing the client that will receive the key inputs
//!   using the `KbdHandle::set_focus` method.
//! - process key inputs from the input backend, allowing them to be catched at the compositor-level
//!   or forwarded to the client. See the documentation of the `KbdHandle::input` method for
//!   details.


use backend::input::KeyState;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};

use tempfile::tempfile;

use wayland_server::{Liveness, Resource};
use wayland_server::protocol::{wl_keyboard, wl_surface};

use xkbcommon::xkb;

pub use xkbcommon::xkb::{Keysym, keysyms};

/// Represents the current state of the keyboard modifiers
///
/// Each field of this struct represents a modifier and is `true` if this modifier is active.
///
/// For some modifiers, this means that the key is currently pressed, others are toggled
/// (like caps lock).
#[derive(Copy,Clone,Debug)]
pub struct ModifiersState {
    /// The "control" key
    pub ctrl: bool,
    /// The "alt" key
    pub alt: bool,
    /// The "shift" key
    pub shift: bool,
    /// The "Caps lock" key
    pub caps_lock: bool,
    /// The "logo" key
    ///
    /// Also known as the "windows" key on most keyboards
    pub logo: bool,
    /// The "Num lock" key
    pub num_lock: bool,
}

impl ModifiersState {
    fn new() -> ModifiersState {
        ModifiersState {
            ctrl: false,
            alt: false,
            shift: false,
            caps_lock: false,
            logo: false,
            num_lock: false,
        }
    }

    fn update_with(&mut self, state: &xkb::State) {
        self.ctrl = state.mod_name_is_active(&xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
        self.alt = state.mod_name_is_active(&xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE);
        self.shift = state.mod_name_is_active(&xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE);
        self.caps_lock = state.mod_name_is_active(&xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE);
        self.logo = state.mod_name_is_active(&xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE);
        self.num_lock = state.mod_name_is_active(&xkb::MOD_NAME_NUM, xkb::STATE_MODS_EFFECTIVE);
    }
}

struct KbdInternal {
    focus: Option<(wl_surface::WlSurface, wl_keyboard::WlKeyboard)>,
    pressed_keys: Vec<u32>,
    mods_state: ModifiersState,
    _context: xkb::Context,
    keymap: xkb::Keymap,
    state: xkb::State,
}

impl KbdInternal {
    fn new() -> Result<KbdInternal, ()> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // TODO: api to choose the keymap to load
        let keymap = xkb::Keymap::new_from_names(&context, &"", &"", &"fr", &"oss", None, 0)
            .ok_or(())?;
        let state = xkb::State::new(&keymap);
        Ok(KbdInternal {
               focus: None,
               pressed_keys: Vec::new(),
               mods_state: ModifiersState::new(),
               _context: context,
               keymap: keymap,
               state: state,
           })
    }

    // return true if modifier state has changed
    fn key_input(&mut self, keycode: u32, state: KeyState) -> bool {
        // track pressed keys as xkbcommon does not seem to expose it :(
        let direction = match state {
            KeyState::Pressed => {
                self.pressed_keys.push(keycode);
                xkb::KeyDirection::Down
            }
            KeyState::Released => {
                self.pressed_keys.retain(|&k| k != keycode);
                xkb::KeyDirection::Up
            }
        };

        // update state
        let state_components = self.state.update_key(keycode, direction);
        if state_components != 0 {
            self.mods_state.update_with(&self.state);
            true
        } else {
            false
        }
    }

    fn serialize_modifiers(&self) -> (u32, u32, u32, u32) {
        let mods_depressed = self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
        let mods_latched = self.state.serialize_mods(xkb::STATE_MODS_LATCHED);
        let mods_locked = self.state.serialize_mods(xkb::STATE_MODS_LOCKED);
        let layout_locked = self.state.serialize_layout(xkb::STATE_LAYOUT_LOCKED);

        return (mods_depressed, mods_latched, mods_locked, layout_locked);
    }

    fn serialize_pressed_keys(&self) -> Vec<u8> {
        let serialized = unsafe {
            ::std::slice::from_raw_parts(self.pressed_keys.as_ptr() as *const u8,
                                         self.pressed_keys.len() * 4)
        };
        serialized.into()
    }
}

pub fn create_keyboard_handler() -> Result<KbdHandle, ()> {
    let internal = KbdInternal::new()?;

    // prepare a tempfile with the keymap, to send it to clients
    // TODO: better error handling
    let mut keymap_file = tempfile().unwrap();
    let keymap_data = internal.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    keymap_file.write_all(keymap_data.as_bytes()).unwrap();
    keymap_file.flush().unwrap();

    Ok(KbdHandle {
           internal: Arc::new((Mutex::new(internal), (keymap_file, keymap_data.as_bytes().len() as u32))),
       })
}

/// An handle to a keyboard handler
///
/// It can be cloned and all clones manipulate the same internal state. Clones
/// can also be sent across threads.
///
/// See module-level documentation for details of its use.
#[derive(Clone)]
pub struct KbdHandle {
    internal: Arc<(Mutex<KbdInternal>, (::std::fs::File, u32))>,
}

impl KbdHandle {
    /// Handle a keystroke
    ///
    /// All keystrokes from the input backend should be fed _in order_ to this method of the
    /// keyboard handler. It will internally track the state of the keymap.
    ///
    /// The `filter` argument is expected to be a closure which will peek at the generated input
    /// as interpreted by the keymap befor it is forwarded to the focused client. If this closure
    /// returns false, the input will not be sent to the client. This mechanism can be used to
    /// implement compositor-level key bindings for example.
    ///
    /// The module `smithay::keyboard::keysyms` exposes definitions of all possible keysyms
    /// to be compared against. This includes non-characted keysyms, such as XF86 special keys.
    pub fn input<F>(&self, keycode: u32, state: KeyState, serial: u32, filter: F)
        where F: FnOnce(&ModifiersState, Keysym) -> bool
    {
        let mut guard = self.internal.0.lock().unwrap();
        let mods_changed = guard.key_input(keycode, state);

        if !filter(&guard.mods_state, guard.state.key_get_one_sym(keycode)) {
            // the filter returned false, we do not forward to client
            return;
        }

        // forward to client if no keybinding is triggered
        if let Some((_, ref kbd)) = guard.focus {
            if mods_changed {
                let (dep, la, lo, gr) = guard.serialize_modifiers();
                kbd.modifiers(serial, dep, la, lo, gr);
            }
            let wl_state = match state {
                KeyState::Pressed => wl_keyboard::KeyState::Pressed,
                KeyState::Released => wl_keyboard::KeyState::Released,
            };
            kbd.key(serial, 0, keycode, wl_state);
        }
    }

    /// Set the current focus of this keyboard
    ///
    /// Any previous focus will be sent a `wl_keyboard::leave` event, and if the new focus
    /// is not `None`, a `wl_keyboard::enter` event will be sent.
    pub fn set_focus(&self, focus: Option<(wl_surface::WlSurface, wl_keyboard::WlKeyboard)>, serial: u32) {
        // TODO: check surface and keyboard are from the same client

        let mut guard = self.internal.0.lock().unwrap();

        // remove current focus
        let old_kbd = if let Some((old_surface, old_kbd)) = guard.focus.take() {
            if old_surface.status() != Liveness::Dead {
                old_kbd.leave(serial, &old_surface);
            }
            Some(old_kbd)
        } else {
            None
        };

        // set new focus
        if let Some((surface, kbd)) = focus {
            if surface.status() != Liveness::Dead {
                // send new mods status if client instance changed
                match old_kbd {
                    Some(ref okbd) if okbd.equals(&kbd) => {}
                    _ => {
                        let (dep, la, lo, gr) = guard.serialize_modifiers();
                        kbd.modifiers(serial, dep, la, lo, gr);
                    }
                }
                // send enter event
                kbd.enter(serial, &surface, guard.serialize_pressed_keys());
            }
            guard.focus = Some((surface, kbd))
        }
    }

    /// Send the keymap to this keyboard
    ///
    /// This should be done first, before anything else is done with this keyboard.
    pub fn send_keymap(&self, kbd: &wl_keyboard::WlKeyboard) {
        let keymap_data = &self.internal.1;
        kbd.keymap(wl_keyboard::KeymapFormat::XkbV1,
                   keymap_data.0.as_raw_fd(),
                   keymap_data.1);
    }
}
