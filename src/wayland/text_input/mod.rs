//! Utilities for text input support
//!
//! This module provides you with utilities to handle text input from multiple text input clients,
//! it must be used in conjunction with the input method module to work.
//! See the input method module for more information.
//!
//! ## How to use
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! # use smithay::wayland::compositor::compositor_init;
//!
//! use std::borrow::BorrowMut;
//!
//! use wayland_server::protocol::wl_surface::WlSurface;
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
//! ```
//! ## Run usage
//! // Once a handle has been added to the seat you need to use to use the set_focus function to
//! // tell the text input handle which surface should receive text input.
//!

use std::{cell::RefCell, convert::TryInto, rc::Rc};

use wayland_protocols::unstable::text_input::v3::server::{
    zwp_text_input_manager_v3::{self, ZwpTextInputManagerV3},
    zwp_text_input_v3::{self, ZwpTextInputV3},
};
use wayland_server::{protocol::wl_surface::WlSurface, Display, Filter, Global, Main};

use crate::{
    utils::{Logical, Point},
    wayland::seat::Seat,
};

use super::input_method::InputMethodHandle;

const TEXT_INPUT_VERSION: u32 = 1;

#[derive(Clone, Debug)]
struct Instance {
    handle: Main<ZwpTextInputV3>,
    serial: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    l_x: i32,
    l_y: i32,
}

/// Contains all the text input instances
#[derive(Default, Clone, Debug)]
struct TextInput {
    instances: Vec<Instance>,
    focus: Option<WlSurface>,
    old_focus: Option<WlSurface>,
}

impl TextInput {
    fn set_focus(&mut self, focus: Option<&WlSurface>) {
        let same = self
            .focus
            .as_ref()
            .and_then(|f| focus.map(|s| s.as_ref().equals(f.as_ref())))
            .unwrap_or(false);
        if !same {
            if let Some(focus) = self.focus.as_ref() {
                if let Some(old_instance) = self.instances.iter().find(|i| {
                    i.handle
                        .as_ref()
                        .same_client_as(self.focus.as_ref().unwrap().as_ref())
                }) {
                    old_instance.handle.leave(focus);
                    self.old_focus = Some(focus.clone());
                }
            }
            self.focus = None;
            // set new focus
            self.focus = focus.cloned();

            if self.old_focus.is_none() {
                if let Some(focus) = &self.focus {
                    if let Some(instance) = &self
                        .instances
                        .iter()
                        .find(|i| i.handle.as_ref().same_client_as(focus.as_ref()))
                    {
                        instance.handle.enter(focus);
                    }
                }
            }
        }
    }

    fn set_point(&mut self, point: &Point<i32, Logical>) {
        if let Some(instance) = self.focused_text_input() {
            instance.l_x = point.x;
            instance.l_y = point.y;
        }
    }

    fn increment(&mut self) {
        if let Some(old_focus) = &self.old_focus {
            if let Some(old_instance) = self
                .instances
                .iter_mut()
                .find(|i| i.handle.as_ref().same_client_as(old_focus.as_ref()))
            {
                old_instance.serial += 1;
                self.old_focus = None;
                if let Some(focus) = &self.focus {
                    if let Some(instance) = &self
                        .instances
                        .iter()
                        .find(|i| i.handle.as_ref().same_client_as(focus.as_ref()))
                    {
                        instance.handle.enter(focus);
                    }
                }
            }
        } else if let Some(focus) = &self.focus {
            if let Some(instance) = self
                .instances
                .iter_mut()
                .find(|i| i.handle.as_ref().same_client_as(focus.as_ref()))
            {
                instance.serial += 1;
            }
        }
    }

    fn focused_text_input(&mut self) -> Option<&mut Instance> {
        if let Some(focus) = &self.focus {
            self.instances
                .iter_mut()
                .find(|i| i.handle.as_ref().same_client_as(focus.as_ref()))
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
    fn add_instance(&self, instance: Instance) {
        let mut inner = self.inner.borrow_mut();
        inner.instances.push(instance);
    }

    fn add_coordinates(&self, x: i32, y: i32, width: i32, height: i32) {
        let mut inner = self.inner.borrow_mut();
        let focused_instance = inner.focused_text_input();
        if let Some(instance) = focused_instance {
            instance.x = x;
            instance.y = y;
            instance.width = width;
            instance.height = height;
        }
    }

    /// Used to access the relative location of an input popup surface
    pub fn coordinates(&self) -> (i32, i32, i32, i32) {
        let mut inner = self.inner.borrow_mut();
        let focused_instance = inner.focused_text_input();
        if let Some(instance) = focused_instance {
            (
                instance.x + instance.l_x,
                instance.y + instance.l_y,
                instance.width,
                instance.height,
            )
        } else {
            (0, 0, 0, 0)
        }
    }

    /// Activates a text input when a surface is focused and deactivates it when
    /// the current surface goes out of focus.
    pub fn set_focus(&mut self, focus: Option<&WlSurface>, location: Option<&Point<i32, Logical>>) {
        let mut inner = self.inner.borrow_mut();
        inner.set_focus(focus);
        if let Some(point) = location {
            inner.set_point(point);
        }
    }

    /// Used to access the Main handle from an input method
    pub fn handle(&self) -> Option<Main<ZwpTextInputV3>> {
        self.inner
            .borrow_mut()
            .focused_text_input()
            .map(|i| i.handle.clone())
    }

    /// Used to access serial for each individual text input.
    /// It is the compositors responsibility to increment a separate serial on each
    /// text input.
    pub fn serial(&self) -> u32 {
        self.inner
            .borrow_mut()
            .focused_text_input()
            .map(|i| i.serial)
            .expect("Got a message from a text input that does not exist!")
    }

    fn increment_serial(&mut self) {
        let mut inner = self.inner.borrow_mut();
        inner.increment();
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
        user_data.insert_if_missing(TextInputHandle::default);
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
                        user_data.insert_if_missing(TextInputHandle::default);
                        let mut ti = user_data.get::<TextInputHandle>().unwrap().clone();
                        ti.add_instance(Instance {
                            handle: id.clone(),
                            serial: 0,
                            x: 0,
                            y: 0,
                            width: 0,
                            height: 0,
                            l_x: 0,
                            l_y: 0,
                        });
                        let input_method = user_data.get::<InputMethodHandle>().unwrap().clone();
                        let text_input_handle = ti.clone();
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
                            zwp_text_input_v3::Request::SetSurroundingText { text, cursor, anchor } => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.surrounding_text(
                                        text,
                                        cursor.try_into().unwrap(),
                                        anchor.try_into().unwrap(),
                                    );
                                }
                            }
                            zwp_text_input_v3::Request::SetContentType { hint, purpose } => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.content_type(hint, purpose);
                                }
                            }
                            zwp_text_input_v3::Request::SetTextChangeCause { cause } => {
                                if let Some(input_method) = input_method.handle() {
                                    input_method.text_change_cause(cause);
                                }
                            }
                            zwp_text_input_v3::Request::SetCursorRectangle { x, y, width, height } => {
                                ti.add_coordinates(x, y, width, height);
                                if let Some(popup_surface) = input_method.popup_surface_handle() {
                                    popup_surface.text_input_rectangle(x, y, width, height);
                                }
                            }
                            zwp_text_input_v3::Request::Commit => {
                                ti.increment_serial();
                                if let Some(input_method) = input_method.handle() {
                                    input_method.done();
                                }
                            }
                            zwp_text_input_v3::Request::Destroy => {}
                            _ => {}
                        });
                        id.assign_destructor(Filter::new(move |text_input: ZwpTextInputV3, _, _| {
                            text_input_handle
                                .inner
                                .borrow_mut()
                                .instances
                                .retain(|ti| !ti.handle.as_ref().equals(text_input.as_ref()))
                        }));
                    }
                    zwp_text_input_manager_v3::Request::Destroy => {
                        //Nothing to do
                    }
                    _ => {}
                })
            },
        ),
    )
}
