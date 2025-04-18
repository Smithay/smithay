//!
//! Input abstractions
//!
//! This module provides some types loosely resembling instances of wayland seats, pointers, touch and keyboards.
//! It is however not directly tied to wayland and can be used to multiplex various input operations
//! between different handlers.
//!
//! If the `wayland_frontend`-feature is enabled the `smithay::wayland::seat`-module provides additional
//! functionality for the provided types of this module to map them to advertised wayland globals and objects.
//!
//! ## How to use it
//!
//! To start using this module you need to create a [`SeatState`] and use that to create [`Seat`]s.
//! Additionally you need to implement the [`SeatHandler`] trait.
//!
//! ### Initialization
//!
//! ```
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! # use smithay::backend::input::KeyState;
//! # use smithay::input::{
//! #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent, RelativeMotionEvent,
//! #             GestureSwipeBeginEvent, GestureSwipeUpdateEvent, GestureSwipeEndEvent,
//! #             GesturePinchBeginEvent, GesturePinchUpdateEvent, GesturePinchEndEvent,
//! #             GestureHoldBeginEvent, GestureHoldEndEvent},
//! #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
//! #   touch::{DownEvent, UpEvent, MotionEvent as TouchMotionEvent, ShapeEvent, OrientationEvent, TouchTarget},
//! # };
//! # use smithay::utils::{IsAlive, Serial};
//!
//! struct State {
//!     seat_state: SeatState<Self>,
//!     // ...
//! };
//!
//! let mut seat_state = SeatState::<State>::new();
//!
//! // create the seat
//! let seat = seat_state.new_seat(
//!     "seat-0",  // the name of the seat, will be advertized to clients
//! );
//!
//! # #[derive(Debug, Clone, PartialEq)]
//! # struct Target;
//! # impl IsAlive for Target {
//! #   fn alive(&self) -> bool { true }
//! # }
//! # impl PointerTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {}
//! #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
//! #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
//! #   fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {}
//! #   fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {}
//! #   fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {}
//! #   fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {}
//! #   fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {}
//! #   fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {}
//! #   fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {}
//! #   fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {}
//! # }
//! # impl KeyboardTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {}
//! #   fn key(
//! #       &self,
//! #       seat: &Seat<State>,
//! #       data: &mut State,
//! #       key: KeysymHandle<'_>,
//! #       state: KeyState,
//! #       serial: Serial,
//! #       time: u32,
//! #   ) {}
//! #   fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {}
//! # }
//! # impl TouchTarget<State> for Target {
//! #   fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent, seq: Serial) {}
//! #   fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent, seq: Serial) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &TouchMotionEvent, seq: Serial) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
//! #   fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
//! #   fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent, seq: Serial) {}
//! #   fn orientation(&self, seat: &Seat<State>, data: &mut State, event: &OrientationEvent, seq: Serial) {}
//! # }
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = Target;
//!     type PointerFocus = Target;
//!     type TouchFocus = Target;
//!
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Target>) {
//!         // handle focus changes, if you need to ...
//!     }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
//!         // handle new images for the cursor ...
//!     }
//! }
//! ```
//!
//! ### Run usage
//!
//! Once the seat is initialized, you can add capabilities to it.
//!
//! Currently, pointer, touch and keyboard capabilities are supported by this module.
//! [`tablet_manager`](crate::wayland::tablet_manager) also provides client interaction for drawing tablets.
//!
//! You can add these capabilities via methods of the [`Seat`] struct:
//! [`Seat::add_keyboard`], [`Seat::add_pointer`] and [`Seat::add_touch`].
//! These methods return handles that can be cloned and sent across thread, so you can keep one around
//! in your event-handling code to forward inputs to your clients.
//!

use std::{
    fmt,
    hash::Hash,
    sync::{Arc, Mutex, Weak},
};

use tracing::{info_span, instrument};

use self::touch::TouchTarget;
use self::{
    keyboard::{Error as KeyboardError, KeyboardHandle, KeyboardTarget, LedState},
    touch::TouchHandle,
};
use self::{
    pointer::{CursorImageStatus, PointerHandle, PointerTarget},
    touch::TouchGrab,
};
use crate::utils::{user_data::UserDataMap, Serial};

pub mod keyboard;
pub mod pointer;
pub mod touch;

/// Handler trait for Seats
pub trait SeatHandler: Sized {
    /// Type used to represent the target currently holding the keyboard focus
    type KeyboardFocus: KeyboardTarget<Self> + 'static;
    /// Type used to represent the target currently holding the pointer focus
    type PointerFocus: PointerTarget<Self> + 'static;
    /// Type used to represent the target currently holding the touch focus
    type TouchFocus: TouchTarget<Self> + 'static;

    /// [SeatState] getter
    fn seat_state(&mut self) -> &mut SeatState<Self>;

    /// Callback that will be notified whenever the focus of the seat changes.
    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&Self::KeyboardFocus>) {}

    /// Callback that will be notified whenever a client requests to set a custom cursor image.
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    /// Callback that will be notified whenever the keyboard led state changes.
    fn led_state_changed(&mut self, _seat: &Seat<Self>, _led_state: LedState) {}
}
/// Delegate type for all [Seat] globals.
///
/// Events will be forwarded to an instance of the Seat global.
pub struct SeatState<D: SeatHandler> {
    pub(crate) seats: Vec<Seat<D>>,
}

impl<D: SeatHandler> fmt::Debug for SeatState<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeatState").field("seats", &self.seats).finish()
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
pub struct Seat<D: SeatHandler> {
    pub(crate) arc: Arc<SeatRc<D>>,
}

/// Weak variant of an [`Seat`]
///
/// Does not keep associated user data alive,
/// and can be used to refer to a potentially already destroyed seat.
#[derive(Debug, Clone)]
pub struct WeakSeat<D: SeatHandler>(Weak<SeatRc<D>>);

impl<D: SeatHandler> WeakSeat<D> {
    /// Try to retrieve the original `Seat`, if it still exists
    pub fn upgrade(&self) -> Option<Seat<D>> {
        self.0.upgrade().map(|arc| Seat { arc })
    }

    /// Check if the seat is still alive
    pub fn is_alive(&self) -> bool {
        self.0.strong_count() != 0
    }
}

impl<D: SeatHandler> Seat<D> {
    /// Create a weak reference to this seat
    pub fn downgrade(&self) -> WeakSeat<D> {
        WeakSeat(Arc::downgrade(&self.arc))
    }
}

impl<D: SeatHandler> fmt::Debug for Seat<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Seat").field("arc", &self.arc).finish()
    }
}

impl<D: SeatHandler> PartialEq for Seat<D> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}
impl<D: SeatHandler> Eq for Seat<D> {}

impl<D: SeatHandler> Hash for Seat<D> {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.arc).hash(state)
    }
}

pub(crate) struct Inner<D: SeatHandler> {
    pub(crate) pointer: Option<PointerHandle<D>>,
    pub(crate) keyboard: Option<KeyboardHandle<D>>,
    pub(crate) touch: Option<TouchHandle<D>>,

    #[cfg(feature = "wayland_frontend")]
    pub(crate) global: Option<wayland_server::backend::GlobalId>,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) known_seats: Vec<wayland_server::Weak<wayland_server::protocol::wl_seat::WlSeat>>,
}

#[cfg(not(feature = "wayland_frontend"))]
impl<D: SeatHandler> fmt::Debug for Inner<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("pointer", &self.pointer)
            .field("keyboard", &self.keyboard)
            .finish()
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D: SeatHandler> fmt::Debug for Inner<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("pointer", &self.pointer)
            .field("keyboard", &self.keyboard)
            .field("touch", &self.touch)
            .field("global", &self.global)
            .field("known_seats", &self.known_seats)
            .finish()
    }
}

pub(crate) struct SeatRc<D: SeatHandler> {
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) inner: Mutex<Inner<D>>,
    span: tracing::Span,
    user_data_map: UserDataMap,
}

impl<D: SeatHandler> fmt::Debug for SeatRc<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeatRc")
            .field("name", &self.name)
            .field("inner", &self.inner)
            .field("user_data_map", &self.user_data_map)
            .finish()
    }
}

impl<D: SeatHandler> Clone for Seat<D> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl<D: SeatHandler> Default for SeatState<D> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<D: SeatHandler> SeatState<D> {
    /// Create new delegate SeatState
    pub fn new() -> Self {
        Self { seats: Vec::new() }
    }

    /// Create a new seat
    pub fn new_seat<N>(&mut self, name: N) -> Seat<D>
    where
        N: Into<String>,
    {
        let name = name.into();
        let span = info_span!("input_seat", name);

        let arc = Arc::new(SeatRc {
            name,
            inner: Mutex::new(Inner {
                pointer: None,
                keyboard: None,
                touch: None,

                #[cfg(feature = "wayland_frontend")]
                global: None,
                #[cfg(feature = "wayland_frontend")]
                known_seats: Vec::new(),
            }),
            span,
            user_data_map: UserDataMap::new(),
        });
        self.seats.push(Seat { arc: arc.clone() });

        Seat { arc }
    }
}

impl<D: SeatHandler + 'static> Seat<D> {
    /// Access the `UserDataMap` associated with this `Seat`
    pub fn user_data(&self) -> &UserDataMap {
        &self.arc.user_data_map
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
    /// # Examples
    ///
    /// ```no_run
    /// # use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
    /// # use smithay::backend::input::KeyState;
    /// # use smithay::input::{
    /// #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent, RelativeMotionEvent,
    /// #             GestureSwipeBeginEvent, GestureSwipeUpdateEvent, GestureSwipeEndEvent,
    /// #             GesturePinchBeginEvent, GesturePinchUpdateEvent, GesturePinchEndEvent,
    /// #             GestureHoldBeginEvent, GestureHoldEndEvent},
    /// #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    /// #   touch::{DownEvent, UpEvent, MotionEvent as TouchMotionEvent, ShapeEvent, OrientationEvent, TouchTarget},
    /// # };
    /// # use smithay::utils::{IsAlive, Serial};
    /// #
    /// # #[derive(Debug, Clone, PartialEq)]
    /// # struct Target;
    /// # impl IsAlive for Target {
    /// #   fn alive(&self) -> bool { true }
    /// # }
    /// # impl PointerTarget<State> for Target {
    /// #   fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
    /// #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
    /// #   fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {}
    /// #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
    /// #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
    /// #   fn frame(&self, seat: &Seat<State>, data: &mut State) {}
    /// #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
    /// #   fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {}
    /// #   fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {}
    /// #   fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {}
    /// #   fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {}
    /// #   fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {}
    /// #   fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {}
    /// #   fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {}
    /// #   fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {}
    /// # }
    /// # impl KeyboardTarget<State> for Target {
    /// #   fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {}
    /// #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {}
    /// #   fn key(
    /// #       &self,
    /// #       seat: &Seat<State>,
    /// #       data: &mut State,
    /// #       key: KeysymHandle<'_>,
    /// #       state: KeyState,
    /// #       serial: Serial,
    /// #       time: u32,
    /// #   ) {}
    /// #   fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {}
    /// # }
    /// # impl TouchTarget<State> for Target {
    /// #   fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent, seq: Serial) {}
    /// #   fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent, seq: Serial) {}
    /// #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &TouchMotionEvent, seq: Serial) {}
    /// #   fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
    /// #   fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
    /// #   fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent, seq: Serial) {}
    /// #   fn orientation(&self, seat: &Seat<State>, data: &mut State, event: &OrientationEvent, seq: Serial) {}
    /// # }
    /// # struct State;
    /// # impl SeatHandler for State {
    /// #     type KeyboardFocus = Target;
    /// #     type PointerFocus = Target;
    /// #     type TouchFocus = Target;
    /// #
    /// #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
    /// #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Target>) { unimplemented!() }
    /// #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
    /// # }
    /// # let mut seat: Seat<State> = unimplemented!();
    /// let pointer_handle = seat.add_pointer();
    /// ```
    #[instrument(parent = &self.arc.span, skip(self))]
    pub fn add_pointer(&mut self) -> PointerHandle<D> {
        let mut inner = self.arc.inner.lock().unwrap();
        let pointer = PointerHandle::new();
        if inner.pointer.is_some() {
            // there is already a pointer, remove it and notify the clients
            // of the change
            inner.pointer = None;
            #[cfg(feature = "wayland_frontend")]
            inner.send_all_caps();
        }
        inner.pointer = Some(pointer.clone());
        #[cfg(feature = "wayland_frontend")]
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
    #[instrument(parent = &self.arc.span, skip(self))]
    pub fn remove_pointer(&mut self) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.pointer.is_some() {
            inner.pointer = None;
            #[cfg(feature = "wayland_frontend")]
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
    /// # use smithay::input::{Seat, SeatState, SeatHandler, keyboard::XkbConfig, pointer::CursorImageStatus};
    /// # use smithay::backend::input::KeyState;
    /// # use smithay::input::{
    /// #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent, RelativeMotionEvent,
    /// #             GestureSwipeBeginEvent, GestureSwipeUpdateEvent, GestureSwipeEndEvent,
    /// #             GesturePinchBeginEvent, GesturePinchUpdateEvent, GesturePinchEndEvent,
    /// #             GestureHoldBeginEvent, GestureHoldEndEvent},
    /// #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    /// #   touch::{DownEvent, UpEvent, MotionEvent as TouchMotionEvent, ShapeEvent, OrientationEvent, TouchTarget},
    /// # };
    /// # use smithay::utils::{IsAlive, Serial};
    /// #
    /// # #[derive(Debug, Clone, PartialEq)]
    /// # struct Target;
    /// # impl IsAlive for Target {
    /// #   fn alive(&self) -> bool { true }
    /// # }
    /// # impl PointerTarget<State> for Target {
    /// #   fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
    /// #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
    /// #   fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {}
    /// #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
    /// #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
    /// #   fn frame(&self, seat: &Seat<State>, data: &mut State) {}
    /// #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
    /// #   fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {}
    /// #   fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {}
    /// #   fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {}
    /// #   fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {}
    /// #   fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {}
    /// #   fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {}
    /// #   fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {}
    /// #   fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {}
    /// # }
    /// # impl KeyboardTarget<State> for Target {
    /// #   fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {}
    /// #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {}
    /// #   fn key(
    /// #       &self,
    /// #       seat: &Seat<State>,
    /// #       data: &mut State,
    /// #       key: KeysymHandle<'_>,
    /// #       state: KeyState,
    /// #       serial: Serial,
    /// #       time: u32,
    /// #   ) {}
    /// #   fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {}
    /// # }
    /// # impl TouchTarget<State> for Target {
    /// #   fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent, seq: Serial) {}
    /// #   fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent, seq: Serial) {}
    /// #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &TouchMotionEvent, seq: Serial) {}
    /// #   fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
    /// #   fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
    /// #   fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent, seq: Serial) {}
    /// #   fn orientation(&self, seat: &Seat<State>, data: &mut State, event: &OrientationEvent, seq: Serial) {}
    /// # }
    /// #
    /// # struct State;
    /// # impl SeatHandler for State {
    /// #     type KeyboardFocus = Target;
    /// #     type PointerFocus = Target;
    /// #     type TouchFocus = Target;
    /// #
    /// #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
    /// #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Target>) { unimplemented!() }
    /// #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
    /// # }
    /// # let mut seat: Seat<State> = unimplemented!();
    /// let keyboard = seat
    ///     .add_keyboard(
    ///         XkbConfig {
    ///             layout: "de",
    ///             variant: "nodeadkeys",
    ///             ..XkbConfig::default()
    ///         },
    ///         200,
    ///         25,
    ///     )
    ///     .expect("Failed to initialize the keyboard");
    /// ```
    #[instrument(parent = &self.arc.span, skip(self))]
    pub fn add_keyboard(
        &mut self,
        xkb_config: keyboard::XkbConfig<'_>,
        repeat_delay: i32,
        repeat_rate: i32,
    ) -> Result<KeyboardHandle<D>, KeyboardError> {
        let mut inner = self.arc.inner.lock().unwrap();
        let keyboard = self::keyboard::KeyboardHandle::new(xkb_config, repeat_delay, repeat_rate)?;
        if inner.keyboard.is_some() {
            // there is already a keyboard, remove it and notify the clients
            // of the change
            inner.keyboard = None;
            #[cfg(feature = "wayland_frontend")]
            inner.send_all_caps();
        }
        inner.keyboard = Some(keyboard.clone());
        #[cfg(feature = "wayland_frontend")]
        inner.send_all_caps();
        Ok(keyboard)
    }

    /// Access the keyboard of this seat if any
    pub fn get_keyboard(&self) -> Option<KeyboardHandle<D>> {
        self.arc.inner.lock().unwrap().keyboard.clone()
    }

    /// Remove the keyboard capability from this seat
    ///
    /// Clients will be appropriately notified.
    #[instrument(parent = &self.arc.span, skip(self))]
    pub fn remove_keyboard(&mut self) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.keyboard.is_some() {
            inner.keyboard = None;
            #[cfg(feature = "wayland_frontend")]
            inner.send_all_caps();
        }
    }

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
    /// # use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
    /// # use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
    /// #
    /// # struct State;
    /// # impl SeatHandler for State {
    /// #     type KeyboardFocus = WlSurface;
    /// #     type PointerFocus = WlSurface;
    /// #     type TouchFocus = WlSurface;
    /// #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
    /// #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
    /// #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
    /// # }
    /// # let mut seat: Seat<State> = unimplemented!();
    /// let touch_handle = seat.add_touch();
    /// ```
    pub fn add_touch(&mut self) -> TouchHandle<D> {
        Self::add_touch_with_default_grab(self, || Box::new(touch::DefaultGrab))
    }

    /// Adds the touch capability to this seat and allows the use of a custom default [`TouchGrab`]
    ///
    /// The default grab is used in case no other grab is currently active. When using [`Seat::add_touch`]
    /// it will use [`touch::DefaultGrab`] which will install [`touch::TouchDownGrab`] on the first touch point.
    /// [`touch::TouchDownGrab`] makes sure all further touch points will use the same target until all touch
    /// points are released again.
    ///
    /// See [`Seat::add_touch`] for more information
    pub fn add_touch_with_default_grab<F>(&mut self, defaut_grab: F) -> TouchHandle<D>
    where
        F: Fn() -> Box<dyn TouchGrab<D>> + Send + 'static,
    {
        let mut inner = self.arc.inner.lock().unwrap();
        let touch = TouchHandle::new(defaut_grab);
        if inner.touch.is_some() {
            // If there's already a tocuh device, remove it notify the clients about the change.
            inner.touch = None;
            #[cfg(feature = "wayland_frontend")]
            inner.send_all_caps();
        }
        inner.touch = Some(touch.clone());
        #[cfg(feature = "wayland_frontend")]
        inner.send_all_caps();
        touch
    }

    /// Access the touch device of this seat, if any.
    pub fn get_touch(&self) -> Option<TouchHandle<D>> {
        self.arc.inner.lock().unwrap().touch.clone()
    }

    /// Remove the touch capability from this seat
    ///
    /// Clients will be appropriately notified.
    pub fn remove_touch(&mut self) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.touch.is_some() {
            inner.touch = None;
            #[cfg(feature = "wayland_frontend")]
            inner.send_all_caps();
        }
    }

    /// Gets this seat's name
    pub fn name(&self) -> &str {
        &self.arc.name
    }
}

pub(super) enum GrabStatus<G: ?Sized> {
    None,
    Active(Serial, Box<G>),
    Borrowed,
}

// `G` is not `Debug`, so we have to impl Debug manually
impl<G: ?Sized> fmt::Debug for GrabStatus<G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrabStatus::None => f.debug_tuple("GrabStatus::None").finish(),
            GrabStatus::Active(serial, _) => f.debug_tuple("GrabStatus::Active").field(&serial).finish(),
            GrabStatus::Borrowed => f.debug_tuple("GrabStatus::Borrowed").finish(),
        }
    }
}
