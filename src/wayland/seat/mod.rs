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
//! use smithay::delegate_seat;
//! use smithay::wayland::seat::{Seat, SeatState, SeatHandler};
//!
//! # struct State { seat_state: SeatState<Self> };
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! // create the seat
//! let seat = Seat::<State>::new(
//!     &display_handle,          // the display
//!     "seat-0",          // the name of the seat, will be advertized to clients
//!     None                      // insert a logger here
//! );
//!
//! let seat_state = SeatState::<State>::new();
//! // add the seat state to your state
//! // ...
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//! }
//! delegate_seat!(State);
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

mod keyboard;
mod pointer;
mod touch;

use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
};

use crate::utils::user_data::UserDataMap;

// TODO: Just make the keyboard, pointer and touch modules public.
pub use self::{
    keyboard::{
        keysyms, Error as KeyboardError, FilterResult, GrabStartData as KeyboardGrabStartData, KeyboardGrab,
        KeyboardHandle, KeyboardInnerHandle, KeyboardUserData, Keysym, KeysymHandle, ModifiersState,
        XkbConfig,
    },
    pointer::{
        AxisFrame, ButtonEvent, CursorImageAttributes, CursorImageStatus,
        GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerHandle, PointerInnerHandle,
        PointerUserData, CURSOR_IMAGE_ROLE,
    },
    touch::{TouchHandle, TouchUserData},
};

use wayland_server::{
    backend::{ClientId, GlobalId, ObjectId},
    protocol::{
        wl_keyboard::WlKeyboard,
        wl_pointer::WlPointer,
        wl_seat::{self, WlSeat},
        wl_surface,
        wl_touch::WlTouch,
    },
    DataInit, DelegateDispatch, DelegateGlobalDispatch, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource,
};

#[derive(Debug)]
struct Inner<D> {
    pointer: Option<PointerHandle<D>>,
    keyboard: Option<KeyboardHandle>,
    touch: Option<TouchHandle>,
    known_seats: Vec<wl_seat::WlSeat>,
    global_id: Option<GlobalId>,
}

#[derive(Debug)]
struct SeatRc<D> {
    name: String,
    inner: Mutex<Inner<D>>,
    user_data_map: UserDataMap,
    log: ::slog::Logger,
}

impl<D> Inner<D> {
    fn compute_caps(&self) -> wl_seat::Capability {
        let mut caps = wl_seat::Capability::empty();
        if self.pointer.is_some() {
            caps |= wl_seat::Capability::Pointer;
        }
        if self.keyboard.is_some() {
            caps |= wl_seat::Capability::Keyboard;
        }
        if self.touch.is_some() {
            caps |= wl_seat::Capability::Touch;
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

/// Handler trait for WlSeat
pub trait SeatHandler: Sized {
    /// [SeatState] getter
    fn seat_state(&mut self) -> &mut SeatState<Self>;
}

/// Global data of WlSeat
#[derive(Debug)]
pub struct SeatGlobalData<D> {
    arc: Arc<SeatRc<D>>,
}

/// A Seat handle
/// This struct gives you access to the control of the
/// capabilities of the associated seat.
///
/// It is directly inserted in the wayland display by its [`new`](Seat::new) method.
///
/// This is an handle to the inner logic, it can be cloned.
///
/// See module-level documentation for details of use.
#[derive(Debug)]
pub struct Seat<D> {
    arc: Arc<SeatRc<D>>,
}

impl<D> Clone for Seat<D> {
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl<D: 'static> Seat<D> {
    /// Create a new seat global
    ///
    /// A new seat global is created with given name and inserted
    /// into this wayland display.
    ///
    /// You are provided with the state token to retrieve it (allowing
    /// you to add or remove capabilities from it), and the global handle,
    /// in case you want to remove it.
    pub fn new<N, L>(display: &DisplayHandle, name: N, logger: L) -> Self
    where
        D: GlobalDispatch<WlSeat, SeatGlobalData<D>> + 'static,
        N: Into<String>,
        L: Into<Option<::slog::Logger>>,
    {
        let name = name.into();

        let log = crate::slog_or_fallback(logger);
        let log = log.new(slog::o!("smithay_module" => "seat_handler", "seat_name" => name.clone()));

        let arc = Arc::new(SeatRc {
            name,
            inner: Mutex::new(Inner {
                pointer: None,
                keyboard: None,
                touch: None,
                known_seats: Default::default(),
                global_id: None,
            }),
            user_data_map: UserDataMap::new(),
            log,
        });

        let global_id = display.create_global::<D, _, _>(7, SeatGlobalData { arc: arc.clone() });
        arc.inner.lock().unwrap().global_id = Some(global_id);

        Self { arc }
    }

    /// Checks whether a given [`WlSeat`](wl_seat::WlSeat) is associated with this [`Seat`]
    pub fn owns(&self, seat: &wl_seat::WlSeat) -> bool {
        let inner = self.arc.inner.lock().unwrap();
        inner.known_seats.iter().any(|s| s == seat)
    }

    /// Attempt to retrieve a [`Seat`] from an existing resource
    pub fn from_resource(seat: &WlSeat) -> Option<Self> {
        seat.data::<SeatUserData<D>>()
            .map(|d| d.arc.clone())
            .map(|arc| Self { arc })
    }

    /// Access the `UserDataMap` associated with this `Seat`
    pub fn user_data(&self) -> &UserDataMap {
        &self.arc.user_data_map
    }

    /// Get the id of WlSeat global
    pub fn global(&self) -> GlobalId {
        self.arc.inner.lock().unwrap().global_id.as_ref().unwrap().clone()
    }
}

// Pointer
impl<D> Seat<D> {
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
    /// ```no_run
    /// # extern crate wayland_server;
    /// #
    /// # use smithay::wayland::seat::Seat;
    /// #
    /// # let mut seat: Seat<()> = unimplemented!();
    /// let pointer_handle = seat.add_pointer(
    ///     |new_status| { /* a closure handling requests from clients to change the cursor icon */ }
    /// );
    /// ```
    pub fn add_pointer<F>(&mut self, cb: F) -> PointerHandle<D>
    where
        F: FnMut(CursorImageStatus) + Send + Sync + 'static,
    {
        let mut inner = self.arc.inner.lock().unwrap();
        let pointer = self::pointer::PointerHandle::new(cb);
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
    pub fn get_pointer(&self) -> Option<PointerHandle<D>> {
        self.arc.inner.lock().unwrap().pointer.clone()
    }

    /// Remove the pointer capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_pointer(&mut self) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.pointer.is_some() {
            inner.pointer = None;
            inner.send_all_caps();
        }
    }
}

// Keyboard
impl<D: 'static> Seat<D> {
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
    /// # let mut seat: Seat<()> = unimplemented!();
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
        F: FnMut(&Self, Option<&wl_surface::WlSurface>) + 'static,
    {
        let me = self.clone();
        let mut inner = self.arc.inner.lock().unwrap();
        let keyboard = self::keyboard::KeyboardHandle::new(
            xkb_config,
            repeat_delay,
            repeat_rate,
            move |focus| focus_hook(&me, focus),
            &self.arc.log,
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
        self.arc.inner.lock().unwrap().keyboard.clone()
    }

    /// Remove the keyboard capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_keyboard(&mut self) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.keyboard.is_some() {
            inner.keyboard = None;
            inner.send_all_caps();
        }
    }
}

// Touch
impl<D> Seat<D> {
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
    /// ```no_run
    /// # extern crate wayland_server;
    /// #
    /// # use smithay::wayland::seat::Seat;
    /// # let mut seat: Seat<()> = unimplemented!();
    /// let touch_handle = seat.add_touch();
    /// ```
    pub fn add_touch(&mut self) -> TouchHandle {
        let mut inner = self.arc.inner.lock().unwrap();
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
        self.arc.inner.lock().unwrap().touch.clone()
    }

    /// Remove the touch capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_touch(&mut self) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.touch.is_some() {
            inner.touch = None;
            inner.send_all_caps();
        }
    }
}

impl<D> ::std::cmp::PartialEq for Seat<D> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}

/// Delegate type for all [Seat] globals.
///
/// Events will be forwarded to an instance of the Seat global.
#[derive(Debug)]
pub struct SeatState<D> {
    pd: PhantomData<D>,
}

impl<D> SeatState<D> {
    /// Create new delegate SeatState
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self { pd: PhantomData }
    }
}

/// User data for seat
#[derive(Debug)]
pub struct SeatUserData<D> {
    arc: Arc<SeatRc<D>>,
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_seat {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_seat::WlSeat: $crate::wayland::seat::SeatGlobalData<$ty>
        ] => $crate::wayland::seat::SeatState<$ty>);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_seat::WlSeat: $crate::wayland::seat::SeatUserData<$ty>
        ] => $crate::wayland::seat::SeatState<$ty>);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_pointer::WlPointer: $crate::wayland::seat::PointerUserData<$ty>
        ] => $crate::wayland::seat::SeatState<$ty>);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_keyboard::WlKeyboard: $crate::wayland::seat::KeyboardUserData
        ] => $crate::wayland::seat::SeatState<$ty>);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?$ty: [
            $crate::reexports::wayland_server::protocol::wl_touch::WlTouch: $crate::wayland::seat::TouchUserData
        ] => $crate::wayland::seat::SeatState<$ty>);
    };
}

impl<D> DelegateDispatch<WlSeat, SeatUserData<D>, D> for SeatState<D>
where
    D: Dispatch<WlSeat, SeatUserData<D>>,
    D: Dispatch<WlKeyboard, KeyboardUserData>,
    D: Dispatch<WlPointer, PointerUserData<D>>,
    D: Dispatch<WlTouch, TouchUserData>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlSeat,
        request: wl_seat::Request,
        data: &SeatUserData<D>,
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wl_seat::Request::GetPointer { id } => {
                let inner = data.arc.inner.lock().unwrap();

                let pointer = data_init.init(
                    id,
                    PointerUserData {
                        handle: inner.pointer.clone(),
                    },
                );

                if let Some(ref ptr_handle) = inner.pointer {
                    ptr_handle.new_pointer(pointer);
                } else {
                    // we should send a protocol error... but the protocol does not allow
                    // us, so this pointer will just remain inactive ¯\_(ツ)_/¯
                }
            }
            wl_seat::Request::GetKeyboard { id } => {
                let inner = data.arc.inner.lock().unwrap();

                let keyboard = data_init.init(
                    id,
                    KeyboardUserData {
                        handle: inner.keyboard.clone(),
                    },
                );

                if let Some(ref h) = inner.keyboard {
                    h.new_kbd(keyboard);
                } else {
                    // same as pointer, should error but cannot
                }
            }
            wl_seat::Request::GetTouch { id } => {
                let inner = data.arc.inner.lock().unwrap();

                let touch = data_init.init(
                    id,
                    TouchUserData {
                        handle: inner.touch.clone(),
                    },
                );

                if let Some(ref h) = inner.touch {
                    h.new_touch(touch);
                } else {
                    // same as pointer, should error but cannot
                }
            }
            wl_seat::Request::Release => {
                // Our destructors already handle it
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _: ClientId, object_id: ObjectId, data: &SeatUserData<D>) {
        data.arc
            .inner
            .lock()
            .unwrap()
            .known_seats
            .retain(|s| s.id() != object_id);
    }
}

impl<D> DelegateGlobalDispatch<WlSeat, SeatGlobalData<D>, D> for SeatState<D>
where
    D: GlobalDispatch<WlSeat, SeatGlobalData<D>>,
    D: Dispatch<WlSeat, SeatUserData<D>>,
    D: Dispatch<WlKeyboard, KeyboardUserData>,
    D: Dispatch<WlPointer, PointerUserData<D>>,
    D: Dispatch<WlTouch, TouchUserData>,
    D: SeatHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WlSeat>,
        global_data: &SeatGlobalData<D>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let data = SeatUserData {
            arc: global_data.arc.clone(),
        };

        let resource = data_init.init(resource, data);

        if resource.version() >= 2 {
            resource.name(global_data.arc.name.clone());
        }

        let mut inner = global_data.arc.inner.lock().unwrap();
        resource.capabilities(inner.compute_caps());

        inner.known_seats.push(resource);
    }
}
