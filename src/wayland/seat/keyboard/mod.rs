use crate::backend::input::KeyState;
use crate::wayland::Serial;
use slog::{debug, info, o, trace, warn};
use std::{
    default::Default,
    fmt,
    io::{Error as IoError, Write},
    os::unix::io::AsRawFd,
    sync::{Arc, Mutex},
};
use tempfile::tempfile;
use thiserror::Error;
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::{
        wl_keyboard::{self, KeyState as WlKeyState, KeymapFormat, WlKeyboard},
        wl_surface::WlSurface,
    },
    Client, DelegateDispatch, DelegateDispatchBase, Dispatch, DisplayHandle, Resource,
};
use xkbcommon::xkb;
pub use xkbcommon::xkb::{keysyms, Keysym};

use super::{SeatHandler, SeatState};

mod modifiers_state;
pub use modifiers_state::ModifiersState;

mod xkb_config;
pub use xkb_config::XkbConfig;

enum GrabStatus {
    None,
    Active(Serial, Box<dyn KeyboardGrab>),
    Borrowed,
}

struct KbdInternal {
    known_kbds: Vec<WlKeyboard>,
    focus: Option<WlSurface>,
    pending_focus: Option<WlSurface>,
    pressed_keys: Vec<u32>,
    mods_state: ModifiersState,
    keymap: xkb::Keymap,
    state: xkb::State,
    repeat_rate: i32,
    repeat_delay: i32,
    focus_hook: Box<dyn FnMut(Option<&WlSurface>)>,
    grab: GrabStatus,
}

// focus_hook does not implement debug, so we have to impl Debug manually
impl fmt::Debug for KbdInternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KbdInternal")
            .field("known_kbds", &self.known_kbds)
            .field("focus", &self.focus)
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
        focus_hook: Box<dyn FnMut(Option<&WlSurface>)>,
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
            known_kbds: Vec::new(),
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
        F: FnMut(&WlKeyboard, &WlSurface),
    {
        if let Some(ref surface) = self.focus {
            for kbd in self.known_kbds.iter() {
                if kbd.id().same_client_as(&surface.id()) {
                    f(kbd, surface);
                }
            }
        }
    }

    fn with_grab<F>(&mut self, f: F, logger: ::slog::Logger)
    where
        F: FnOnce(KeyboardInnerHandle<'_>, &mut dyn KeyboardGrab),
    {
        let mut grab = ::std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => panic!("Accessed a keyboard grab from within a keyboard grab access."),
            GrabStatus::Active(_, ref mut handler) => {
                // If this grab is associated with a surface that is no longer alive, discard it
                if let Some(ref _surface) = handler.start_data().focus {
                    // TODO
                    /*
                    if !surface.as_ref().is_alive() {
                        self.grab = GrabStatus::None;
                        f(KeyboardInnerHandle { inner: self, logger }, &mut DefaultGrab);
                        return;
                    }
                    */
                }
                f(KeyboardInnerHandle { inner: self, logger }, &mut **handler);
            }
            GrabStatus::None => {
                f(KeyboardInnerHandle { inner: self, logger }, &mut DefaultGrab);
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

#[derive(Debug)]
struct KbdRc {
    internal: Mutex<KbdInternal>,
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
#[derive(Debug, Clone)]
pub struct GrabStartData {
    /// The focused surface, if any, at the start of the grab.
    pub focus: Option<WlSurface>,
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
    #[allow(clippy::too_many_arguments)]
    fn input(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut KeyboardInnerHandle<'_>,
        keycode: u32,
        key_state: WlKeyState,
        modifiers: Option<(u32, u32, u32, u32)>,
        serial: Serial,
        time: u32,
    );

    /// A focus change was requested
    fn set_focus(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut KeyboardInnerHandle<'_>,
        focus: Option<&WlSurface>,
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
    arc: Arc<KbdRc>,
}

impl KeyboardHandle {
    /// Create a keyboard handler from a set of RMLVO rules
    pub(crate) fn new<F>(
        xkb_config: XkbConfig<'_>,
        repeat_delay: i32,
        repeat_rate: i32,
        cb: F,
        logger: &::slog::Logger,
    ) -> Result<Self, Error>
    where
        F: FnMut(Option<&WlSurface>) + 'static,
    {
        let log = logger.new(o!("smithay_module" => "xkbcommon_handler"));
        info!(log, "Initializing a xkbcommon handler with keymap query";
            "rules" => xkb_config.rules, "model" => xkb_config.model, "layout" => xkb_config.layout,
            "variant" => xkb_config.variant, "options" => &xkb_config.options
        );
        let internal =
            KbdInternal::new(xkb_config, repeat_rate, repeat_delay, Box::new(cb)).map_err(|_| {
                debug!(log, "Loading keymap failed");
                Error::BadKeymap
            })?;

        info!(log, "Loaded Keymap"; "name" => internal.keymap.layouts().next());

        let keymap = internal.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);

        Ok(Self {
            arc: Arc::new(KbdRc {
                internal: Mutex::new(internal),
                keymap,
                logger: log,
            }),
        })
    }

    /// Change the current grab on this keyboard to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: KeyboardGrab + 'static>(&self, grab: G, serial: Serial) {
        self.arc.internal.lock().unwrap().grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this keyboard, resetting it to the default behavior
    pub fn unset_grab(&self) {
        self.arc.internal.lock().unwrap().grab = GrabStatus::None;
    }

    /// Check if this keyboard is currently grabbed with this serial
    pub fn has_grab(&self, serial: Serial) -> bool {
        let guard = self.arc.internal.lock().unwrap();
        match guard.grab {
            GrabStatus::Active(s, _) => s == serial,
            _ => false,
        }
    }

    /// Check if this keyboard is currently being grabbed
    pub fn is_grabbed(&self) -> bool {
        let guard = self.arc.internal.lock().unwrap();
        !matches!(guard.grab, GrabStatus::None)
    }

    /// Returns the start data for the grab, if any.
    pub fn grab_start_data(&self) -> Option<GrabStartData> {
        let guard = self.arc.internal.lock().unwrap();
        match &guard.grab {
            GrabStatus::Active(_, g) => Some(g.start_data().clone()),
            _ => None,
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
        dh: &mut DisplayHandle<'_>,
        keycode: u32,
        state: KeyState,
        serial: Serial,
        time: u32,
        filter: F,
    ) -> Option<T>
    where
        F: FnOnce(&ModifiersState, KeysymHandle<'_>) -> FilterResult<T>,
    {
        trace!(self.arc.logger, "Handling keystroke"; "keycode" => keycode, "state" => format_args!("{:?}", state));
        let mut guard = self.arc.internal.lock().unwrap();
        let mods_changed = guard.key_input(keycode, state);
        let handle = KeysymHandle {
            // Offset the keycode by 8, as the evdev XKB rules reflect X's
            // broken keycode system, which starts at 8.
            keycode: keycode + 8,
            state: &guard.state,
            keymap: &guard.keymap,
        };

        trace!(self.arc.logger, "Calling input filter";
            "mods_state" => format_args!("{:?}", guard.mods_state), "sym" => xkb::keysym_get_name(handle.modified_sym())
        );

        if let FilterResult::Intercept(val) = filter(&guard.mods_state, handle) {
            // the filter returned false, we do not forward to client
            trace!(self.arc.logger, "Input was intercepted by filter");
            return Some(val);
        }

        // forward to client if no keybinding is triggered
        let modifiers = if mods_changed {
            Some(guard.serialize_modifiers())
        } else {
            None
        };
        let wl_state = match state {
            KeyState::Pressed => WlKeyState::Pressed,
            KeyState::Released => WlKeyState::Released,
        };
        guard.with_grab(
            move |mut handle, grab| {
                grab.input(dh, &mut handle, keycode, wl_state, modifiers, serial, time);
            },
            self.arc.logger.clone(),
        );
        if guard.focus.is_some() {
            trace!(self.arc.logger, "Input forwarded to client");
        } else {
            trace!(self.arc.logger, "No client currently focused");
        }

        None
    }

    /// Set the current focus of this keyboard
    ///
    /// If the new focus is different from the previous one, any previous focus
    /// will be sent a [`wl_keyboard::Event::Leave`](wayland_server::protocol::wl_keyboard::Event::Leave)
    /// event, and if the new focus is not `None`,
    /// a [`wl_keyboard::Event::Enter`](wayland_server::protocol::wl_keyboard::Event::Enter) event will be sent.
    pub fn set_focus(&self, dh: &mut DisplayHandle<'_>, focus: Option<&WlSurface>, serial: Serial) {
        let mut guard = self.arc.internal.lock().unwrap();
        guard.pending_focus = focus.cloned();
        guard.with_grab(
            move |mut handle, grab| {
                grab.set_focus(dh, &mut handle, focus, serial);
            },
            self.arc.logger.clone(),
        );
    }

    /// Check if given client currently has keyboard focus
    pub fn has_focus(&self, _client: &Client) -> bool {
        todo!("has_focus");
        // let client_id = client.id();

        // self.arc
        //     .internal
        //     .lock()
        //     .unwrap()
        //     .focus
        //     .as_ref()
        //     .and_then(|f| f.id().client())
        //     .map(|c| c.equals(client))
        //     .unwrap_or(false)
    }

    /// Check if keyboard has focus
    pub fn is_focused(&self) -> bool {
        self.arc.internal.lock().unwrap().focus.is_some()
    }

    /// Register a new keyboard to this handler
    ///
    /// The keymap will automatically be sent to it
    ///
    /// This should be done first, before anything else is done with this keyboard.
    pub(crate) fn new_kbd(&self, dh: &mut DisplayHandle<'_>, kbd: WlKeyboard) {
        trace!(self.arc.logger, "Sending keymap to client");

        // prepare a tempfile with the keymap, to send it to the client
        let ret = tempfile().and_then(|mut f| {
            f.write_all(self.arc.keymap.as_bytes())?;
            f.flush()?;
            kbd.keymap(
                dh,
                KeymapFormat::XkbV1,
                f.as_raw_fd(),
                self.arc.keymap.as_bytes().len() as u32,
            );
            Ok(())
        });

        if let Err(e) = ret {
            warn!(self.arc.logger,
                "Failed write keymap to client in a tempfile";
                "err" => format!("{:?}", e)
            );
            return;
        };

        let mut guard = self.arc.internal.lock().unwrap();
        if kbd.version() >= 4 {
            kbd.repeat_info(dh, guard.repeat_rate, guard.repeat_delay);
        }
        guard.known_kbds.push(kbd);
    }

    /// Change the repeat info configured for this keyboard
    pub fn change_repeat_info(&self, dh: &mut DisplayHandle<'_>, rate: i32, delay: i32) {
        let mut guard = self.arc.internal.lock().unwrap();
        guard.repeat_delay = delay;
        guard.repeat_rate = rate;
        for kbd in &guard.known_kbds {
            kbd.repeat_info(dh, rate, delay);
        }
    }
}

/// User data for keyboard
#[derive(Debug)]
pub struct KeyboardUserData {
    pub(crate) handle: Option<KeyboardHandle>,
}

impl<T> DelegateDispatchBase<WlKeyboard> for SeatState<T> {
    type UserData = KeyboardUserData;
}

impl<T, D> DelegateDispatch<WlKeyboard, D> for SeatState<T>
where
    D: 'static + Dispatch<WlKeyboard, UserData = KeyboardUserData>,
    D: SeatHandler<T>,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlKeyboard,
        _request: wl_keyboard::Request,
        _data: &Self::UserData,
        _dhandle: &mut DisplayHandle<'_>,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client_id: ClientId, object_id: ObjectId, data: &Self::UserData) {
        if let Some(ref handle) = data.handle {
            handle
                .arc
                .internal
                .lock()
                .unwrap()
                .known_kbds
                .retain(|k| k.id() != object_id)
        }
    }
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
    pub fn unset_grab(&mut self, dh: &mut DisplayHandle<'_>, serial: Serial, restore_focus: bool) {
        self.inner.grab = GrabStatus::None;
        // restore the focus
        if restore_focus {
            let focus = self.inner.pending_focus.clone();
            self.set_focus(dh, focus.as_ref(), serial);
        }
    }

    /// Access the current focus of this keyboard
    pub fn current_focus(&self) -> Option<&WlSurface> {
        self.inner.focus.as_ref()
    }

    /// Send the input to the focused keyboards
    pub fn input(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        keycode: u32,
        key_state: WlKeyState,
        modifiers: Option<(u32, u32, u32, u32)>,
        serial: Serial,
        time: u32,
    ) {
        self.inner.with_focused_kbds(|kbd, _| {
            // key event must be sent before modifers event for libxkbcommon
            // to process them correctly
            kbd.key(dh, serial.into(), time, keycode, key_state);
            if let Some((dep, la, lo, gr)) = modifiers {
                kbd.modifiers(dh, serial.into(), dep, la, lo, gr);
            }
        });
    }

    /// Set the current focus of this keyboard
    ///
    /// If the new focus is different from the previous one, any previous focus
    /// will be sent a [`wl_keyboard::Event::Leave`](wayland_server::protocol::wl_keyboard::Event::Leave)
    /// event, and if the new focus is not `None`,
    /// a [`wl_keyboard::Event::Enter`](wayland_server::protocol::wl_keyboard::Event::Enter) event will be sent.
    pub fn set_focus(&mut self, dh: &mut DisplayHandle<'_>, focus: Option<&WlSurface>, serial: Serial) {
        let same = self
            .inner
            .focus
            .as_ref()
            .and_then(|f| focus.map(|s| s == f))
            .unwrap_or(false);

        if !same {
            // unset old focus
            self.inner.with_focused_kbds(|kbd, s| {
                kbd.leave(dh, serial.into(), s);
            });

            // set new focus
            self.inner.focus = focus.cloned();
            let (dep, la, lo, gr) = self.inner.serialize_modifiers();
            let keys = self.inner.serialize_pressed_keys();
            self.inner.with_focused_kbds(|kbd, surface| {
                kbd.enter(dh, serial.into(), surface, keys.clone());
                // Modifiers must be send after enter event.
                kbd.modifiers(dh, serial.into(), dep, la, lo, gr);
            });
            {
                let KbdInternal {
                    ref focus,
                    ref mut focus_hook,
                    ..
                } = *self.inner;
                focus_hook(focus.as_ref());
            }
            if self.inner.focus.is_some() {
                trace!(self.logger, "Focus set to new surface");
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
        dh: &mut DisplayHandle<'_>,
        handle: &mut KeyboardInnerHandle<'_>,
        keycode: u32,
        key_state: WlKeyState,
        modifiers: Option<(u32, u32, u32, u32)>,
        serial: Serial,
        time: u32,
    ) {
        handle.input(dh, keycode, key_state, modifiers, serial, time)
    }

    fn set_focus(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut KeyboardInnerHandle<'_>,
        focus: Option<&WlSurface>,
        serial: Serial,
    ) {
        handle.set_focus(dh, focus, serial)
    }

    fn start_data(&self) -> &GrabStartData {
        unreachable!()
    }
}
