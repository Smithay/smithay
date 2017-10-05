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
//!
//! use smithay::wayland::seat::Seat;
//!
//! # fn main(){
//! # let (_display, mut event_loop) = wayland_server::create_display();
//! // insert the seat:
//! let (seat_state_token, seat_global) = Seat::new(
//!     &mut event_loop,
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
//! You can add these capabilities via methods of the `Seat` struct that was
//! inserted in the event loop, that you can retreive via its token:
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! #
//! # use smithay::wayland::seat::Seat;
//! #
//! # fn main(){
//! # let (_display, mut event_loop) = wayland_server::create_display();
//! # let (seat_state_token, seat_global) = Seat::new(
//! #     &mut event_loop,
//! #     "seat-0".into(), // the name of the seat, will be advertize to clients
//! #     None /* insert a logger here*/
//! # );
//! let pointer_handle = event_loop.state().get_mut(&seat_state_token).add_pointer();
//! # }
//! ```
//!
//! These handles can be cloned and sent accross thread, so you can keep one around
//! in your event-handling code to forward inputs to your clients.

mod keyboard;
mod pointer;

pub use self::keyboard::{Error as KeyboardError, KeyboardHandle};
pub use self::pointer::PointerHandle;
use wayland_server::{Client, EventLoop, EventLoopHandle, Global, Liveness, Resource, StateToken};
use wayland_server::protocol::{wl_keyboard, wl_pointer, wl_seat};

/// Internal data of a seat global
///
/// This struct gives you access to the control of the
/// capabilities of the associated seat.
///
/// It is directly inserted in the event loop by its `new` method.
///
/// See module-level documentation for details of use.
pub struct Seat {
    log: ::slog::Logger,
    name: String,
    pointer: Option<PointerHandle>,
    keyboard: Option<KeyboardHandle>,
    known_seats: Vec<wl_seat::WlSeat>,
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
    pub fn new<L>(evl: &mut EventLoop, name: String, logger: L)
                  -> (StateToken<Seat>, Global<wl_seat::WlSeat, StateToken<Seat>>)
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger);
        let seat = Seat {
            log: log.new(o!("smithay_module" => "seat_handler", "seat_name" => name.clone())),
            name: name,
            pointer: None,
            keyboard: None,
            known_seats: Vec::new(),
        };
        let token = evl.state().insert(seat);
        // TODO: support version 5 (axis)
        let global = evl.register_global(4, seat_global_bind, token.clone());
        (token, global)
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
        let pointer = self::pointer::create_pointer_handler();
        if self.pointer.is_some() {
            // there is already a pointer, remove it and notify the clients
            // of the change
            self.pointer = None;
            let caps = self.compute_caps();
            for seat in &self.known_seats {
                seat.capabilities(caps);
            }
        }
        self.pointer = Some(pointer.clone());
        let caps = self.compute_caps();
        for seat in &self.known_seats {
            seat.capabilities(caps);
        }
        pointer
    }

    /// Remove the pointer capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_pointer(&mut self) {
        if self.pointer.is_some() {
            self.pointer = None;
            let caps = self.compute_caps();
            for seat in &self.known_seats {
                seat.capabilities(caps);
            }
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
    pub fn add_keyboard(&mut self, model: &str, layout: &str, variant: &str, options: Option<String>,
                        repeat_delay: i32, repeat_rate: i32)
                        -> Result<KeyboardHandle, KeyboardError> {
        let keyboard = self::keyboard::create_keyboard_handler(
            "evdev", // we need this one
            model,
            layout,
            variant,
            options,
            repeat_delay,
            repeat_rate,
            &self.log,
        )?;
        if self.keyboard.is_some() {
            // there is already a keyboard, remove it and notify the clients
            // of the change
            self.keyboard = None;
            let caps = self.compute_caps();
            for seat in &self.known_seats {
                seat.capabilities(caps);
            }
        }
        self.keyboard = Some(keyboard.clone());
        let caps = self.compute_caps();
        for seat in &self.known_seats {
            seat.capabilities(caps);
        }
        Ok(keyboard)
    }

    /// Remove the keyboard capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_keyboard(&mut self) {
        if self.keyboard.is_some() {
            self.keyboard = None;
            let caps = self.compute_caps();
            for seat in &self.known_seats {
                seat.capabilities(caps);
            }
        }
    }

    /// Cleanup internal states from old resources
    ///
    /// Deletes all remnnant of ressources from clients that
    /// are now disconnected.
    ///
    /// It can be wise to run this from time to time.
    pub fn cleanup(&mut self) {
        if let Some(ref pointer) = self.pointer {
            pointer.cleanup_old_pointers();
        }
        if let Some(ref kbd) = self.keyboard {
            kbd.cleanup_old_kbds();
        }
        self.known_seats.retain(|s| s.status() == Liveness::Alive);
    }

    fn compute_caps(&self) -> wl_seat::Capability {
        let mut caps = wl_seat::Capability::empty();
        if self.pointer.is_some() {
            caps |= wl_seat::Pointer;
        }
        if self.keyboard.is_some() {
            caps |= wl_seat::Keyboard;
        }
        caps
    }
}

fn seat_global_bind(evlh: &mut EventLoopHandle, token: &mut StateToken<Seat>, _: &Client,
                    seat: wl_seat::WlSeat) {
    evlh.register(&seat, seat_implementation(), token.clone(), None);
    let seat_mgr = evlh.state().get_mut(token);
    seat.name(seat_mgr.name.clone());
    seat.capabilities(seat_mgr.compute_caps());
    seat_mgr.known_seats.push(seat);
}

fn seat_implementation() -> wl_seat::Implementation<StateToken<Seat>> {
    wl_seat::Implementation {
        get_pointer: |evlh, token, _, _, pointer| {
            evlh.register(&pointer, pointer_implementation(), (), None);
            if let Some(ref ptr_handle) = evlh.state().get(token).pointer {
                ptr_handle.new_pointer(pointer);
            } else {
                // we should send a protocol error... but the protocol does not allow
                // us, so this pointer will just remain inactive ¯\_(ツ)_/¯
            }
        },
        get_keyboard: |evlh, token, _, _, keyboard| {
            evlh.register(&keyboard, keyboard_implementation(), (), None);
            if let Some(ref kbd_handle) = evlh.state().get(token).keyboard {
                kbd_handle.new_kbd(keyboard);
            } else {
                // same, should error but cant
            }
        },
        get_touch: |_evlh, _token, _, _, _touch| {
            // TODO
        },
        release: |_, _, _, _| {},
    }
}

fn pointer_implementation() -> wl_pointer::Implementation<()> {
    wl_pointer::Implementation {
        set_cursor: |_, _, _, _, _, _, _, _| {},
        release: |_, _, _, _| {},
    }
}

fn keyboard_implementation() -> wl_keyboard::Implementation<()> {
    wl_keyboard::Implementation {
        release: |_, _, _, _| {},
    }
}
