//! Utilities for text input support
//!
//! This module provides you with utilities to handle text input from multiple text input clients,
//! it must be used in conjunction with the input method module to work.
//! See the input method module for more information.
//! 
//! ##How to Initialize
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! # use smithay::wayland::compositor::compositor_init;
//!
//! use smithay::wayland::seat::{Seat};
//! use smithay::wayland::text_input::{init_text_input_manager_global, TextInputSeatTrait, TextInputHandle};
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
//! init_text_input_manager_global(&mut display.borrow_mut());
//! 
//! // Add the input method handle to the seat
//! let text_input = seat.add_text_input();
//! 
//! ```
//! 
//! ## Run usage
//! Once a handle has been added to the seat you need to use to use the set_focus function to 
//! tell the text input handle which surface should receive text input.
//! "self" is the compositor state.
//! 
//! ```
//! self.text_input
//!     .set_focus(under.as_ref().map(|&(ref s, _)| s));
//! 
//! ```

use std::{rc::Rc, cell::RefCell, convert::TryInto};

use wayland_protocols::{unstable::text_input::v3::server::{
    zwp_text_input_manager_v3::{ZwpTextInputManagerV3, self}, 
    zwp_text_input_v3::{ZwpTextInputV3, self}}};
use wayland_server::{Display, Global, Main, Filter, protocol::wl_surface::WlSurface};

use crate::wayland::seat::Seat;

use super::input_method::InputMethodHandle;

const TEXT_INPUT_VERSION: u32 = 1;

/// Contains all the text input instances
#[derive(Default, Clone, Debug)]
struct TextInput {
    instances: Vec<Main<ZwpTextInputV3>>,
    focus: Option<WlSurface>,
}

impl TextInput {
    fn set_focus(&mut self, focus: Option<&WlSurface>) {
        let same = self
            .focus
            .as_ref()
            .and_then(|f| focus.map(|s| s.as_ref().equals(f.as_ref())))
            .unwrap_or(false);
        if !same {
            // unset old focus
            if let Some(focus) = self.focus.as_ref() {
                if let Some(old_instance) = &self
                    .instances
                    .iter()
                    .find(|i| i.as_ref().same_client_as(self.focus.as_ref().unwrap().as_ref()))
                {
                    old_instance.leave(focus);
                }
            }
            self.focus = None;
            
            // set new focus
            self.focus = focus.cloned();
            if let Some(focus) = &self.focus {
                if let Some(instance) = &self
                    .instances
                    .iter()
                    .find(|i| i.as_ref().same_client_as(focus.as_ref()))
                {
                    instance.enter(&focus);
                }
            }  
        }
    }

    fn focused_text_input(&self) -> Option<Main<ZwpTextInputV3>> {
        if let Some(focus) = &self.focus {
            if let Some(instance) = self
                .instances
                .iter()
                .find(|i| i.as_ref().same_client_as(focus.as_ref()))
            {
                Some(instance.clone())
            } else {
                None
            }
        } else {
            None
        }
    }
}

///Handle to a text input
#[derive(Default, Debug, Clone)]
pub struct TextInputHandle {
    inner: Rc<RefCell<TextInput>>,
}

impl TextInputHandle {
    fn add_instance(&self, instance: Main<ZwpTextInputV3>){
        let mut inner = self.inner.borrow_mut();
        inner.instances.push(instance);
    }

    /// Activates a text input when a surface is focused and deactivates it when 
    /// the current surface goes out of focus.
    pub fn set_focus(&mut self, focus: Option<&WlSurface>) {
        let mut inner = self.inner.borrow_mut();
        inner.set_focus(focus);
    }

    /// used to access the Main handle from an input method
    pub fn handle(&self) -> Option<Main<ZwpTextInputV3>> {
        self.inner.borrow().focused_text_input()
    }
}

/// Extend [Seat] with text input specific functionality
pub trait TextInputSeatTrait {
    /// Get text input associated with this seat
    fn add_text_input(&self) -> TextInputHandle;
}

impl TextInputSeatTrait for Seat {
    fn add_text_input(&self) -> TextInputHandle {
        let user_data = self.user_data();
        user_data.insert_if_missing(|| TextInputHandle::default());
        user_data.get::<TextInputHandle>().unwrap().clone()
    }
}

/// Initialize a text input global
pub fn init_text_input_manager_global(display: &mut Display) -> Global<ZwpTextInputManagerV3> {
    display.create_global::<ZwpTextInputManagerV3, _>(
        TEXT_INPUT_VERSION, 
        Filter::new(
            move |(manager, _version): (Main<ZwpTextInputManagerV3>, u32), _, _| {
                manager.quick_assign(|_manager, req, _| match req {
                    zwp_text_input_manager_v3::Request::GetTextInput { seat, id } => {
                        let seat = Seat::from_resource(&seat).unwrap();
                        let user_data = seat.user_data();
                        user_data.insert_if_missing(|| TextInputHandle::default());
                        let ti = user_data.get::<TextInputHandle>().unwrap().clone();
                        ti.add_instance(id.clone());
                        let input_method = user_data.get::<InputMethodHandle>().unwrap().clone();
                        id.quick_assign(move |_text_input, req, _| match req {
                            zwp_text_input_v3::Request::Enable => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.activate();
                                }
                            }
                            zwp_text_input_v3::Request::Disable => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.deactivate();
                                }
                            }
                            zwp_text_input_v3::Request::SetSurroundingText { text, cursor, anchor} => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.surrounding_text(text, cursor.try_into().unwrap(), anchor.try_into().unwrap());
                                }
                            }
                            zwp_text_input_v3::Request::SetContentType { hint, purpose} => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.content_type(hint, purpose);
                                }
                            }
                            zwp_text_input_v3::Request::SetTextChangeCause { cause } => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.text_change_cause(cause);
                                }
                            }
                            zwp_text_input_v3::Request::SetCursorRectangle { x, y, width, height} => {
                                if let Some(popup_surface) = input_method.popup_surface() {
                                    popup_surface.text_input_rectangle(x, y, width, height);
                                }
                            }
                            zwp_text_input_v3::Request::Commit => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.done()
                                }
                            }
                            zwp_text_input_v3::Request::Destroy => {}
                            _ => {}
                        });
                        id.assign_destructor(Filter::new(move |text_input: ZwpTextInputV3, _, _|
                            ti
                                .inner
                                .borrow_mut()
                                .instances
                                .retain(|ti| !ti.as_ref().equals(text_input.as_ref()))));
                    }
                    zwp_text_input_manager_v3::Request::Destroy => {
                        //Nothing to do
                    }
                    _ => {}
                })
            }
        ))
}