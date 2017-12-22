use backend::input::KeyState;
use std::io::{Error as IoError, Write};
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};
use tempfile::tempfile;
use wayland_server::{Liveness, Resource};
use wayland_server::protocol::{wl_keyboard, wl_surface};
use xkbcommon::xkb;
pub use xkbcommon::xkb::{keysyms, Keysym};

/// Represents the current state of the keyboard modifiers
///
/// Each field of this struct represents a modifier and is `true` if this modifier is active.
///
/// For some modifiers, this means that the key is currently pressed, others are toggled
/// (like caps lock).
#[derive(Copy, Clone, Debug)]
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
    known_kbds: Vec<wl_keyboard::WlKeyboard>,
    focus: Option<wl_surface::WlSurface>,
    pressed_keys: Vec<u32>,
    mods_state: ModifiersState,
    keymap: xkb::Keymap,
    state: xkb::State,
    repeat_rate: i32,
    repeat_delay: i32,
}

impl KbdInternal {
    fn new(
        rules: &str, model: &str, layout: &str, variant: &str, options: Option<String>, repeat_rate: i32,
        repeat_delay: i32,
    ) -> Result<KbdInternal, ()> {
        // we create a new contex for each keyboard because libxkbcommon is actually NOT threadsafe
        // so confining it inside the KbdInternal allows us to use Rusts mutability rules to make
        // sure nothing goes wrong.
        //
        // FIXME: This is an issue with the xkbcommon-rs crate that does not reflect this
        // non-threadsafety properly.
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            &rules,
            &model,
            &layout,
            &variant,
            options,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        ).ok_or(())?;
        let state = xkb::State::new(&keymap);
        Ok(KbdInternal {
            known_kbds: Vec::new(),
            focus: None,
            pressed_keys: Vec::new(),
            mods_state: ModifiersState::new(),
            keymap: keymap,
            state: state,
            repeat_rate: repeat_rate,
            repeat_delay: repeat_delay,
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
        // Offset the keycode by 8, as the evdev XKB rules reflect X's
        // broken keycode system, which starts at 8.
        let state_components = self.state.update_key(keycode + 8, direction);

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

        (mods_depressed, mods_latched, mods_locked, layout_locked)
    }

    fn serialize_pressed_keys(&self) -> Vec<u8> {
        let serialized = unsafe {
            ::std::slice::from_raw_parts(
                self.pressed_keys.as_ptr() as *const u8,
                self.pressed_keys.len() * 4,
            )
        };
        serialized.into()
    }

    fn with_focused_kbds<F>(&self, mut f: F)
    where
        F: FnMut(&wl_keyboard::WlKeyboard, &wl_surface::WlSurface),
    {
        if let Some(ref surface) = self.focus {
            for kbd in &self.known_kbds {
                if kbd.same_client_as(surface) {
                    f(kbd, surface);
                }
            }
        }
    }
}

/// Errors that can be encountered when creating a keyboard handler
#[derive(Debug)]
pub enum Error {
    /// libxkbcommon could not load the specified keymap
    BadKeymap,
    /// Smithay could not create a tempfile to share the keymap with clients
    IoError(IoError),
}

/// Create a keyboard handler from a set of RMLVO rules
pub(crate) fn create_keyboard_handler(
    rules: &str, model: &str, layout: &str, variant: &str, options: Option<String>, repeat_delay: i32,
    repeat_rate: i32, logger: &::slog::Logger,
) -> Result<KeyboardHandle, Error> {
    let log = logger.new(o!("smithay_module" => "xkbcommon_handler"));
    info!(log, "Initializing a xkbcommon handler with keymap query";
        "rules" => rules, "model" => model, "layout" => layout, "variant" => variant,
        "options" => &options
    );
    let internal = KbdInternal::new(
        rules,
        model,
        layout,
        variant,
        options,
        repeat_rate,
        repeat_delay,
    ).map_err(|_| {
        debug!(log, "Loading keymap failed");
        Error::BadKeymap
    })?;

    info!(log, "Loaded Keymap"; "name" => internal.keymap.layouts().next());

    // prepare a tempfile with the keymap, to send it to clients
    let mut keymap_file = tempfile().map_err(Error::IoError)?;
    let keymap_data = internal.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    keymap_file
        .write_all(keymap_data.as_bytes())
        .map_err(Error::IoError)?;
    keymap_file.flush().map_err(Error::IoError)?;

    trace!(log, "Keymap loaded and copied to tempfile.";
        "fd" => keymap_file.as_raw_fd(), "len" => keymap_data.as_bytes().len()
    );

    Ok(KeyboardHandle {
        arc: Arc::new(KbdArc {
            internal: Mutex::new(internal),
            keymap_file: keymap_file,
            keymap_len: keymap_data.as_bytes().len() as u32,
            logger: log,
        }),
    })
}

struct KbdArc {
    internal: Mutex<KbdInternal>,
    keymap_file: ::std::fs::File,
    keymap_len: u32,
    logger: ::slog::Logger,
}

/// An handle to a keyboard handler
///
/// It can be cloned and all clones manipulate the same internal state. Clones
/// can also be sent across threads.
///
/// This handle gives you 2 main ways to interact with the keyboard handling:
///
/// - set the current focus for this keyboard: designing the surface that will receive the key inputs
///   using the `KeyboardHandle::set_focus` method.
/// - process key inputs from the input backend, allowing them to be catched at the compositor-level
///   or forwarded to the client. See the documentation of the `KeyboardHandle::input` method for
///   details.
#[derive(Clone)]
pub struct KeyboardHandle {
    arc: Arc<KbdArc>,
}

impl KeyboardHandle {
    /// Handle a keystroke
    ///
    /// All keystrokes from the input backend should be fed _in order_ to this method of the
    /// keyboard handler. It will internally track the state of the keymap.
    ///
    /// The `filter` argument is expected to be a closure which will peek at the generated input
    /// as interpreted by the keymap before it is forwarded to the focused client. If this closure
    /// returns false, the input will not be sent to the client. This mechanism can be used to
    /// implement compositor-level key bindings for example.
    ///
    /// The module `smithay::keyboard::keysyms` exposes definitions of all possible keysyms
    /// to be compared against. This includes non-characted keysyms, such as XF86 special keys.
    pub fn input<F>(&self, keycode: u32, state: KeyState, serial: u32, filter: F)
    where
        F: FnOnce(&ModifiersState, Keysym) -> bool,
    {
        trace!(self.arc.logger, "Handling keystroke"; "keycode" => keycode, "state" => format_args!("{:?}", state));
        let mut guard = self.arc.internal.lock().unwrap();

        // Offset the keycode by 8, as the evdev XKB rules reflect X's
        // broken keycode system, which starts at 8.
        let sym = guard.state.key_get_one_sym(keycode + 8);

        let mods_changed = guard.key_input(keycode, state);

        trace!(self.arc.logger, "Calling input filter";
            "mods_state" => format_args!("{:?}", guard.mods_state), "sym" => xkb::keysym_get_name(sym)
        );

        if !filter(&guard.mods_state, sym) {
            // the filter returned false, we do not forward to client
            trace!(self.arc.logger, "Input was intercepted by filter");
            return;
        }

        // forward to client if no keybinding is triggered
        let modifiers = if mods_changed {
            Some(guard.serialize_modifiers())
        } else {
            None
        };
        let wl_state = match state {
            KeyState::Pressed => wl_keyboard::KeyState::Pressed,
            KeyState::Released => wl_keyboard::KeyState::Released,
        };
        guard.with_focused_kbds(|kbd, _| {
            if let Some((dep, la, lo, gr)) = modifiers {
                kbd.modifiers(serial, dep, la, lo, gr);
            }
            kbd.key(serial, 0, keycode, wl_state);
        });
        if guard.focus.is_some() {
            trace!(self.arc.logger, "Input forwarded to client");
        } else {
            trace!(self.arc.logger, "No client currently focused");
        }
    }

    /// Set the current focus of this keyboard
    ///
    /// If the ne focus is different from the previous one, any previous focus
    /// will be sent a `wl_keyboard::leave` event, and if the new focus is not `None`,
    /// a `wl_keyboard::enter` event will be sent.
    pub fn set_focus(&self, focus: Option<&wl_surface::WlSurface>, serial: u32) {
        let mut guard = self.arc.internal.lock().unwrap();

        let same = guard
            .focus
            .as_ref()
            .and_then(|f| focus.map(|s| s.equals(f)))
            .unwrap_or(false);

        if !same {
            // unset old focus
            guard.with_focused_kbds(|kbd, s| {
                kbd.leave(serial, s);
            });

            // set new focus
            guard.focus = focus.and_then(|s| s.clone());
            let (dep, la, lo, gr) = guard.serialize_modifiers();
            let keys = guard.serialize_pressed_keys();
            guard.with_focused_kbds(|kbd, s| {
                kbd.modifiers(serial, dep, la, lo, gr);
                kbd.enter(serial, s, keys.clone());
            });
            if guard.focus.is_some() {
                trace!(self.arc.logger, "Focus set to new surface");
            } else {
                trace!(self.arc.logger, "Focus unset");
            }
        } else {
            trace!(self.arc.logger, "Focus unchanged");
        }
    }

    /// Register a new keyboard to this handler
    ///
    /// The keymap will automatically be sent to it
    ///
    /// This should be done first, before anything else is done with this keyboard.
    pub(crate) fn new_kbd(&self, kbd: wl_keyboard::WlKeyboard) {
        trace!(self.arc.logger, "Sending keymap to client");
        kbd.keymap(
            wl_keyboard::KeymapFormat::XkbV1,
            self.arc.keymap_file.as_raw_fd(),
            self.arc.keymap_len,
        );
        let mut guard = self.arc.internal.lock().unwrap();
        if kbd.version() >= 4 {
            kbd.repeat_info(guard.repeat_rate, guard.repeat_delay);
        }
        guard.known_kbds.push(kbd);
    }

    /// Change the repeat info configured for this keyboard
    pub fn change_repeat_info(&self, rate: i32, delay: i32) {
        let mut guard = self.arc.internal.lock().unwrap();
        guard.repeat_delay = delay;
        guard.repeat_rate = rate;
        for kbd in &guard.known_kbds {
            kbd.repeat_info(rate, delay);
        }
    }

    /// Performs an internal cleanup of known kbds
    ///
    /// Drops any wl_keyboard that is no longer alive
    pub(crate) fn cleanup_old_kbds(&self) {
        let mut guard = self.arc.internal.lock().unwrap();
        guard
            .known_kbds
            .retain(|kbd| kbd.status() != Liveness::Dead);
    }
}
