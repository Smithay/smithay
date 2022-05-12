//! Seat utilities
//!
//! This module provides you with utilities for handling seats.
use crate::utils::user_data::UserDataMap;
use std::{cell::RefCell, fmt, rc::Rc};
#[cfg(feature = "wayland_frontend")]
use wayland_server::{protocol::wl_seat, Display, Filter, Global, Main};

mod keyboard;
mod pointer;
/*
mod touch;
*/
pub use self::{
    keyboard::{
        keysyms, Error as KeyboardError, FilterResult, GrabStartData as KeyboardGrabStartData, KeyboardGrab,
        KeyboardHandle, KeyboardHandler, KeyboardInnerHandle, Keysym, KeysymHandle, ModifiersState,
        XkbConfig,
    },
    pointer::{
        Axis, AxisFrame, AxisSource, ButtonState, CursorImageAttributes, CursorImageStatus,
        GrabStartData as PointerGrabStartData, PointerGrab, PointerHandle, PointerHandler,
        PointerInnerHandle, UnknownButtonState,
    },
    //touch::TouchHandle,
};

#[derive(Debug)]
struct Inner {
    pointer: Option<PointerHandle>,
    keyboard: Option<KeyboardHandle>,
    //touch: Option<TouchHandle>,
    #[cfg(feature = "wayland_frontend")]
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

#[cfg(feature = "wayland_frontend")]
impl Inner {
    fn compute_caps(&self) -> wl_seat::Capability {
        let mut caps = wl_seat::Capability::empty();
        if self.pointer.is_some() {
            caps |= wl_seat::Capability::Pointer;
        }
        if self.keyboard.is_some() {
            caps |= wl_seat::Capability::Keyboard;
        }
        /*
        if self.touch.is_some() {
            caps |= wl_seat::Capability::Touch;
        }
        */
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
/// This is an handle to the inner logic, it can be cloned.
///
/// See module-level documentation for details of use.
#[derive(Debug, Clone)]
pub struct Seat {
    pub(crate) rc: Rc<SeatRc>,
}

impl Seat {
    /// Create a new seat
    ///
    /// A new seat is created with given name.
    ///
    /// You are provided with the state token to retrieve it (allowing
    /// you to add or remove capabilities from it)
    pub fn new<L>(name: String, logger: L) -> Seat
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger);
        let rc = Rc::new(SeatRc {
            inner: RefCell::new(Inner {
                pointer: None,
                keyboard: None,
                //touch: None,
                known_seats: Vec::new(),
            }),
            log: log.new(slog::o!("smithay_module" => "seat_handler", "seat_name" => name.clone())),
            name,
            user_data: UserDataMap::new(),
        });
        Seat { rc: rc.clone() }
    }

    #[cfg(feature = "wayland_frontend")]
    pub fn create_global(&mut self, display: &mut Display) -> Global<wl_seat::WlSeat> {
        let rc = self.rc.clone();
        display.create_global(
            5,
            Filter::new(move |(new_seat, _version), _, _| {
                let seat = implement_seat(new_seat, rc.clone());
                let mut inner = rc.inner.borrow_mut();
                if seat.as_ref().version() >= 2 {
                    seat.name(rc.name.clone());
                }
                seat.capabilities(inner.compute_caps());
                inner.known_seats.push(seat);
            }),
        )
    }

    #[cfg(feature = "wayland_frontend")]
    /// Attempt to retrieve a [`Seat`] from an existing resource
    pub fn from_resource(seat: &wl_seat::WlSeat) -> Option<Seat> {
        seat.as_ref()
            .user_data()
            .get::<Rc<SeatRc>>()
            .cloned()
            .map(|rc| Seat { rc })
    }

    /// Access the `UserDataMap` associated with this `Seat`
    pub fn user_data(&self) -> &UserDataMap {
        &self.rc.user_data
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
        let mut inner = self.rc.inner.borrow_mut();
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
        self.rc.inner.borrow_mut().pointer.clone()
    }

    /// Remove the pointer capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_pointer(&mut self) {
        let mut inner = self.rc.inner.borrow_mut();
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
        F: FnMut(&Seat, Option<&dyn KeyboardHandler>) + 'static,
    {
        let me = self.clone();
        let mut inner = self.rc.inner.borrow_mut();
        let keyboard = self::keyboard::create_keyboard_handler(
            xkb_config,
            repeat_delay,
            repeat_rate,
            &self.rc.log,
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
        self.rc.inner.borrow_mut().keyboard.clone()
    }

    /// Remove the keyboard capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_keyboard(&mut self) {
        let mut inner = self.rc.inner.borrow_mut();
        if inner.keyboard.is_some() {
            inner.keyboard = None;
            inner.send_all_caps();
        }
    }

    /*
    /// Adds the touch capability to this seat
    ///
    /// You are provided a [`TouchHandle`], which allows you to send input events
    /// to this pointer. This handle can be cloned.
    ///
    /// Calling this method on a seat that already has a touch capability
    /// will overwrite it, and will be seen by the clients as if the
    /// touchscreen was unplugged and a new one was plugged in.
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
    /// let touch_handle = seat.add_touch();
    /// ```
    pub fn add_touch(&mut self) -> TouchHandle {
        let mut inner = self.arc.inner.borrow_mut();
        let touch = TouchHandle::new();
        if inner.touch.is_some() {
            // If there's already a tocuh device, remove it notify the clients about the change.
            inner.touch = None;
            inner.send_all_caps();
        }
        inner.touch = Some(touch.clone());
        inner.send_all_caps();
        touch
    }

    /// Access the touch device of this seat, if any.
    pub fn get_touch(&self) -> Option<TouchHandle> {
        self.rc.inner.borrow_mut().touch.clone()
    }

    /// Remove the touch capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_touch(&mut self) {
        let mut inner = self.arc.inner.borrow_mut();
        if inner.touch.is_some() {
            inner.touch = None;
            inner.send_all_caps();
        }
    }
    */

    #[cfg(feature = "wayland_frontend")]
    /// Checks whether a given [`WlSeat`](wl_seat::WlSeat) is associated with this [`Seat`]
    pub fn owns(&self, seat: &wl_seat::WlSeat) -> bool {
        let inner = self.rc.inner.borrow_mut();
        inner.known_seats.iter().any(|s| s.as_ref().equals(seat.as_ref()))
    }
}

impl ::std::cmp::PartialEq for Seat {
    fn eq(&self, other: &Seat) -> bool {
        Rc::ptr_eq(&self.rc, &other.rc)
    }
}

#[cfg(feature = "wayland_frontend")]
fn implement_seat(seat: Main<wl_seat::WlSeat>, arc: Rc<SeatRc>) -> wl_seat::WlSeat {
    use std::ops::Deref;

    let dest_arc = arc.clone();
    seat.quick_assign(move |seat, request, _| {
        let arc = seat.as_ref().user_data().get::<Rc<SeatRc>>().unwrap();
        let inner = arc.inner.borrow_mut();
        match request {
            wl_seat::Request::GetPointer { id } => {
                self::pointer::implement_pointer(id, inner.pointer.as_ref())
            }
            wl_seat::Request::GetKeyboard { id } => {
                self::keyboard::implement_keyboard(id, inner.keyboard.as_ref())
            }
            /*
            wl_seat::Request::GetTouch { id } => {
                self::touch::implement_touch(id, inner.touch.as_ref())
            }
            */
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
