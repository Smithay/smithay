use crate::backend::input::KeyState;
use crate::wayland::Serial;
use slog::{debug, info, o, trace, warn};
use std::{
    cell::{Ref, RefCell},
    default::Default,
    fmt,
    io::{self, Seek, Write},
    ops::Deref as _,
    os::unix::io::AsRawFd,
    rc::Rc,
};
use tempfile::tempfile;
use thiserror::Error;

#[cfg(feature = "wayland_frontend")]
use wayland_server::{
    protocol::{
        wl_keyboard::{KeyState as WlKeyState, KeymapFormat, Request, WlKeyboard},
        wl_surface::WlSurface,
    },
    Client, Filter, Main,
};
use xkbcommon::xkb;
pub use xkbcommon::xkb::{keysyms, Keysym};

/// Represents the current state of the keyboard modifiers
///
/// Each field of this struct represents a modifier and is `true` if this modifier is active.
///
/// For some modifiers, this means that the key is currently pressed, others are toggled
/// (like caps lock).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
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
    fn update_with(&mut self, state: &xkb::State) {
        self.ctrl = state.mod_name_is_active(&xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
        self.alt = state.mod_name_is_active(&xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE);
        self.shift = state.mod_name_is_active(&xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE);
        self.caps_lock = state.mod_name_is_active(&xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE);
        self.logo = state.mod_name_is_active(&xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE);
        self.num_lock = state.mod_name_is_active(&xkb::MOD_NAME_NUM, xkb::STATE_MODS_EFFECTIVE);
    }
}

/// Configuration for xkbcommon.
///
/// For the fields that are not set ("" or None, as set in the `Default` impl), xkbcommon will use
/// the values from the environment variables `XKB_DEFAULT_RULES`, `XKB_DEFAULT_MODEL`,
/// `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT` and `XKB_DEFAULT_OPTIONS`.
///
/// For details, see the [documentation at xkbcommon.org][docs].
///
/// [docs]: https://xkbcommon.org/doc/current/structxkb__rule__names.html
#[derive(Default, Clone, Debug)]
pub struct XkbConfig<'a> {
    /// The rules file to use.
    ///
    /// The rules file describes how to interpret the values of the model, layout, variant and
    /// options fields.
    pub rules: &'a str,
    /// The keyboard model by which to interpret keycodes and LEDs.
    pub model: &'a str,
    /// A comma separated list of layouts (languages) to include in the keymap.
    pub layout: &'a str,
    /// A comma separated list of variants, one per layout, which may modify or augment the
    /// respective layout in various ways.
    pub variant: &'a str,
    /// A comma separated list of options, through which the user specifies non-layout related
    /// preferences, like which key combinations are used for switching layouts, or which key is the
    /// Compose key.
    pub options: Option<String>,
}

enum GrabStatus {
    None,
    Active(Serial, Box<dyn KeyboardGrab>),
    Borrowed,
}

pub trait KeyboardHandler: std::any::Any {
    fn enter(&mut self, keys: Vec<KeysymHandle<'_>>, serial: Serial);
    fn leave(&mut self, serial: Serial);
    fn key(&mut self, key: KeysymHandle<'_>, state: KeyState, serial: Serial, time: u32);
    fn modifiers(&mut self, state: &xkb::State, modifiers: ModifiersState, serial: Serial);

    fn is_alive(&self) -> bool;
    fn same_handler_as(&self, other: &dyn KeyboardHandler) -> bool;
    fn as_any<'a>(&'a self) -> Box<dyn std::ops::Deref<Target = dyn std::any::Any> + 'a>;
}

impl KeyboardHandler for Box<dyn KeyboardHandler> {
    fn enter(&mut self, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        KeyboardHandler::enter(&mut **self, keys, serial)
    }
    fn leave(&mut self, serial: Serial) {
        KeyboardHandler::leave(&mut **self, serial)
    }
    fn key(&mut self, key: KeysymHandle<'_>, state: KeyState, serial: Serial, time: u32) {
        KeyboardHandler::key(&mut **self, key, state, serial, time)
    }
    fn modifiers(&mut self, state: &xkb::State, modifiers: ModifiersState, serial: Serial) {
        KeyboardHandler::modifiers(&mut **self, state, modifiers, serial)
    }

    fn is_alive(&self) -> bool {
        KeyboardHandler::is_alive(&**self)
    }
    fn same_handler_as(&self, other: &dyn KeyboardHandler) -> bool {
        KeyboardHandler::same_handler_as(&**self, other)
    }
    fn as_any<'a>(&'a self) -> Box<dyn std::ops::Deref<Target = dyn std::any::Any> + 'a> {
        Box::new(self as &'a dyn std::any::Any)
    }
}

impl KeyboardHandler for Rc<RefCell<Box<dyn KeyboardHandler>>> {
    fn enter(&mut self, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        self.borrow_mut().enter(keys, serial)
    }
    fn leave(&mut self, serial: Serial) {
        self.borrow_mut().leave(serial)
    }
    fn key(&mut self, key: KeysymHandle<'_>, state: KeyState, serial: Serial, time: u32) {
        self.borrow_mut().key(key, state, serial, time)
    }
    fn modifiers(&mut self, state: &xkb::State, modifiers: ModifiersState, serial: Serial) {
        self.borrow_mut().modifiers(state, modifiers, serial)
    }

    fn is_alive(&self) -> bool {
        self.borrow().is_alive()
    }
    fn same_handler_as(&self, other: &dyn KeyboardHandler) -> bool {
        self.borrow().same_handler_as(other)
    }
    fn as_any<'a>(&'a self) -> Box<dyn std::ops::Deref<Target = dyn std::any::Any> + 'a> {
        Box::new(Ref::map(self.borrow(), |k| k as &dyn std::any::Any))
    }
}

struct DummyHandler;
impl KeyboardHandler for DummyHandler {
    fn enter(&mut self, _keys: Vec<KeysymHandle<'_>>, _serial: Serial) {
        unimplemented!()
    }
    fn leave(&mut self, _serial: Serial) {
        unimplemented!()
    }
    fn key(&mut self, _key: KeysymHandle<'_>, _state: KeyState, _serial: Serial, _time: u32) {
        unimplemented!()
    }
    fn modifiers(&mut self, _state: &xkb::State, _modifiers: ModifiersState, _serial: Serial) {
        unimplemented!()
    }

    fn is_alive(&self) -> bool {
        unimplemented!()
    }
    fn same_handler_as(&self, _other: &dyn KeyboardHandler) -> bool {
        unimplemented!()
    }
    fn as_any<'a>(&'a self) -> Box<dyn std::ops::Deref<Target = dyn std::any::Any> + 'a> {
        unimplemented!()
    }
}

struct KbdInternal {
    focus: Option<(Box<dyn KeyboardHandler>, Serial)>,
    pending_focus: Option<Rc<RefCell<Box<dyn KeyboardHandler>>>>,
    pressed_keys: Vec<u32>,
    mods_state: ModifiersState,
    keymap: xkb::Keymap,
    state: xkb::State,
    repeat_rate: i32,
    repeat_delay: i32,
    focus_hook: Box<dyn FnMut(Option<&dyn KeyboardHandler>)>,
    grab: GrabStatus,
}

// focus_hook does not implement debug, so we have to impl Debug manually
impl fmt::Debug for KbdInternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KbdInternal")
            .field("focus", &self.focus.as_ref().map(|(_, s)| (&"...", s)))
            .field("pressed_keys", &self.pressed_keys)
            .field("mods_state", &self.mods_state)
            .field("keymap", &self.keymap.get_raw_ptr())
            .field("state", &self.state.get_raw_ptr())
            .field("repeat_rate", &self.repeat_rate)
            .field("repeat_delay", &self.repeat_delay)
            .field("focus_hook", &"...")
            .finish()
    }
}

// This is OK because all parts of `xkb` will remain on the
// same thread
unsafe impl Send for KbdInternal {}

impl KbdInternal {
    fn new(
        xkb_config: XkbConfig<'_>,
        repeat_rate: i32,
        repeat_delay: i32,
        focus_hook: Box<dyn FnMut(Option<&dyn KeyboardHandler>)>,
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
            &xkb_config.rules,
            &xkb_config.model,
            &xkb_config.layout,
            &xkb_config.variant,
            xkb_config.options,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or(())?;
        let state = xkb::State::new(&keymap);
        Ok(KbdInternal {
            focus: None,
            pending_focus: None,
            pressed_keys: Vec::new(),
            mods_state: ModifiersState::default(),
            keymap,
            state,
            repeat_rate,
            repeat_delay,
            focus_hook,
            grab: GrabStatus::None,
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

    /*
    fn serialize_modifiers(&self) -> (u32, u32, u32, u32) {
        let mods_depressed = self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
        let mods_latched = self.state.serialize_mods(xkb::STATE_MODS_LATCHED);
        let mods_locked = self.state.serialize_mods(xkb::STATE_MODS_LOCKED);
        let layout_locked = self.state.serialize_layout(xkb::STATE_LAYOUT_LOCKED);

        (mods_depressed, mods_latched, mods_locked, layout_locked)
    }
    */

    fn pressed_keys(&self) -> Vec<u32> {
        self.pressed_keys.clone()
    }

    fn with_grab<F>(&mut self, f: F, focus: Option<impl KeyboardHandler>, logger: ::slog::Logger)
    where
        F: FnOnce(KeyboardInnerHandle<'_>, &mut dyn KeyboardGrab, Option<Box<dyn KeyboardHandler>>),
    {
        let focus = focus.map(|h| Box::new(h) as Box<dyn KeyboardHandler>);
        let mut grab = ::std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => panic!("Accessed a keyboard grab from within a keyboard grab access."),
            GrabStatus::Active(_, ref mut handler) => {
                // If this grab is associated with a surface that is no longer alive, discard it
                if let Some(ref surface) = handler.start_data().focus {
                    if !surface.as_ref().is_alive() {
                        self.grab = GrabStatus::None;
                        let _ = self.pending_focus.take();
                        f(
                            KeyboardInnerHandle { inner: self, logger },
                            &mut DefaultGrab,
                            focus,
                        );
                        return;
                    }
                }

                let focus = focus.map(|h| Rc::new(RefCell::new(h)));
                self.pending_focus = focus.clone();

                f(
                    KeyboardInnerHandle { inner: self, logger },
                    &mut **handler,
                    focus.map(|h| Box::new(h) as Box<dyn KeyboardHandler>),
                );
            }
            GrabStatus::None => {
                f(
                    KeyboardInnerHandle { inner: self, logger },
                    &mut DefaultGrab,
                    focus,
                );
            }
        }

        if let GrabStatus::Borrowed = self.grab {
            // the grab has not been ended nor replaced, put it back in place
            self.grab = grab;
        }
    }
}

/// Errors that can be encountered when creating a keyboard handler
#[derive(Debug, Error)]
pub enum Error {
    /// libxkbcommon could not load the specified keymap
    #[error("Libxkbcommon could not load the specified keymap")]
    BadKeymap,
    /// Smithay could not create a tempfile to share the keymap with clients
    #[error("Failed to create tempfile to share the keymap: {0}")]
    IoError(io::Error),
}

/// Create a keyboard handler from a set of RMLVO rules
pub(crate) fn create_keyboard_handler<F>(
    xkb_config: XkbConfig<'_>,
    repeat_delay: i32,
    repeat_rate: i32,
    logger: &::slog::Logger,
    focus_hook: F,
) -> Result<KeyboardHandle, Error>
where
    F: FnMut(Option<&dyn KeyboardHandler>) + 'static,
{
    let log = logger.new(o!("smithay_module" => "xkbcommon_handler"));
    info!(log, "Initializing a xkbcommon handler with keymap query";
        "rules" => xkb_config.rules, "model" => xkb_config.model, "layout" => xkb_config.layout,
        "variant" => xkb_config.variant, "options" => &xkb_config.options
    );
    let internal =
        KbdInternal::new(xkb_config, repeat_rate, repeat_delay, Box::new(focus_hook)).map_err(|_| {
            debug!(log, "Loading keymap failed");
            Error::BadKeymap
        })?;

    info!(log, "Loaded Keymap"; "name" => internal.keymap.layouts().next());

    let keymap = internal.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);

    Ok(KeyboardHandle {
        rc: Rc::new(KbdRc {
            internal: RefCell::new(internal),
            keymap,
            logger: log,
        }),
    })
}

#[derive(Debug)]
struct KbdRc {
    internal: RefCell<KbdInternal>,
    #[allow(dead_code)]
    keymap: String,
    logger: ::slog::Logger,
}

/// Handle to the underlying keycode to allow for different conversions
pub struct KeysymHandle<'a> {
    keycode: u32,
    keymap: &'a xkb::Keymap,
    state: &'a xkb::State,
}

impl<'a> fmt::Debug for KeysymHandle<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.keycode)
    }
}

impl<'a> KeysymHandle<'a> {
    /// Returns the sym for the underlying keycode with all modifications by the current keymap state applied.
    ///
    /// This function is similar to [`KeysymHandle::modified_syms`], but is intended for cases where the user
    /// does not want to or cannot handle multiple keysyms.
    ///
    /// If the key does not have exactly one keysym, returns [`keysyms::KEY_NoSymbol`].
    pub fn modified_sym(&'a self) -> Keysym {
        self.state.key_get_one_sym(self.keycode)
    }

    /// Returns the syms for the underlying keycode with all modifications by the current keymap state applied.
    pub fn modified_syms(&'a self) -> &'a [Keysym] {
        self.state.key_get_syms(self.keycode)
    }

    /// Returns the syms for the underlying keycode without any modifications by the current keymap state applied.
    pub fn raw_syms(&'a self) -> &'a [Keysym] {
        self.keymap
            .key_get_syms_by_level(self.keycode, self.state.key_get_layout(self.keycode), 0)
    }

    /// Returns the raw code in X keycode system (shifted by 8)
    pub fn raw_code(&'a self) -> u32 {
        self.keycode
    }
}

/// Result for key input filtering (see [`KeyboardHandle::input`])
#[derive(Debug)]
pub enum FilterResult<T> {
    /// Forward the given keycode to the client
    Forward,
    /// Do not forward and return value
    Intercept(T),
}

/// Data about the event that started the grab.
pub struct GrabStartData {
    /// The focused surface, if any, at the start of the grab.
    pub focus: Option<Box<dyn KeyboardHandler>>,
}

impl fmt::Debug for GrabStartData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrabStartData").field("focus", &"...").finish()
    }
}

/// A trait to implement a keyboard grab
///
/// In some context, it is necessary to temporarily change the behavior of the keyboard. This is
/// typically known as a keyboard grab. A example would be, during a popup grab the keyboard focus
/// will not be changed and stay on the grabbed popup.
///
/// This trait is the interface to intercept regular keyboard events and change them as needed, its
/// interface mimics the [`KeyboardHandle`] interface.
///
/// If your logic decides that the grab should end, both [`KeyboardInnerHandle`] and [`KeyboardHandle`] have
/// a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the server),
/// the struct implementing this trait will be dropped. As such you should put clean-up logic in the destructor,
/// rather than trying to guess when the grab will end.
pub trait KeyboardGrab {
    /// An input was reported
    fn input(
        &mut self,
        handle: &mut KeyboardInnerHandle<'_>,
        keycode: u32,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    );

    /// A focus change was requested
    fn set_focus(
        &mut self,
        handle: &mut KeyboardInnerHandle<'_>,
        focus: Option<Box<dyn KeyboardHandler>>,
        serial: Serial,
    );

    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData;
}

/// An handle to a keyboard handler
///
/// It can be cloned and all clones manipulate the same internal state.
///
/// This handle gives you 2 main ways to interact with the keyboard handling:
///
/// - set the current focus for this keyboard: designing the surface that will receive the key inputs
///   using the [`KeyboardHandle::set_focus`] method.
/// - process key inputs from the input backend, allowing them to be caught at the compositor-level
///   or forwarded to the client. See the documentation of the [`KeyboardHandle::input`] method for
///   details.
#[derive(Debug, Clone)]
pub struct KeyboardHandle {
    rc: Rc<KbdRc>,
}

impl KeyboardHandle {
    /// Change the current grab on this keyboard to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: KeyboardGrab + 'static>(&self, grab: G, serial: Serial) {
        self.rc.internal.borrow_mut().grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this keyboard, resetting it to the default behavior
    pub fn unset_grab(&self) {
        self.rc.internal.borrow_mut().grab = GrabStatus::None;
    }

    /// Check if this keyboard is currently grabbed with this serial
    pub fn has_grab(&self, serial: Serial) -> bool {
        let guard = self.rc.internal.borrow_mut();
        match guard.grab {
            GrabStatus::Active(s, _) => s == serial,
            _ => false,
        }
    }

    /// Check if this keyboard is currently being grabbed
    pub fn is_grabbed(&self) -> bool {
        let guard = self.rc.internal.borrow_mut();
        !matches!(guard.grab, GrabStatus::None)
    }

    /// Returns the start data for the grab, if any.
    pub fn grab_start_data(&self) -> Option<Ref<'_, GrabStartData>> {
        let guard = self.rc.internal.borrow();
        if matches!(&guard.grab, GrabStatus::Active(_, _)) {
            Some(Ref::map(guard, |g| match &g.grab {
                GrabStatus::Active(_, g) => g.start_data(),
                _ => unreachable!(),
            }))
        } else {
            None
        }
    }

    /// Handle a keystroke
    ///
    /// All keystrokes from the input backend should be fed _in order_ to this method of the
    /// keyboard handler. It will internally track the state of the keymap.
    ///
    /// The `filter` argument is expected to be a closure which will peek at the generated input
    /// as interpreted by the keymap before it is forwarded to the focused client. If this closure
    /// returns [`FilterResult::Forward`], the input will not be sent to the client. If it returns
    /// [`FilterResult::Intercept`] a value can be passed to be returned by the whole function.
    /// This mechanism can be used to implement compositor-level key bindings for example.
    ///
    /// The module [`crate::wayland::seat::keysyms`] exposes definitions of all possible keysyms
    /// to be compared against. This includes non-character keysyms, such as XF86 special keys.
    pub fn input<T, F>(
        &self,
        keycode: u32,
        state: KeyState,
        serial: Serial,
        time: u32,
        filter: F,
    ) -> Option<T>
    where
        F: FnOnce(&ModifiersState, KeysymHandle<'_>) -> FilterResult<T>,
    {
        trace!(self.rc.logger, "Handling keystroke"; "keycode" => keycode, "state" => format_args!("{:?}", state));
        let mut guard = self.rc.internal.borrow_mut();
        let mods_changed = guard.key_input(keycode, state);
        let handle = KeysymHandle {
            // Offset the keycode by 8, as the evdev XKB rules reflect X's
            // broken keycode system, which starts at 8.
            keycode: keycode + 8,
            state: &guard.state,
            keymap: &guard.keymap,
        };

        trace!(self.rc.logger, "Calling input filter";
            "mods_state" => format_args!("{:?}", guard.mods_state), "sym" => xkb::keysym_get_name(handle.modified_sym())
        );

        if let FilterResult::Intercept(val) = filter(&guard.mods_state, handle) {
            // the filter returned false, we do not forward to client
            trace!(self.rc.logger, "Input was intercepted by filter");
            return Some(val);
        }

        // forward to handler if no keybinding is triggered
        let mods_state = guard.mods_state;
        guard.with_grab(
            move |mut kbd_handle, grab, _focus| {
                grab.input(
                    &mut kbd_handle,
                    keycode,
                    state,
                    if mods_changed { Some(mods_state) } else { None },
                    serial,
                    time,
                );
            },
            Option::<DummyHandler>::None,
            self.rc.logger.clone(),
        );
        if guard.focus.is_some() {
            trace!(self.rc.logger, "Input forwarded to client");
        } else {
            trace!(self.rc.logger, "No client currently focused");
        }

        None
    }

    /// Set the current focus of this keyboard
    ///
    /// If the new focus is different from the previous one, any previous focus
    /// will be sent a [`wl_keyboard::Event::Leave`](wayland_server::protocol::wl_keyboard::Event::Leave)
    /// event, and if the new focus is not `None`,
    /// a [`wl_keyboard::Event::Enter`](wayland_server::protocol::wl_keyboard::Event::Enter) event will be sent.
    pub fn set_focus(&self, focus: Option<impl KeyboardHandler>, serial: Serial) {
        let mut guard = self.rc.internal.borrow_mut();
        guard.with_grab(
            move |mut handle, grab, focus| {
                grab.set_focus(&mut handle, focus, serial);
            },
            focus,
            self.rc.logger.clone(),
        );
    }

    /// Check if given client currently has keyboard focus
    pub fn has_focus(&self, client: &Client) -> bool {
        self.rc
            .internal
            .borrow_mut()
            .focus
            .as_ref()
            .and_then(|f| {
                f.0.as_any()
                    .downcast_ref::<WlSurface>()
                    .and_then(|s| s.as_ref().client())
            })
            .map(|c| c.equals(client))
            .unwrap_or(false)
    }

    /// Check if keyboard has focus
    pub fn is_focused(&self) -> bool {
        self.rc.internal.borrow_mut().focus.is_some()
    }

    /// Change the repeat info configured for this keyboard
    pub fn change_repeat_info(&self, rate: i32, delay: i32) {
        let mut guard = self.rc.internal.borrow_mut();
        guard.repeat_delay = delay;
        guard.repeat_rate = rate;
        /*
        for kbd in &guard.known_kbds {
            kbd.repeat_info(rate, delay);
        }
        */
    }
}

struct KnownKeyboards(RefCell<Vec<WlKeyboard>>);

#[cfg(feature = "wayland_frontend")]
pub(crate) fn implement_keyboard(keyboard: Main<WlKeyboard>, handle: Option<&KeyboardHandle>) {
    let client = keyboard.as_ref().client().unwrap();
    {
        let client_data_map = client.data_map();
        client_data_map.insert_if_missing(|| KnownKeyboards(RefCell::new(Vec::new())));
        client_data_map
            .get::<KnownKeyboards>()
            .unwrap()
            .0
            .borrow_mut()
            .push(keyboard.deref().clone());
    }

    keyboard.quick_assign(|_keyboard, request, _data| {
        match request {
            Request::Release => {
                // Our destructors already handle it
            }
            _ => unreachable!(),
        }
    });

    keyboard.assign_destructor(Filter::new(move |keyboard: WlKeyboard, _, _| {
        client
            .data_map()
            .get::<KnownKeyboards>()
            .unwrap()
            .0
            .borrow_mut()
            .retain(|k| !k.as_ref().equals(keyboard.as_ref()))
    }));

    if let Some(h) = handle {
        trace!(h.rc.logger, "Sending keymap to client");

        // prepare a tempfile with the keymap, to send it to the client
        let ret = tempfile().and_then(|mut f| {
            f.write_all(h.rc.keymap.as_bytes())?;
            f.flush()?;
            f.rewind()?;
            keyboard.keymap(
                KeymapFormat::XkbV1,
                f.as_raw_fd(),
                h.rc.keymap.as_bytes().len() as u32,
            );
            Ok(())
        });

        if let Err(e) = ret {
            warn!(h.rc.logger,
                "Failed write keymap to client in a tempfile";
                "err" => format!("{:?}", e)
            );
            return;
        };

        let guard = h.rc.internal.borrow_mut();
        if keyboard.as_ref().version() >= 4 {
            keyboard.repeat_info(guard.repeat_rate, guard.repeat_delay);
        }
        if let Some((focused, serial)) = guard.focus.as_ref() {
            if let Some(focused_surface) = focused.as_any().downcast_ref::<WlSurface>() {
                if focused_surface.as_ref().same_client_as(keyboard.as_ref()) {
                    let (dep, la, lo, gr) = serialize_modifiers(&guard.state);
                    let keys = serialize_pressed_keys(&guard.pressed_keys);
                    keyboard.enter((*serial).into(), focused_surface, keys);
                    // Modifiers must be send after enter event.
                    keyboard.modifiers((*serial).into(), dep, la, lo, gr);
                }
            }
        }
    }
}

#[cfg(feature = "wayland_frontend")]
fn serialize_modifiers(mods: &xkb::State) -> (u32, u32, u32, u32) {
    let mods_depressed = mods.serialize_mods(xkb::STATE_MODS_DEPRESSED);
    let mods_latched = mods.serialize_mods(xkb::STATE_MODS_LATCHED);
    let mods_locked = mods.serialize_mods(xkb::STATE_MODS_LOCKED);
    let layout_locked = mods.serialize_layout(xkb::STATE_LAYOUT_LOCKED);

    (mods_depressed, mods_latched, mods_locked, layout_locked)
}

#[cfg(feature = "wayland_frontend")]
fn serialize_pressed_keys(pressed_keys: &Vec<u32>) -> Vec<u8> {
    let serialized =
        unsafe { ::std::slice::from_raw_parts(pressed_keys.as_ptr() as *const u8, pressed_keys.len() * 4) };
    serialized.into()
}

/// This inner handle is accessed from inside a keyboard grab logic, and directly
/// sends event to the client
#[derive(Debug)]
pub struct KeyboardInnerHandle<'a> {
    inner: &'a mut KbdInternal,
    logger: ::slog::Logger,
}

impl<'a> KeyboardInnerHandle<'a> {
    /// Change the current grab on this keyboard to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: KeyboardGrab + 'static>(&mut self, serial: Serial, grab: G) {
        self.inner.grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this keyboard, resetting it to the default behavior
    ///
    /// This will also restore the focus of the underlying keyboard if restore_focus
    /// is [`true`]
    pub fn unset_grab(&mut self, serial: Serial, restore_focus: bool) {
        self.inner.grab = GrabStatus::None;
        // restore the focus
        if restore_focus {
            let focus = self.inner.pending_focus.take();
            self.set_focus(focus, serial);
        }
    }

    /// Access the current focus of this keyboard
    pub fn current_focus(&self) -> Option<&dyn KeyboardHandler> {
        self.inner.focus.as_ref().map(|f| &*f.0)
    }

    /// Send the input to the focused keyboards
    pub fn input(
        &mut self,
        keycode: u32,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        // key event must be sent before modifers event for libxkbcommon
        // to process them correctly
        if let Some((focus, _)) = self.inner.focus.as_mut() {
            let key = KeysymHandle {
                keycode: keycode + 8,
                state: &self.inner.state,
                keymap: &self.inner.keymap,
            };
            focus.key(key, key_state, serial, time);
            if let Some(modifiers) = modifiers {
                focus.modifiers(&self.inner.state, modifiers, serial);
            };
        }
    }

    /// Set the current focus of this keyboard
    ///
    /// If the new focus is different from the previous one, any previous focus
    /// will be sent a [`wl_keyboard::Event::Leave`](wayland_server::protocol::wl_keyboard::Event::Leave)
    /// event, and if the new focus is not `None`,
    /// a [`wl_keyboard::Event::Enter`](wayland_server::protocol::wl_keyboard::Event::Enter) event will be sent.
    pub fn set_focus(&mut self, focus: Option<impl KeyboardHandler>, serial: Serial) {
        let same = self
            .inner
            .focus
            .as_ref()
            .and_then(|f| focus.as_ref().map(|f2| f2.same_handler_as(&*f.0)))
            .unwrap_or(false);

        if !same {
            // unset old focus
            if let Some((old_focus, _)) = self.inner.focus.as_mut() {
                old_focus.leave(serial);
            }

            // set new focus
            self.inner.focus = focus.map(|f| (Box::new(f) as Box<dyn KeyboardHandler>, serial));
            let state = &self.inner.state;
            let keymap = &self.inner.keymap;
            let keys = self
                .inner
                .pressed_keys()
                .into_iter()
                .map(|code| KeysymHandle {
                    keycode: code + 8,
                    state,
                    keymap,
                })
                .collect();

            if let Some((focus, _)) = self.inner.focus.as_mut() {
                focus.enter(keys, serial);
                // Modifiers must be send after enter event.
                focus.modifiers(&self.inner.state, self.inner.mods_state, serial);
            }
            {
                let KbdInternal {
                    ref focus,
                    ref mut focus_hook,
                    ..
                } = *self.inner;
                focus_hook(focus.as_ref().map(|f| &*f.0));
            }
            if self.inner.focus.is_some() {
                trace!(self.logger, "Focus set to new handler");
            } else {
                trace!(self.logger, "Focus unset");
            }
        } else {
            trace!(self.logger, "Focus unchanged");
        }
    }
}

// The default grab, the behavior when no particular grab is in progress
struct DefaultGrab;

impl KeyboardGrab for DefaultGrab {
    fn input(
        &mut self,
        handle: &mut KeyboardInnerHandle<'_>,
        keycode: u32,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        handle.input(keycode, key_state, modifiers, serial, time)
    }

    fn set_focus(
        &mut self,
        handle: &mut KeyboardInnerHandle<'_>,
        focus: Option<Box<dyn KeyboardHandler>>,
        serial: Serial,
    ) {
        handle.set_focus(focus, serial)
    }

    fn start_data(&self) -> &GrabStartData {
        unreachable!()
    }
}

#[cfg(feature = "wayland_frontend")]
impl KeyboardHandler for WlSurface {
    fn enter(&mut self, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_keyboards) = client.data_map().get::<KnownKeyboards>() {
                for kbd in &*known_keyboards.0.borrow() {
                    if kbd.as_ref().same_client_as(self.as_ref()) {
                        kbd.enter(
                            serial.into(),
                            self,
                            serialize_pressed_keys(&keys.iter().map(|h| h.raw_code() - 8).collect()),
                        )
                    }
                }
            }
        }
    }
    fn leave(&mut self, serial: Serial) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_keyboards) = client.data_map().get::<KnownKeyboards>() {
                for kbd in &*known_keyboards.0.borrow() {
                    if kbd.as_ref().same_client_as(self.as_ref()) {
                        kbd.leave(serial.into(), self)
                    }
                }
            }
        }
    }
    fn key(&mut self, key: KeysymHandle<'_>, state: KeyState, serial: Serial, time: u32) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_keyboards) = client.data_map().get::<KnownKeyboards>() {
                for kbd in &*known_keyboards.0.borrow() {
                    if kbd.as_ref().same_client_as(self.as_ref()) {
                        kbd.key(serial.into(), time, key.raw_code() - 8, state.into())
                    }
                }
            }
        }
    }
    fn modifiers(&mut self, state: &xkb::State, _modifiers: ModifiersState, serial: Serial) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_keyboards) = client.data_map().get::<KnownKeyboards>() {
                for kbd in &*known_keyboards.0.borrow() {
                    if kbd.as_ref().same_client_as(self.as_ref()) {
                        let (de, la, lo, gr) = serialize_modifiers(state);
                        kbd.modifiers(serial.into(), de, la, lo, gr)
                    }
                }
            }
        }
    }

    fn is_alive(&self) -> bool {
        self.as_ref().is_alive()
    }
    fn same_handler_as(&self, other: &dyn KeyboardHandler) -> bool {
        if let Some(other_surface) = other.as_any().downcast_ref::<WlSurface>() {
            self == other_surface
        } else {
            false
        }
    }
    fn as_any<'a>(&'a self) -> Box<dyn std::ops::Deref<Target = dyn std::any::Any> + 'a> {
        Box::new(self as &dyn std::any::Any)
    }
}

#[cfg(feature = "wayland_frontend")]
impl From<KeyState> for WlKeyState {
    fn from(s: KeyState) -> Self {
        match s {
            KeyState::Pressed => WlKeyState::Pressed,
            KeyState::Released => WlKeyState::Released,
        }
    }
}
