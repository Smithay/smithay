//! Seat global utilities
//!
//! This module provides you with utilities for handling the seat globals
//! and the associated input Wayland objects.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! ```
//! # extern crate wayland_server;
//! use smithay::wayland::seat::Seat;
//!
//! # let mut display = wayland_server::Display::new();
//! // insert the seat:
//! let (seat, seat_global) = Seat::new(
//!     &mut display,             // the display
//!     "seat-0".into(),          // the name of the seat, will be advertized to clients
//!     None                      // insert a logger here
//! );
//! ```
//!
//! ### Run usage
//!
//! Once the seat is initialized, you can add capabilities to it.
//!
//! Currently, only pointer and keyboard capabilities are supported by smithay.
//!
//! You can add these capabilities via methods of the [`Seat`] struct:
//! [`Seat::add_keyboard`] and [`Seat::add_pointer`].
//! These methods return handles that can be cloned and sent across thread, so you can keep one around
//! in your event-handling code to forward inputs to your clients.
//!
//! This module further defines the `"cursor_image"` role, that is assigned to surfaces used by clients
//! to change the cursor icon.

use std::{cell::RefCell, fmt, ops::Deref as _, rc::Rc};

mod keyboard;
mod pointer;

pub use self::{
    keyboard::{keysyms, Error as KeyboardError, KeyboardHandle, Keysym, ModifiersState, XkbConfig},
    pointer::{
        AxisFrame, CursorImageAttributes, CursorImageStatus, GrabStartData, PointerGrab, PointerHandle,
        PointerInnerHandle,
    },
};

use wayland_server::{
    protocol::{wl_seat, wl_surface},
    Display, Filter, Global, Main, UserDataMap,
};

#[derive(Debug)]
struct Inner {
    pointer: Option<PointerHandle>,
    keyboard: Option<KeyboardHandle>,
    known_seats: Vec<wl_seat::WlSeat>,
}

pub(crate) struct SeatRc {
    inner: RefCell<Inner>,
    user_data: UserDataMap,
    pub(crate) log: ::slog::Logger,
    name: String,
}

// UserDataMap does not implement debug, so we have to impl Debug manually
impl fmt::Debug for SeatRc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeatRc")
            .field("inner", &self.inner)
            .field("user_data", &"...")
            .field("log", &self.log)
            .field("name", &self.name)
            .finish()
    }
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
            seat.capabilities(capabilities);
        }
    }
}

/// A Seat handle
///
/// This struct gives you access to the control of the
/// capabilities of the associated seat.
///
/// It is directly inserted in the wayland display by its [`new`](Seat::new) method.
///
/// This is an handle to the inner logic, it can be cloned.
///
/// See module-level documentation for details of use.
#[derive(Debug, Clone)]
pub struct Seat {
    pub(crate) arc: Rc<SeatRc>,
}

impl Seat {
    /// Create a new seat global
    ///
    /// A new seat global is created with given name and inserted
    /// into this wayland display.
    ///
    /// You are provided with the state token to retrieve it (allowing
    /// you to add or remove capabilities from it), and the global handle,
    /// in case you want to remove it.
    pub fn new<L>(display: &mut Display, name: String, logger: L) -> (Seat, Global<wl_seat::WlSeat>)
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger);
        let arc = Rc::new(SeatRc {
            inner: RefCell::new(Inner {
                pointer: None,
                keyboard: None,
                known_seats: Vec::new(),
            }),
            log: log.new(slog::o!("smithay_module" => "seat_handler", "seat_name" => name.clone())),
            name,
            user_data: UserDataMap::new(),
        });
        let seat = Seat { arc: arc.clone() };
        let global = display.create_global(
            5,
            Filter::new(move |(new_seat, _version), _, _| {
                let seat = implement_seat(new_seat, arc.clone());
                let mut inner = arc.inner.borrow_mut();
                if seat.as_ref().version() >= 2 {
                    seat.name(arc.name.clone());
                }
                seat.capabilities(inner.compute_caps());
                inner.known_seats.push(seat);
            }),
        );
        (seat, global)
    }

    /// Attempt to retrieve a [`Seat`] from an existing resource
    pub fn from_resource(seat: &wl_seat::WlSeat) -> Option<Seat> {
        seat.as_ref()
            .user_data()
            .get::<Rc<SeatRc>>()
            .cloned()
            .map(|arc| Seat { arc })
    }

    /// Access the `UserDataMap` associated with this `Seat`
    pub fn user_data(&self) -> &UserDataMap {
        &self.arc.user_data
    }

    /// Adds the pointer capability to this seat
    ///
    /// You are provided a [`PointerHandle`], which allows you to send input events
    /// to this pointer. This handle can be cloned.
    ///
    /// Calling this method on a seat that already has a pointer capability
    /// will overwrite it, and will be seen by the clients as if the
    /// mouse was unplugged and a new one was plugged.
    ///
    /// You need to provide a callback that will be notified whenever a client requests
    /// to set a custom cursor image.
    ///
    /// # Examples
    ///
    /// ```
    /// # extern crate wayland_server;
    /// #
    /// # use smithay::wayland::seat::Seat;
    /// #
    /// # let mut display = wayland_server::Display::new();
    /// # let (mut seat, seat_global) = Seat::new(
    /// #     &mut display,
    /// #     "seat-0".into(),
    /// #     None
    /// # );
    /// let pointer_handle = seat.add_pointer(
    ///     |new_status| { /* a closure handling requests from clients to change the cursor icon */ }
    /// );
    /// ```
    pub fn add_pointer<F>(&mut self, cb: F) -> PointerHandle
    where
        F: FnMut(CursorImageStatus) + 'static,
    {
        let mut inner = self.arc.inner.borrow_mut();
        let pointer = self::pointer::create_pointer_handler(cb);
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

    /// Access the pointer of this seat if any
    pub fn get_pointer(&self) -> Option<PointerHandle> {
        self.arc.inner.borrow_mut().pointer.clone()
    }

    /// Remove the pointer capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_pointer(&mut self) {
        let mut inner = self.arc.inner.borrow_mut();
        if inner.pointer.is_some() {
            inner.pointer = None;
            inner.send_all_caps();
        }
    }

    /// Adds the keyboard capability to this seat
    ///
    /// You are provided a [`KeyboardHandle`], which allows you to send input events
    /// to this keyboard. This handle can be cloned.
    ///
    /// You also provide a Model/Layout/Variant/Options specification of the
    /// keymap to be used for this keyboard, as well as any repeat-info that
    /// will be forwarded to the clients.
    ///
    /// Calling this method on a seat that already has a keyboard capability
    /// will overwrite it, and will be seen by the clients as if the
    /// keyboard was unplugged and a new one was plugged.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # extern crate smithay;
    /// # use smithay::wayland::seat::{Seat, XkbConfig};
    /// # let mut seat: Seat = unimplemented!();
    /// let keyboard = seat
    ///     .add_keyboard(
    ///         XkbConfig {
    ///             layout: "de",
    ///             variant: "nodeadkeys",
    ///             ..XkbConfig::default()
    ///         },
    ///         200,
    ///         25,
    ///         |seat, focus| {
    ///             /* This closure is called whenever the keyboard focus
    ///              * changes, with the new focus as argument */
    ///         }
    ///     )
    ///     .expect("Failed to initialize the keyboard");
    /// ```
    pub fn add_keyboard<F>(
        &mut self,
        xkb_config: keyboard::XkbConfig<'_>,
        repeat_delay: i32,
        repeat_rate: i32,
        mut focus_hook: F,
    ) -> Result<KeyboardHandle, KeyboardError>
    where
        F: FnMut(&Seat, Option<&wl_surface::WlSurface>) + 'static,
    {
        let me = self.clone();
        let mut inner = self.arc.inner.borrow_mut();
        let keyboard = self::keyboard::create_keyboard_handler(
            xkb_config,
            repeat_delay,
            repeat_rate,
            &self.arc.log,
            move |focus| focus_hook(&me, focus),
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

    /// Access the keyboard of this seat if any
    pub fn get_keyboard(&self) -> Option<KeyboardHandle> {
        self.arc.inner.borrow_mut().keyboard.clone()
    }

    /// Remove the keyboard capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_keyboard(&mut self) {
        let mut inner = self.arc.inner.borrow_mut();
        if inner.keyboard.is_some() {
            inner.keyboard = None;
            inner.send_all_caps();
        }
    }

    /// Checks whether a given [`WlSeat`](wl_seat::WlSeat) is associated with this [`Seat`]
    pub fn owns(&self, seat: &wl_seat::WlSeat) -> bool {
        let inner = self.arc.inner.borrow_mut();
        inner.known_seats.iter().any(|s| s.as_ref().equals(seat.as_ref()))
    }
}

impl ::std::cmp::PartialEq for Seat {
    fn eq(&self, other: &Seat) -> bool {
        Rc::ptr_eq(&self.arc, &other.arc)
    }
}

fn implement_seat(seat: Main<wl_seat::WlSeat>, arc: Rc<SeatRc>) -> wl_seat::WlSeat {
    let dest_arc = arc.clone();
    seat.quick_assign(move |seat, request, _| {
        let arc = seat.as_ref().user_data().get::<Rc<SeatRc>>().unwrap();
        let inner = arc.inner.borrow_mut();
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
            wl_seat::Request::GetTouch { .. } => {
                // TODO
            }
            wl_seat::Request::Release => {
                // Our destructors already handle it
            }
            _ => unreachable!(),
        }
    });
    seat.assign_destructor(Filter::new(move |seat: wl_seat::WlSeat, _, _| {
        dest_arc
            .inner
            .borrow_mut()
            .known_seats
            .retain(|s| !s.as_ref().equals(seat.as_ref()));
    }));
    seat.as_ref().user_data().set(move || arc);

    seat.deref().clone()
}
