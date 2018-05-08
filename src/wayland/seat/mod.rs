//! Seat global utilities
//!
//! This module provides you with utilities for handling the seat globals
//! and the associated input wayland objects.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! use smithay::wayland::seat::Seat;
//!
//! # fn main(){
//! # let (mut display, event_loop) = wayland_server::Display::new();
//! // insert the seat:
//! let (seat, seat_global) = Seat::new(
//!     &mut display, // the display
//!     event_loop.token(), // a LoopToken
//!     "seat-0".into(), // the name of the seat, will be advertize to clients
//!     None /* insert a logger here*/
//! );
//! # }
//! ```
//!
//! ### Run usage
//!
//! Once the seat is initialized, you can add capabilities to it.
//!
//! Currently, only pointer and keyboard capabilities are supported by
//! smithay.
//!
//! You can add these capabilities via methods of the `Seat` struct:
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! #
//! # use smithay::wayland::seat::Seat;
//! #
//! # fn main(){
//! # let (mut display, event_loop) = wayland_server::Display::new();
//! # let (mut seat, seat_global) = Seat::new(
//! #     &mut display,
//! #     event_loop.token(),
//! #     "seat-0".into(), // the name of the seat, will be advertize to clients
//! #     None /* insert a logger here*/
//! # );
//! let pointer_handle = seat.add_pointer();
//! # }
//! ```
//!
//! These handles can be cloned and sent accross thread, so you can keep one around
//! in your event-handling code to forward inputs to your clients.

use std::sync::{Arc, Mutex};

mod keyboard;
mod pointer;

pub use self::keyboard::{keysyms, Error as KeyboardError, KeyboardHandle, Keysym, ModifiersState};
pub use self::pointer::{PointerAxisHandle, PointerHandle};
use wayland_server::{Display, Global, LoopToken, NewResource, Resource};
use wayland_server::protocol::wl_seat;

struct Inner {
    log: ::slog::Logger,
    name: String,
    pointer: Option<PointerHandle>,
    keyboard: Option<KeyboardHandle>,
    known_seats: Vec<Resource<wl_seat::WlSeat>>,
}

impl Inner {
    fn compute_caps(&self) -> wl_seat::Capability {
        let mut caps = wl_seat::Capability::empty();
        if self.pointer.is_some() {
            caps |= wl_seat::Capability::Pointer;
        }
        if self.keyboard.is_some() {
            caps |= wl_seat::Capability::Keyboard;
        }
        caps
    }

    fn send_all_caps(&self) {
        let capabilities = self.compute_caps();
        for seat in &self.known_seats {
            seat.send(wl_seat::Event::Capabilities { capabilities });
        }
    }
}

/// A Seat handle
///
/// This struct gives you access to the control of the
/// capabilities of the associated seat.
///
/// It is directly inserted in the event loop by its `new` method.
///
/// See module-level documentation for details of use.
pub struct Seat {
    inner: Arc<Mutex<Inner>>,
}

impl Seat {
    /// Create a new seat global
    ///
    /// A new seat global is created with given name and inserted
    /// into this event loop.
    ///
    /// You are provided with the state token to retrieve it (allowing
    /// you to add or remove capabilities from it), and the global handle,
    /// in case you want to remove it.
    pub fn new<L>(
        display: &mut Display,
        token: LoopToken,
        name: String,
        logger: L,
    ) -> (Seat, Global<wl_seat::WlSeat>)
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger);
        let inner = Arc::new(Mutex::new(Inner {
            log: log.new(o!("smithay_module" => "seat_handler", "seat_name" => name.clone())),
            name: name,
            pointer: None,
            keyboard: None,
            known_seats: Vec::new(),
        }));
        let seat = Seat {
            inner: inner.clone(),
        };
        let global = display.create_global(&token, 5, move |_version, new_seat| {
            let seat = implement_seat(new_seat, inner.clone());
            let mut inner = inner.lock().unwrap();
            if seat.version() >= 2 {
                seat.send(wl_seat::Event::Name {
                    name: inner.name.clone(),
                });
            }
            seat.send(wl_seat::Event::Capabilities {
                capabilities: inner.compute_caps(),
            });
            inner.known_seats.push(seat);
        });
        (seat, global)
    }

    /// Adds the pointer capability to this seat
    ///
    /// You are provided a `PointerHandle`, which allows you to send input events
    /// to this keyboard. This handle can be cloned and sent accross threads.
    ///
    /// Calling this method on a seat that already has a pointer capability
    /// will overwrite it, and will be seen by the clients as if the
    /// mouse was unplugged and a new one was plugged.
    pub fn add_pointer(&mut self) -> PointerHandle {
        let mut inner = self.inner.lock().unwrap();
        let pointer = self::pointer::create_pointer_handler();
        if inner.pointer.is_some() {
            // there is already a pointer, remove it and notify the clients
            // of the change
            inner.pointer = None;
            inner.send_all_caps();
        }
        inner.pointer = Some(pointer.clone());
        inner.send_all_caps();
        pointer
    }

    /// Remove the pointer capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_pointer(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.pointer.is_some() {
            inner.pointer = None;
            inner.send_all_caps();
        }
    }

    /// Adds the keyboard capability to this seat
    ///
    /// You are provided a `KbdHandle`, which allows you to send input events
    /// to this keyboard. This handle can be cloned and sent accross threads.
    ///
    /// You also provide a Model/Layout/Variant/Options specification of the
    /// keymap to be used for this keyboard, as well as any repeat-info that
    /// will be forwarded to the clients.
    ///
    /// Calling this method on a seat that already has a keyboard capability
    /// will overwrite it, and will be seen by the clients as if the
    /// keyboard was unplugged and a new one was plugged.
    pub fn add_keyboard(
        &mut self,
        model: &str,
        layout: &str,
        variant: &str,
        options: Option<String>,
        repeat_delay: i32,
        repeat_rate: i32,
    ) -> Result<KeyboardHandle, KeyboardError> {
        let mut inner = self.inner.lock().unwrap();
        let keyboard = self::keyboard::create_keyboard_handler(
            "evdev", // we need this one
            model,
            layout,
            variant,
            options,
            repeat_delay,
            repeat_rate,
            &inner.log,
        )?;
        if inner.keyboard.is_some() {
            // there is already a keyboard, remove it and notify the clients
            // of the change
            inner.keyboard = None;
            inner.send_all_caps();
        }
        inner.keyboard = Some(keyboard.clone());
        inner.send_all_caps();
        Ok(keyboard)
    }

    /// Remove the keyboard capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_keyboard(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.keyboard.is_some() {
            inner.keyboard = None;
            inner.send_all_caps();
        }
    }

    /// Checks wether a given `WlSeat` is associated with this `Seat`
    pub fn owns(&self, seat: &Resource<wl_seat::WlSeat>) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.known_seats.iter().any(|s| s.equals(seat))
    }
}

fn implement_seat(
    new_seat: NewResource<wl_seat::WlSeat>,
    inner: Arc<Mutex<Inner>>,
) -> Resource<wl_seat::WlSeat> {
    let dest_inner = inner.clone();
    new_seat.implement(
        move |request, _seat| {
            let inner = inner.lock().unwrap();
            match request {
                wl_seat::Request::GetPointer { id } => {
                    let pointer = self::pointer::implement_pointer(id, inner.pointer.as_ref());
                    if let Some(ref ptr_handle) = inner.pointer {
                        ptr_handle.new_pointer(pointer);
                    } else {
                        // we should send a protocol error... but the protocol does not allow
                        // us, so this pointer will just remain inactive ¯\_(ツ)_/¯
                    }
                }
                wl_seat::Request::GetKeyboard { id } => {
                    let keyboard = self::keyboard::implement_keyboard(id, inner.keyboard.as_ref());
                    if let Some(ref kbd_handle) = inner.keyboard {
                        kbd_handle.new_kbd(keyboard);
                    } else {
                        // same as pointer, should error but cannot
                    }
                }
                wl_seat::Request::GetTouch { id: _ } => {
                    // TODO
                }
                wl_seat::Request::Release => {
                    // Our destructors already handle it
                }
            }
        },
        Some(move |seat, _| {
            dest_inner
                .lock()
                .unwrap()
                .known_seats
                .retain(|s| !s.equals(&seat));
        }),
    )
}
