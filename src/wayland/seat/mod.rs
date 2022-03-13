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
        PointerUserData,
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
    DataInit, DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase,
    Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
};

#[derive(Debug)]
struct Inner<T> {
    pointer: Option<PointerHandle<T>>,
    keyboard: Option<KeyboardHandle>,
    touch: Option<TouchHandle>,
    known_seats: Vec<wl_seat::WlSeat>,

    global_id: Option<GlobalId>,
}

#[derive(Debug)]
struct SeatRc<T> {
    name: String,
    inner: Mutex<Inner<T>>,
    user_data_map: UserDataMap,

    log: ::slog::Logger,
}

impl<T> Inner<T> {
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

    fn send_all_caps(&self, dh: &mut DisplayHandle<'_>) {
        let capabilities = self.compute_caps();
        for seat in &self.known_seats {
            seat.capabilities(dh, capabilities);
        }
    }
}

/// Handler trait for WlSeat
pub trait SeatHandler<T> {
    /// [SeatState] getter
    fn seat_state(&mut self) -> &mut SeatState<T>;
}

/// Global data of WlSeat
#[derive(Debug)]
pub struct SeatGlobalData<T> {
    arc: Arc<SeatRc<T>>,
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
pub struct Seat<T> {
    arc: Arc<SeatRc<T>>,
}

impl<T> Clone for Seat<T> {
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl<T: 'static> Seat<T> {
    /// Create a new seat global
    ///
    /// A new seat global is created with given name and inserted
    /// into this wayland display.
    ///
    /// You are provided with the state token to retrieve it (allowing
    /// you to add or remove capabilities from it), and the global handle,
    /// in case you want to remove it.
    pub fn new<D, N, L>(display: &mut Display<D>, name: N, logger: L) -> Self
    where
        D: GlobalDispatch<WlSeat, GlobalData = SeatGlobalData<T>> + 'static,
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

        let global_id = display.create_global(5, SeatGlobalData { arc: arc.clone() });
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
        seat.data::<SeatUserData<T>>()
            .map(|d| d.arc.clone())
            .map(|arc| Self { arc })
    }

    /// Access the `UserDataMap` associated with this `Seat`
    pub fn user_data(&self) -> &UserDataMap {
        &self.arc.user_data_map
    }

    /// Get the id of WlSeta global
    pub fn global_id(&self) -> GlobalId {
        self.arc.inner.lock().unwrap().global_id.as_ref().unwrap().clone()
    }
}

// Pointer
impl<T> Seat<T> {
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
    pub fn add_pointer<F>(&mut self, dh: &mut DisplayHandle<'_>, cb: F) -> PointerHandle<T>
    where
        F: FnMut(CursorImageStatus) + Send + Sync + 'static,
    {
        let mut inner = self.arc.inner.lock().unwrap();
        let pointer = self::pointer::PointerHandle::new(cb);
        if inner.pointer.is_some() {
            // there is already a pointer, remove it and notify the clients
            // of the change
            inner.pointer = None;
            inner.send_all_caps(dh);
        }
        inner.pointer = Some(pointer.clone());
        inner.send_all_caps(dh);
        pointer
    }

    /// Access the pointer of this seat if any
    pub fn get_pointer(&self) -> Option<PointerHandle<T>> {
        self.arc.inner.lock().unwrap().pointer.clone()
    }

    /// Remove the pointer capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_pointer(&mut self, dh: &mut DisplayHandle<'_>) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.pointer.is_some() {
            inner.pointer = None;
            inner.send_all_caps(dh);
        }
    }
}

// Keyboard
impl<T: 'static> Seat<T> {
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
        dh: &mut DisplayHandle<'_>,
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
            inner.send_all_caps(dh);
        }
        inner.keyboard = Some(keyboard.clone());
        inner.send_all_caps(dh);
        Ok(keyboard)
    }

    /// Access the keyboard of this seat if any
    pub fn get_keyboard(&self) -> Option<KeyboardHandle> {
        self.arc.inner.lock().unwrap().keyboard.clone()
    }

    /// Remove the keyboard capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_keyboard(&mut self, dh: &mut DisplayHandle<'_>) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.keyboard.is_some() {
            inner.keyboard = None;
            inner.send_all_caps(dh);
        }
    }
}

// Touch
impl<T> Seat<T> {
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
    pub fn add_touch(&mut self, dh: &mut DisplayHandle<'_>) -> TouchHandle {
        let mut inner = self.arc.inner.lock().unwrap();
        let touch = TouchHandle::new();
        if inner.touch.is_some() {
            // If there's already a tocuh device, remove it notify the clients about the change.
            inner.touch = None;
            inner.send_all_caps(dh);
        }
        inner.touch = Some(touch.clone());
        inner.send_all_caps(dh);
        touch
    }

    /// Access the touch device of this seat, if any.
    pub fn get_touch(&self) -> Option<TouchHandle> {
        self.arc.inner.lock().unwrap().touch.clone()
    }

    /// Remove the touch capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_touch(&mut self, dh: &mut DisplayHandle<'_>) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.touch.is_some() {
            inner.touch = None;
            inner.send_all_caps(dh);
        }
    }
}

impl<T> ::std::cmp::PartialEq for Seat<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
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
pub struct SeatState<T> {
    pd: PhantomData<T>,
}

impl<T> Default for SeatState<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for SeatState<T> {
    fn clone(&self) -> Self {
        Self { pd: self.pd }
    }
}

impl<T> SeatState<T> {
    /// Create a new seat global
    ///
    /// A new seat global is created with given name and inserted
    /// into this wayland display.
    ///
    /// You are provided with the state token to retrieve it (allowing
    /// you to add or remove capabilities from it), and the global handle,
    /// in case you want to remove it.
    pub fn new() -> Self {
        Self { pd: PhantomData }
    }
}

/// User data for seat
#[derive(Debug)]
pub struct SeatUserData<T> {
    arc: Arc<SeatRc<T>>,
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_seat {
    ($ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($ty: [
            $crate::reexports::wayland_server::protocol::wl_seat::WlSeat
        ] => $crate::wayland::seat::SeatState<$ty>);

        $crate::reexports::wayland_server::delegate_dispatch!($ty: [
            $crate::reexports::wayland_server::protocol::wl_seat::WlSeat,
            $crate::reexports::wayland_server::protocol::wl_pointer::WlPointer,
            $crate::reexports::wayland_server::protocol::wl_keyboard::WlKeyboard,
            $crate::reexports::wayland_server::protocol::wl_touch::WlTouch
        ] => $crate::wayland::seat::SeatState<$ty>);
    };
}

impl<T: 'static> DelegateDispatchBase<WlSeat> for SeatState<T> {
    type UserData = SeatUserData<T>;
}

impl<T, D> DelegateDispatch<WlSeat, D> for SeatState<T>
where
    D: Dispatch<WlSeat, UserData = SeatUserData<T>>,
    D: Dispatch<WlKeyboard, UserData = KeyboardUserData>,
    D: Dispatch<WlPointer, UserData = PointerUserData<T>>,
    D: Dispatch<WlTouch, UserData = TouchUserData>,
    D: SeatHandler<T>,
    D: 'static,
    T: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlSeat,
        request: wl_seat::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
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
                    h.new_kbd(dh, keyboard);
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

    fn destroyed(_state: &mut D, _: ClientId, object_id: ObjectId, data: &Self::UserData) {
        data.arc
            .inner
            .lock()
            .unwrap()
            .known_seats
            .retain(|s| s.id() != object_id);
    }
}

impl<T: 'static> DelegateGlobalDispatchBase<WlSeat> for SeatState<T> {
    type GlobalData = SeatGlobalData<T>;
}

impl<T, D> DelegateGlobalDispatch<WlSeat, D> for SeatState<T>
where
    D: GlobalDispatch<WlSeat, GlobalData = <Self as DelegateGlobalDispatchBase<WlSeat>>::GlobalData>,
    D: Dispatch<WlSeat, UserData = SeatUserData<T>>,
    D: Dispatch<WlKeyboard, UserData = KeyboardUserData>,
    D: Dispatch<WlPointer, UserData = PointerUserData<T>>,
    D: Dispatch<WlTouch, UserData = TouchUserData>,
    D: SeatHandler<T>,
    D: 'static,
    T: 'static,
{
    fn bind(
        _state: &mut D,
        handle: &mut wayland_server::DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: New<WlSeat>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let data = SeatUserData {
            arc: global_data.arc.clone(),
        };

        let resource = data_init.init(resource, data);

        if resource.version() >= 2 {
            resource.name(handle, global_data.arc.name.clone());
        }

        let mut inner = global_data.arc.inner.lock().unwrap();
        resource.capabilities(handle, inner.compute_caps());

        inner.known_seats.push(resource.clone());
    }
}
