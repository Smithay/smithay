//! Keyboard-related types for smithay's input abstraction

use crate::backend::input::KeyState;
use crate::utils::{IsAlive, Serial};
use slog::{debug, error, info, o, trace};
use std::{
    default::Default,
    ffi::CString,
    fmt, io,
    sync::{Arc, Mutex},
};
use thiserror::Error;

use xkbcommon::xkb;
pub use xkbcommon::xkb::{keysyms, Keysym};

use super::{Seat, SeatHandler};

mod keymap_file;
pub(crate) use keymap_file::KeymapFile;

mod modifiers_state;
pub use modifiers_state::ModifiersState;

mod xkb_config;
pub use xkb_config::XkbConfig;

/// Trait representing object that can receive keyboard interactions
pub trait KeyboardTarget<D>: IsAlive + PartialEq + Clone + Send
where
    D: SeatHandler,
{
    /// Keyboard focus of a given seat was assigned to this handler
    fn enter(&self, seat: &Seat<D>, data: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial);
    /// The keyboard focus of a given seat left this handler
    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial);
    /// A key was pressed on a keyboard from a given seat
    fn key(
        &self,
        seat: &Seat<D>,
        data: &mut D,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    );
    /// Hold modifiers were changed on a keyboard from a given seat
    fn modifiers(&self, seat: &Seat<D>, data: &mut D, modifiers: ModifiersState, serial: Serial);
}

enum GrabStatus<D> {
    None,
    Active(Serial, Box<dyn KeyboardGrab<D>>),
    Borrowed,
}

pub(crate) struct KbdInternal<D: SeatHandler> {
    pub(crate) focus: Option<(<D as SeatHandler>::KeyboardFocus, Serial)>,
    pending_focus: Option<<D as SeatHandler>::KeyboardFocus>,
    pub(crate) pressed_keys: Vec<u32>,
    pub(crate) mods_state: ModifiersState,
    keymap: xkb::Keymap,
    pub(crate) state: xkb::State,
    pub(crate) repeat_rate: i32,
    pub(crate) repeat_delay: i32,
    grab: GrabStatus<D>,
}

// focus_hook does not implement debug, so we have to impl Debug manually
impl<D: SeatHandler> fmt::Debug for KbdInternal<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KbdInternal")
            .field("focus", &self.focus.as_ref().map(|_| "..."))
            .field("pending_focus", &self.pending_focus.as_ref().map(|_| "..."))
            .field("pressed_keys", &self.pressed_keys)
            .field("mods_state", &self.mods_state)
            .field("keymap", &self.keymap.get_raw_ptr())
            .field("state", &self.state.get_raw_ptr())
            .field("repeat_rate", &self.repeat_rate)
            .field("repeat_delay", &self.repeat_delay)
            .finish()
    }
}

// This is OK because all parts of `xkb` will remain on the
// same thread
unsafe impl<D: SeatHandler> Send for KbdInternal<D> {}

impl<D: SeatHandler + 'static> KbdInternal<D> {
    fn new(xkb_config: XkbConfig<'_>, repeat_rate: i32, repeat_delay: i32) -> Result<KbdInternal<D>, ()> {
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

    fn with_grab<F>(&mut self, seat: &Seat<D>, f: F, logger: ::slog::Logger)
    where
        F: FnOnce(KeyboardInnerHandle<'_, D>, &mut dyn KeyboardGrab<D>),
    {
        let mut grab = ::std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => panic!("Accessed a keyboard grab from within a keyboard grab access."),
            GrabStatus::Active(_, ref mut handler) => {
                // If this grab is associated with a surface that is no longer alive, discard it
                if let Some(ref surface) = handler.start_data().focus {
                    if !surface.alive() {
                        self.grab = GrabStatus::None;
                        f(
                            KeyboardInnerHandle {
                                inner: self,
                                seat,
                                logger,
                            },
                            &mut DefaultGrab,
                        );
                        return;
                    }
                }
                f(
                    KeyboardInnerHandle {
                        inner: self,
                        seat,
                        logger,
                    },
                    &mut **handler,
                );
            }
            GrabStatus::None => {
                f(
                    KeyboardInnerHandle {
                        inner: self,
                        seat,
                        logger,
                    },
                    &mut DefaultGrab,
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

#[derive(Debug)]
pub(crate) struct KbdRc<D: SeatHandler> {
    pub(crate) internal: Mutex<KbdInternal<D>>,
    #[allow(dead_code)]
    pub(crate) keymap: KeymapFile,
    pub(crate) logger: ::slog::Logger,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) known_kbds: Mutex<Vec<wayland_server::protocol::wl_keyboard::WlKeyboard>>,
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
pub struct GrabStartData<D: SeatHandler> {
    /// The focused surface, if any, at the start of the grab.
    pub focus: Option<<D as SeatHandler>::KeyboardFocus>,
}

impl<D: SeatHandler + 'static> fmt::Debug for GrabStartData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrabStartData")
            .field("focus", &self.focus.as_ref().map(|_| "..."))
            .finish()
    }
}

impl<D: SeatHandler + 'static> Clone for GrabStartData<D> {
    fn clone(&self) -> Self {
        GrabStartData {
            focus: self.focus.clone(),
        }
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
pub trait KeyboardGrab<D: SeatHandler> {
    /// An input was reported
    #[allow(clippy::too_many_arguments)]
    fn input(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: u32,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    );

    /// A focus change was requested
    fn set_focus(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<<D as SeatHandler>::KeyboardFocus>,
        serial: Serial,
    );

    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData<D>;
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
#[derive(Debug)]
pub struct KeyboardHandle<D: SeatHandler> {
    pub(crate) arc: Arc<KbdRc<D>>,
}

impl<D: SeatHandler> Clone for KeyboardHandle<D> {
    fn clone(&self) -> Self {
        KeyboardHandle {
            arc: self.arc.clone(),
        }
    }
}

impl<D: SeatHandler> ::std::cmp::PartialEq for KeyboardHandle<D> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}

impl<D: SeatHandler + 'static> KeyboardHandle<D> {
    /// Create a keyboard handler from a set of RMLVO rules
    pub(crate) fn new(
        xkb_config: XkbConfig<'_>,
        repeat_delay: i32,
        repeat_rate: i32,
        logger: &::slog::Logger,
    ) -> Result<Self, Error> {
        let log = logger.new(o!("smithay_module" => "xkbcommon_handler"));
        info!(log, "Initializing a xkbcommon handler with keymap query";
            "rules" => xkb_config.rules, "model" => xkb_config.model, "layout" => xkb_config.layout,
            "variant" => xkb_config.variant, "options" => &xkb_config.options
        );
        let internal = KbdInternal::new(xkb_config, repeat_rate, repeat_delay).map_err(|_| {
            debug!(log, "Loading keymap failed");
            Error::BadKeymap
        })?;

        info!(log, "Loaded Keymap"; "name" => internal.keymap.layouts().next());

        let keymap = internal.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        let keymap = CString::new(keymap).expect("Keymap should not contain interior nul bytes");

        Ok(Self {
            arc: Arc::new(KbdRc {
                internal: Mutex::new(internal),
                keymap: KeymapFile::new(keymap, log.clone()),
                logger: log,
                #[cfg(feature = "wayland_frontend")]
                known_kbds: Mutex::new(Vec::new()),
            }),
        })
    }

    /// Change the current grab on this keyboard to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: KeyboardGrab<D> + 'static>(&self, grab: G, serial: Serial) {
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
    pub fn grab_start_data(&self) -> Option<GrabStartData<D>> {
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
        data: &mut D,
        keycode: u32,
        state: KeyState,
        serial: Serial,
        time: u32,
        filter: F,
    ) -> Option<T>
    where
        F: FnOnce(&mut D, &ModifiersState, KeysymHandle<'_>) -> FilterResult<T>,
    {
        trace!(self.arc.logger, "Handling keystroke"; "keycode" => keycode, "state" => format_args!("{:?}", state));
        let mut guard = self.arc.internal.lock().unwrap();
        let mods_changed = guard.key_input(keycode, state);
        let key_handle = KeysymHandle {
            // Offset the keycode by 8, as the evdev XKB rules reflect X's
            // broken keycode system, which starts at 8.
            keycode: keycode + 8,
            state: &guard.state,
            keymap: &guard.keymap,
        };

        trace!(self.arc.logger, "Calling input filter";
            "mods_state" => format_args!("{:?}", guard.mods_state), "sym" => xkb::keysym_get_name(key_handle.modified_sym())
        );

        if let FilterResult::Intercept(val) = filter(data, &guard.mods_state, key_handle) {
            // the filter returned false, we do not forward to client
            trace!(self.arc.logger, "Input was intercepted by filter");
            return Some(val);
        }

        // forward to client if no keybinding is triggered
        let seat = self.get_seat(data);
        let modifiers = mods_changed.then(|| guard.mods_state);
        guard.with_grab(
            &seat,
            move |mut handle, grab| {
                grab.input(data, &mut handle, keycode, state, modifiers, serial, time);
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
    pub fn set_focus(&self, data: &mut D, focus: Option<<D as SeatHandler>::KeyboardFocus>, serial: Serial) {
        let mut guard = self.arc.internal.lock().unwrap();
        guard.pending_focus = focus.clone();
        let seat = self.get_seat(data);
        guard.with_grab(
            &seat,
            move |mut handle, grab| {
                grab.set_focus(data, &mut handle, focus, serial);
            },
            self.arc.logger.clone(),
        );
    }

    /// Check if keyboard has focus
    pub fn is_focused(&self) -> bool {
        self.arc.internal.lock().unwrap().focus.is_some()
    }

    /// Change the repeat info configured for this keyboard
    pub fn change_repeat_info(&self, rate: i32, delay: i32) {
        let mut guard = self.arc.internal.lock().unwrap();
        guard.repeat_delay = delay;
        guard.repeat_rate = rate;
        #[cfg(feature = "wayland_frontend")]
        for kbd in &*self.arc.known_kbds.lock().unwrap() {
            kbd.repeat_info(rate, delay);
        }
    }

    fn get_seat(&self, data: &mut D) -> Seat<D> {
        let seat_state = data.seat_state();
        seat_state
            .seats
            .iter()
            .find(|seat| seat.get_keyboard().map(|h| &h == self).unwrap_or(false))
            .cloned()
            .unwrap()
    }
}

/// This inner handle is accessed from inside a keyboard grab logic, and directly
/// sends event to the client
#[derive(Debug)]
pub struct KeyboardInnerHandle<'a, D: SeatHandler> {
    inner: &'a mut KbdInternal<D>,
    seat: &'a Seat<D>,
    logger: ::slog::Logger,
}

impl<'a, D: SeatHandler + 'static> KeyboardInnerHandle<'a, D> {
    /// Change the current grab on this keyboard to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: KeyboardGrab<D> + 'static>(&mut self, _data: &mut D, serial: Serial, grab: G) {
        self.inner.grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this keyboard, resetting it to the default behavior
    ///
    /// This will also restore the focus of the underlying keyboard if restore_focus
    /// is [`true`]
    pub fn unset_grab(&mut self, data: &mut D, serial: Serial, restore_focus: bool) {
        self.inner.grab = GrabStatus::None;
        // restore the focus
        if restore_focus {
            let focus = self.inner.pending_focus.clone();
            self.set_focus(data, focus, serial);
        }
    }

    /// Access the current focus of this keyboard
    pub fn current_focus(&self) -> Option<&<D as SeatHandler>::KeyboardFocus> {
        self.inner.focus.as_ref().map(|f| &f.0)
    }

    /// Convert a given keycode as a [`KeysymHandle`] modified by this keyboards state
    pub fn keysym_handle(&self, keycode: u32) -> KeysymHandle<'_> {
        KeysymHandle {
            keycode: keycode + 8,
            state: &self.inner.state,
            keymap: &self.inner.keymap,
        }
    }

    /// Send the input to the focused keyboards
    pub fn input(
        &mut self,
        data: &mut D,
        keycode: u32,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        //TODO
        if let Some((focus, _)) = self.inner.focus.as_mut() {
            // key event must be sent before modifers event for libxkbcommon
            // to process them correctly
            let key = KeysymHandle {
                keycode: keycode + 8,
                state: &self.inner.state,
                keymap: &self.inner.keymap,
            };

            focus.key(self.seat, data, key, key_state, serial, time);
            if let Some(mods) = modifiers {
                focus.modifiers(self.seat, data, mods, serial);
            }
        };
    }

    /// Set the current focus of this keyboard
    ///
    /// If the new focus is different from the previous one, any previous focus
    /// will be sent a [`wl_keyboard::Event::Leave`](wayland_server::protocol::wl_keyboard::Event::Leave)
    /// event, and if the new focus is not `None`,
    /// a [`wl_keyboard::Event::Enter`](wayland_server::protocol::wl_keyboard::Event::Enter) event will be sent.
    pub fn set_focus(
        &mut self,
        data: &mut D,
        focus: Option<<D as SeatHandler>::KeyboardFocus>,
        serial: Serial,
    ) {
        let focus_clone = focus.clone();
        let same = self
            .inner
            .focus
            .as_ref()
            .and_then(|f| focus_clone.map(|f2| f.0 == f2))
            .unwrap_or(false);

        if !same {
            // unset old focus
            if let Some((focus, _)) = self.inner.focus.as_mut() {
                focus.leave(self.seat, data, serial);
            };

            // set new focus
            self.inner.focus = focus.map(|f| (f, serial));
            if let Some((focus, _)) = self.inner.focus.as_mut() {
                let keys = self
                    .inner
                    .pressed_keys
                    .iter()
                    .map(|keycode| {
                        KeysymHandle {
                            // Offset the keycode by 8, as the evdev XKB rules reflect X's
                            // broken keycode system, which starts at 8.
                            keycode: keycode + 8,
                            state: &self.inner.state,
                            keymap: &self.inner.keymap,
                        }
                    })
                    .collect();
                focus.enter(self.seat, data, keys, serial);
                focus.modifiers(self.seat, data, self.inner.mods_state, serial);
            };
            {
                let KbdInternal { ref focus, .. } = *self.inner;
                data.focus_changed(self.seat, focus.as_ref().map(|f| &f.0));
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

impl<D: SeatHandler + 'static> KeyboardGrab<D> for DefaultGrab {
    fn input(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: u32,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        handle.input(data, keycode, state, modifiers, serial, time)
    }

    fn set_focus(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<<D as SeatHandler>::KeyboardFocus>,
        serial: Serial,
    ) {
        handle.set_focus(data, focus, serial)
    }

    fn start_data(&self) -> &GrabStartData<D> {
        unreachable!()
    }
}
