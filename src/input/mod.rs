//!
//! Input abstractions
//!
//! This module provides some types loosely resembling instances of wayland seats, pointers and keyboards.
//! It is however not directly tied to wayland and can be used to multiplex various input operations
//! between different handlers.
//!
//! If the [`wayland_frontend`]-feature is enabled the `smithay::wayland::seat`-module provides additional
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
//! #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent},
//! #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
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
//!     None       // insert a logger here
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
//! #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
//! #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
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
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = Target;
//!     type PointerFocus = Target;
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
//! Currently, pointer and keyboard capabilities are supported by this module.
//! [`smithay::wayland::seat`] also provides an abstraction to send touch-events to client,
//! further helpers are not provided at this point.
//! [`smithay::wayland::tablet_manager`] also provides client interaction for drawing tablets.
//!
//! You can add these capabilities via methods of the [`Seat`] struct:
//! [`Seat::add_keyboard`] and [`Seat::add_pointer`].
//! These methods return handles that can be cloned and sent across thread, so you can keep one around
//! in your event-handling code to forward inputs to your clients.
//!

use std::sync::{Arc, Mutex};

use self::keyboard::{Error as KeyboardError, KeyboardHandle, KeyboardTarget};
use self::pointer::{CursorImageStatus, PointerHandle, PointerTarget};
use crate::utils::user_data::UserDataMap;

pub mod keyboard;
pub mod pointer;

/// Handler trait for Seats
pub trait SeatHandler: Sized {
    /// Type used to represent the target currently holding the keyboard focus
    type KeyboardFocus: KeyboardTarget<Self> + 'static;
    /// Type used to represent the target currently holding the pointer focus
    type PointerFocus: PointerTarget<Self> + 'static;

    /// [SeatState] getter
    fn seat_state(&mut self) -> &mut SeatState<Self>;

    /// Callback that will be notified whenever the focus of the seat changes.
    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&Self::KeyboardFocus>) {}

    /// Callback that will be notified whenever a client requests to set a custom cursor image.
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}
/// Delegate type for all [Seat] globals.
///
/// Events will be forwarded to an instance of the Seat global.
#[derive(Debug)]
pub struct SeatState<D: SeatHandler> {
    pub(crate) seats: Vec<Seat<D>>,
}

/// A Seat handle
///
/// This struct gives you access to the control of the
/// capabilities of the associated seat.
///
/// This is an handle to the inner logic, it can be cloned.
///
/// See module-level documentation for details of use.
#[derive(Debug)]
pub struct Seat<D: SeatHandler> {
    pub(crate) arc: Arc<SeatRc<D>>,
}

#[derive(Debug)]
pub(crate) struct Inner<D: SeatHandler> {
    pub(crate) pointer: Option<PointerHandle<D>>,
    pub(crate) keyboard: Option<KeyboardHandle<D>>,

    #[cfg(feature = "wayland_frontend")]
    pub(crate) touch: Option<crate::wayland::seat::TouchHandle>,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) global: Option<wayland_server::backend::GlobalId>,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) known_seats: Vec<wayland_server::protocol::wl_seat::WlSeat>,
}

#[derive(Debug)]
pub(crate) struct SeatRc<D: SeatHandler> {
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) inner: Mutex<Inner<D>>,
    user_data_map: UserDataMap,
    log: ::slog::Logger,
}

impl<D: SeatHandler> Clone for Seat<D> {
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl<D: SeatHandler> Default for SeatState<D> {
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
    pub fn new_seat<N, L>(&mut self, name: N, logger: L) -> Seat<D>
    where
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

                #[cfg(feature = "wayland_frontend")]
                touch: None,
                #[cfg(feature = "wayland_frontend")]
                global: None,
                #[cfg(feature = "wayland_frontend")]
                known_seats: Vec::new(),
            }),
            user_data_map: UserDataMap::new(),
            log,
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
    /// #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent},
    /// #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
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
    /// #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
    /// #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
    /// #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
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
    /// # struct State;
    /// # impl SeatHandler for State {
    /// #     type KeyboardFocus = Target;
    /// #     type PointerFocus = Target;
    /// #
    /// #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
    /// #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Target>) { unimplemented!() }
    /// #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
    /// # }
    /// # let mut seat: Seat<State> = unimplemented!();
    /// let pointer_handle = seat.add_pointer();
    /// ```
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
    /// #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent},
    /// #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
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
    /// #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
    /// #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
    /// #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
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
    /// #
    /// # struct State;
    /// # impl SeatHandler for State {
    /// #     type KeyboardFocus = Target;
    /// #     type PointerFocus = Target;
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
    pub fn add_keyboard(
        &mut self,
        xkb_config: keyboard::XkbConfig<'_>,
        repeat_delay: i32,
        repeat_rate: i32,
    ) -> Result<KeyboardHandle<D>, KeyboardError> {
        let mut inner = self.arc.inner.lock().unwrap();
        let keyboard =
            self::keyboard::KeyboardHandle::new(xkb_config, repeat_delay, repeat_rate, &self.arc.log)?;
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
    pub fn remove_keyboard(&mut self) {
        let mut inner = self.arc.inner.lock().unwrap();
        if inner.keyboard.is_some() {
            inner.keyboard = None;
            #[cfg(feature = "wayland_frontend")]
            inner.send_all_caps();
        }
    }
}

impl<D: SeatHandler> ::std::cmp::PartialEq for Seat<D> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}
