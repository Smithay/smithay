//! Utilities for input method support
//!
//! This module provides you with utilities to handle input methods,
//! it must be used in conjunction with the text input module to work.
//! See the text input module for more information.
//!
//! ##How to Initialize
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! # use smithay::wayland::compositor::compositor_init;
//!
//! use std::borrow::BorrowMut;
//!
//! use smithay::wayland::seat::{Seat, XkbConfig};
//! use smithay::wayland::input_method::{init_input_method_manager_global, InputMethodHandle, InputMethodSeatTrait};
//!
//! # let mut display = wayland_server::Display::new();
//! # compositor_init(&mut display, |_, _| {}, None);
//! // First we need a regular seat
//! let (seat, seat_global) = Seat::new(
//!     &mut display,
//!     "seat-0".into(),
//!     None
//! );
//!
//! // Insert the manager into your event loop
//! init_input_method_manager_global(&mut display.borrow_mut());
//!
//! // Add the input method handle to the seat, 25 is the keyboard repeat rate, and 200 is the keyboard repeat delay.
//! // These are just arbitrary numbers and can be set to any real number
//! // The XkbConfig is the configuration of xkbcommon, sent to every new input method
//! let input_method = seat.add_input_method(25, 200, XkbConfig::default());
//!
//! ```
//!
//! ## Run usage
//! Once a handle has been added to the seat you need to wrap the keyboard input using
//! the keyboard_grabbed function.
//!

use std::{cell::RefCell, fmt, io::Write, os::unix::prelude::AsRawFd, rc::Rc};

use tempfile::tempfile;

use wayland_server::{
    protocol::{
        wl_keyboard::{KeyState as WlKeyState, KeymapFormat},
        wl_surface::WlSurface,
    },
    Display, Filter, Global, Main,
};

use wayland_protocols::misc::zwp_input_method_v2::server::{
    zwp_input_method_keyboard_grab_v2::{self, ZwpInputMethodKeyboardGrabV2},
    zwp_input_method_manager_v2::{self, ZwpInputMethodManagerV2},
    zwp_input_method_v2::{self, ZwpInputMethodV2},
    zwp_input_popup_surface_v2::{self, ZwpInputPopupSurfaceV2},
};

use xkbcommon::xkb;

use crate::{backend::input::KeyState, wayland::seat::Seat};

use super::{seat::XkbConfig, text_input::TextInputHandle, Serial};

const INPUT_METHOD_VERSION: u32 = 1;

#[derive(Clone)]
struct KeyboardState {
    xkbstate: xkb::State,
    rate: i32,
    delay: i32,
    keymap: String,
}

impl fmt::Debug for KeyboardState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyboardState")
            .field("rate", &self.rate)
            .field("delay", &self.delay)
            .field("keymap", &self.keymap)
            .finish()
    }
}

#[derive(Default, Clone, Debug)]
struct InputMethod {
    keyboard: Option<Main<ZwpInputMethodKeyboardGrabV2>>,
    instance: Option<Main<ZwpInputMethodV2>>,
    popup_surface_handle: Option<Main<ZwpInputPopupSurfaceV2>>,
    popup_surface: Option<WlSurface>,
    keyboard_state: Option<KeyboardState>,
}

impl InputMethod {
    fn config(&mut self, delay: i32, rate: i32, xkb_config: XkbConfig<'_>) {
        let (keymap, xkbstate) = xkb_handler(xkb_config);
        self.keyboard_state = Some(KeyboardState {
            xkbstate,
            rate,
            delay,
            keymap,
        });
    }

    fn send_keyboard_info(&self) {
        let keyboard = self.keyboard.as_ref().unwrap();
        let keyboard_state = self.keyboard_state.as_ref().unwrap();

        keyboard.repeat_info(keyboard_state.rate, keyboard_state.delay);
        // prepare a tempfile with the keymap, to send it to the client
        tempfile()
            .and_then(|mut f| {
                f.write_all(keyboard_state.keymap.as_bytes())?;
                f.flush()?;
                keyboard.keymap(
                    KeymapFormat::XkbV1,
                    f.as_raw_fd(),
                    keyboard_state.keymap.as_bytes().len() as u32,
                );
                Ok(())
            })
            .expect("File not working!");
    }

    // return true if modifier state has changed
    fn key_input(&mut self, keycode: u32, keystate: KeyState) -> bool {
        // track pressed keys as xkbcommon does not seem to expose it :(
        let direction = match keystate {
            KeyState::Pressed => xkb::KeyDirection::Down,
            KeyState::Released => xkb::KeyDirection::Up,
        };

        // update state
        // Offset the keycode by 8, as the evdev XKB rules reflect X's
        // broken keycode system, which starts at 8.
        let state = &mut self.keyboard_state.as_mut().unwrap().xkbstate;
        let state_components = state.update_key(keycode + 8, direction);

        state_components != 0
    }

    fn serialize_modifiers(&self) -> (u32, u32, u32, u32) {
        let state = &self.keyboard_state.as_ref().unwrap().xkbstate;
        let mods_depressed = state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
        let mods_latched = state.serialize_mods(xkb::STATE_MODS_LATCHED);
        let mods_locked = state.serialize_mods(xkb::STATE_MODS_LOCKED);
        let layout_locked = state.serialize_layout(xkb::STATE_LAYOUT_LOCKED);

        (mods_depressed, mods_latched, mods_locked, layout_locked)
    }

    fn input(
        &self,
        keycode: u32,
        state: KeyState,
        serial: Serial,
        time: u32,
        update_modifiers: bool, //TODO: Add modifier handling from closure
                                //modifiers:ModifiersState
    ) {
        let keyboard = self.keyboard.as_ref().unwrap();
        let wl_state = match state {
            KeyState::Pressed => WlKeyState::Pressed,
            KeyState::Released => WlKeyState::Released,
        };
        keyboard.key(serial.into(), time, keycode, wl_state);

        if update_modifiers {
            let (dep, la, lo, gr) = self.serialize_modifiers();
            keyboard.modifiers(serial.into(), dep, la, lo, gr);
        }
    }
}

fn xkb_handler(xkb_config: XkbConfig<'_>) -> (String, xkb::State) {
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
    .unwrap();
    let state = xkb::State::new(&keymap);
    (keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1), state)
}

/// Handle to an input method
#[derive(Default, Debug, Clone)]
pub struct InputMethodHandle {
    inner: Rc<RefCell<InputMethod>>,
}

impl InputMethodHandle {
    fn new_handle(&self, delay: i32, rate: i32, xkb_config: XkbConfig<'_>) {
        let mut inner = self.inner.borrow_mut();
        inner.config(delay, rate, xkb_config)
    }

    fn add_instance(&self, instance: Main<ZwpInputMethodV2>) {
        let mut inner = self.inner.borrow_mut();
        inner.instance = Some(instance);
    }

    fn instance_unavailable(&self) -> bool {
        let inner = self.inner.borrow();
        inner.instance.is_some()
    }

    fn add_keyboard(&self, keyboard: Main<ZwpInputMethodKeyboardGrabV2>) {
        let mut inner = self.inner.borrow_mut();
        inner.keyboard = Some(keyboard);
        inner.send_keyboard_info();
    }

    fn add_popup_surface(&self, popup_surface: Main<ZwpInputPopupSurfaceV2>, surface: WlSurface) {
        let mut inner = self.inner.borrow_mut();
        inner.popup_surface_handle = Some(popup_surface);
        inner.popup_surface = Some(surface);
    }

    /// Takes keyboard input and sends it to the input method
    pub fn input(&self, keycode: u32, state: KeyState, serial: Serial, time: u32) {
        let mut inner = self.inner.borrow_mut();
        let update_modifiers = inner.key_input(keycode, state);
        inner.input(keycode, state, serial, time, update_modifiers)
    }

    /// Indicates that an input method has grabbed a keyboard
    pub fn keyboard_grabbed(&self) -> bool {
        let inner = self.inner.borrow_mut();
        inner.keyboard.is_some()
    }

    /// used to access the Main handle from text input
    pub fn handle(&self) -> Option<Main<ZwpInputMethodV2>> {
        self.inner.borrow().instance.clone()
    }

    /// used to access the Main popup surface handler from text input
    pub fn popup_surface_handle(&self) -> Option<Main<ZwpInputPopupSurfaceV2>> {
        self.inner.borrow().popup_surface_handle.clone()
    }

    /// used to access the surface for popup
    pub fn popup_surface(&self) -> Option<WlSurface> {
        self.inner.borrow().popup_surface.clone()
    }
}

/// Extend [Seat] with input method specific functionality
pub trait InputMethodSeatTrait {
    /// Get input method seat associated with this seat
    /// this is also used to set xkb config parameters that will be sent to the input method
    /// Input methods need different keyboard languages for different input methods
    /// E.g Norwegian pinyin user will want to use a nordic keyboard layout
    /// but a Taiwanese person will use their input method with a us layout
    fn add_input_method(
        &self,
        repeat_delay: i32,
        repeat_rate: i32,
        xkb_config: XkbConfig<'_>,
    ) -> InputMethodHandle;
}

impl InputMethodSeatTrait for Seat {
    fn add_input_method(
        &self,
        repeat_delay: i32,
        repeat_rate: i32,
        xkb_config: XkbConfig<'_>,
    ) -> InputMethodHandle {
        let user_data = self.user_data();
        user_data.insert_if_missing(InputMethodHandle::default);
        let im = user_data.get::<InputMethodHandle>().unwrap().clone();
        im.new_handle(repeat_rate, repeat_delay, xkb_config);
        im
    }
}

/// Initialize an input method global.
pub fn init_input_method_manager_global(display: &mut Display) -> Global<ZwpInputMethodManagerV2> {
    display.create_global::<ZwpInputMethodManagerV2, _>(
        INPUT_METHOD_VERSION,
        Filter::new(
            move |(manager, _version): (Main<ZwpInputMethodManagerV2>, u32), _, _| {
                manager.quick_assign(|_manager, req, _| match req {
                    zwp_input_method_manager_v2::Request::GetInputMethod { seat, input_method } => {
                        let seat = Seat::from_resource(&seat).unwrap();
                        let user_data = seat.user_data();
                        user_data.insert_if_missing(InputMethodHandle::default);
                        let im = user_data.get::<InputMethodHandle>().unwrap().clone();
                        if im.instance_unavailable() {
                            input_method.quick_assign(|_, _, _| {});
                            input_method.unavailable();
                        } else {
                            im.add_instance(input_method.clone());
                            user_data.insert_if_missing(TextInputHandle::default);
                            let text_input = user_data.get::<TextInputHandle>().unwrap().clone();
                            let input_method_handle = im.clone();
                            input_method.quick_assign(move |_input_method, req, _| match req {
                                zwp_input_method_v2::Request::CommitString { text } => {
                                    if let Some(text_input) = text_input.handle() {
                                        text_input.commit_string(Some(text));
                                    }
                                }
                                zwp_input_method_v2::Request::SetPreeditString {
                                    text,
                                    cursor_begin,
                                    cursor_end,
                                } => {
                                    if let Some(text_input) = text_input.handle() {
                                        text_input.preedit_string(Some(text), cursor_begin, cursor_end);
                                    }
                                }
                                zwp_input_method_v2::Request::DeleteSurroundingText {
                                    before_length,
                                    after_length,
                                } => {
                                    if let Some(text_input) = text_input.handle() {
                                        text_input.delete_surrounding_text(before_length, after_length);
                                    }
                                }
                                zwp_input_method_v2::Request::Commit { serial: _ } => {
                                    if let Some(text_input_handle) = text_input.handle() {
                                        text_input_handle.done(text_input.serial());
                                    }
                                }
                                zwp_input_method_v2::Request::GetInputPopupSurface { id, surface } => {
                                    im.add_popup_surface(id.clone(), surface);
                                    id.quick_assign(|_popup_surface, req, _| {
                                        if let zwp_input_popup_surface_v2::Request::Destroy = req {}
                                    });
                                    let input_method_handle = im.clone();
                                    id.assign_destructor(Filter::new(
                                        move |_popup_surface: ZwpInputPopupSurfaceV2, _, _| {
                                            input_method_handle.inner.borrow_mut().popup_surface_handle = None
                                        },
                                    ));
                                }
                                zwp_input_method_v2::Request::GrabKeyboard { keyboard } => {
                                    im.add_keyboard(keyboard.clone());
                                    keyboard.quick_assign(|_keyboard, req, _| {
                                        if let zwp_input_method_keyboard_grab_v2::Request::Release = req {}
                                    });
                                    let input_method_handle = im.clone();
                                    keyboard.assign_destructor(Filter::new(
                                        move |_keyboard: ZwpInputMethodKeyboardGrabV2, _, _| {
                                            input_method_handle.inner.borrow_mut().keyboard = None
                                        },
                                    ));
                                }
                                zwp_input_method_v2::Request::Destroy => {}
                                _ => {}
                            });
                            input_method.assign_destructor(Filter::new(
                                move |_input_method: ZwpInputMethodV2, _, _| {
                                    input_method_handle.inner.borrow_mut().instance = None
                                },
                            ))
                        }
                    }
                    zwp_input_method_manager_v2::Request::Destroy => {
                        // Nothing to do
                    }
                    _ => {}
                });
            },
        ),
    )
}
