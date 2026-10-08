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
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! # use smithay::wayland::pointer_constraints::PointerConstraintsHandler;
//! # use smithay::reexports::wayland_server::Client;
//!
//! # struct State { seat_state: SeatState<Self> };
//! # let mut display = Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let mut seat_state = SeatState::<State>::new();
//! // add the seat state to your state
//! // ...
//!
//! // create the wl_seat
//! let seat = seat_state.new_wl_seat(
//!     &display_handle,          // the display
//!     "seat-0",                 // the name of the seat, will be advertized to clients
//! );
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = WlSurface;
//!     type PointerFocus = WlSurface;
//!     type TouchFocus = WlSurface;
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
//!         // ...
//!     }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
//!         // ...
//!     }
//! }
//! # impl PointerConstraintsHandler for State {}
//!
//! smithay::delegate_dispatch2!(State);
//!
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &WlSurface) {}
//! # }
//! ```
//!
//! ### Run usage
//!
//! Once the seat is initialized, you can add capabilities to it.
//!
//! You can add these capabilities via methods of the [`Seat`] struct:
//! [`Seat::add_keyboard`], [`Seat::add_pointer`] and [`Seat::add_touch`].
//! These methods return handles that can be cloned and sent across thread, so you can keep one around
//! in your event-handling code to forward inputs to your clients.
//!
//! This module further defines the `"cursor_image"` role, that is assigned to surfaces used by clients
//! to change the cursor icon.

pub(crate) mod keyboard;
pub(crate) mod pointer;
mod touch;

use std::{
    borrow::Cow,
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicU32, Ordering},
    },
};

use crate::input::{Inner, Seat, SeatHandler, SeatRc, SeatState};
use crate::wayland::{Dispatch2, GlobalDispatch2};

pub use self::{
    keyboard::KeyboardUserData,
    pointer::{CURSOR_IMAGE_ROLE, PointerUserData},
    touch::TouchUserData,
};

use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    backend::{ClientId, GlobalId, ObjectId},
    protocol::{
        wl_keyboard::WlKeyboard,
        wl_pointer::WlPointer,
        wl_seat::{self, WlSeat},
        wl_surface,
        wl_touch::WlTouch,
    },
};

use super::compositor::CompositorHandler;

/// Focused objects that *might* have an underlying wl_surface.
pub trait WaylandFocus {
    /// Returns the underlying wl_surface, if any.
    ///
    /// *Note*: This has to return `Some`, if `same_client_as` can return true
    /// for any provided `ObjectId`
    fn wl_surface(&self) -> Option<Cow<'_, wl_surface::WlSurface>>;
    /// Returns true, if the underlying wayland object originates from
    /// the same client connection as the provided `ObjectId`.
    ///
    /// *Must* return false, if there is not underlying wayland object.
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        self.wl_surface()
            .map(|s| s.id().same_client_as(object_id))
            .unwrap_or(false)
    }
}

impl WaylandFocus for wl_surface::WlSurface {
    #[inline]
    fn wl_surface(&self) -> Option<Cow<'_, wl_surface::WlSurface>> {
        Some(Cow::Borrowed(self))
    }
}

impl<D: SeatHandler> Inner<D> {
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

    pub(crate) fn send_all_caps(&self) {
        let capabilities = self.compute_caps();
        for seat in &self.known_seats {
            if let Ok(seat) = seat.upgrade() {
                seat.capabilities(capabilities);

                let data = seat.data::<SeatUserData<D>>().unwrap();
                data.sent_capabilities
                    .fetch_or(u32::from(capabilities), Ordering::SeqCst);
            }
        }
    }
}

/// Global data of WlSeat
pub struct SeatGlobalData<D: SeatHandler> {
    arc: Arc<SeatRc<D>>,
}

impl<D: SeatHandler> fmt::Debug for SeatGlobalData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeatGlobalData").field("arc", &self.arc).finish()
    }
}

impl<D: SeatHandler + 'static> SeatState<D> {
    /// Create a new seat global
    ///
    /// A new seat global is created with given name and inserted
    /// into this wayland display.
    ///
    /// You are provided with the state token to retrieve it (allowing
    /// you to add or remove capabilities from it), and the global handle,
    /// in case you want to remove it.
    pub fn new_wl_seat<N>(&mut self, display: &DisplayHandle, name: N) -> Seat<D>
    where
        D: GlobalDispatch<WlSeat, SeatGlobalData<D>> + SeatHandler + 'static,
        <D as SeatHandler>::PointerFocus: WaylandFocus,
        <D as SeatHandler>::KeyboardFocus: WaylandFocus,
        N: Into<String>,
    {
        let Seat { arc } = self.new_seat(name);

        let global_id = display.create_global::<D, _, _>(9, SeatGlobalData { arc: arc.clone() });
        arc.inner.lock().unwrap().global = Some(global_id);

        Seat { arc }
    }
}

impl<D: SeatHandler + 'static> Seat<D> {
    /// Checks whether a given [`WlSeat`] is associated with this [`Seat`]
    pub fn owns(&self, seat: &wl_seat::WlSeat) -> bool {
        let inner = self.arc.inner.lock().unwrap();
        inner.known_seats.iter().any(|s| s == seat)
    }

    /// Attempt to retrieve a [`Seat`] from an existing resource
    pub fn from_resource(seat: &WlSeat) -> Option<Self> {
        Some(Self {
            arc: seat.data::<SeatUserData<D>>()?.arc.upgrade()?,
        })
    }

    /// Retrieves [`WlSeat`] resources for a given client
    pub fn client_seats(&self, client: &Client) -> Vec<WlSeat> {
        self.arc
            .inner
            .lock()
            .unwrap()
            .known_seats
            .iter()
            .filter_map(|w| w.upgrade().ok())
            .filter(|s| s.client().is_some_and(|c| &c == client))
            .collect()
    }

    /// Get the id of WlSeat global
    pub fn global(&self) -> Option<GlobalId> {
        self.arc.inner.lock().unwrap().global.as_ref().cloned()
    }
}

/// User data for seat
pub struct SeatUserData<D: SeatHandler> {
    arc: Weak<SeatRc<D>>,
    sent_capabilities: AtomicU32,
}

impl<D: SeatHandler> fmt::Debug for SeatUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeatUserData").field("arc", &self.arc).finish()
    }
}

impl<D> Dispatch2<WlSeat, D> for SeatUserData<D>
where
    D: Dispatch<WlKeyboard, KeyboardUserData<D>>,
    D: Dispatch<WlPointer, PointerUserData<D>>,
    D: Dispatch<WlTouch, TouchUserData<D>>,
    D: SeatHandler,
    D: CompositorHandler,
    <D as SeatHandler>::PointerFocus: WaylandFocus,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    <D as SeatHandler>::TouchFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        client: &wayland_server::Client,
        seat: &WlSeat,
        request: wl_seat::Request,
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let sent_capabilities =
            wl_seat::Capability::from_bits_retain(self.sent_capabilities.load(Ordering::SeqCst));

        match request {
            wl_seat::Request::GetPointer { id } => {
                if !sent_capabilities.contains(wl_seat::Capability::Pointer) {
                    seat.post_error(wl_seat::Error::MissingCapability, "missing pointer capability");
                    return;
                }

                let ptr_handle = self
                    .arc
                    .upgrade()
                    .and_then(|arc| arc.inner.lock().unwrap().pointer.clone());

                let client_scale = state.client_compositor_state(client).clone_client_scale();
                let pointer = data_init.init(
                    id,
                    PointerUserData {
                        arc: ptr_handle
                            .as_ref()
                            .map_or_else(Weak::new, |h| Arc::downgrade(&h.arc)),
                        client_scale,
                    },
                );

                if let Some(ptr_handle) = &ptr_handle {
                    ptr_handle.arc.wl_pointer.new_pointer::<D>(pointer);
                } else {
                    // we should send a protocol error... but the protocol does not allow
                    // us, so this pointer will just remain inactive ¯\_(ツ)_/¯
                }
            }
            wl_seat::Request::GetKeyboard { id } => {
                if !sent_capabilities.contains(wl_seat::Capability::Keyboard) {
                    seat.post_error(wl_seat::Error::MissingCapability, "missing keyboard capability");
                    return;
                }

                let kbd_handle = self
                    .arc
                    .upgrade()
                    .and_then(|arc| arc.inner.lock().unwrap().keyboard.clone());

                let keyboard = data_init.init(
                    id,
                    KeyboardUserData {
                        arc: kbd_handle
                            .as_ref()
                            .map_or_else(Weak::new, |h| Arc::downgrade(&h.arc)),
                    },
                );

                if let Some(h) = &kbd_handle {
                    h.new_kbd(keyboard);
                } else {
                    // same as pointer, should error but cannot

                    // Protocol spec says this should be sent immediately on creation, so send
                    // for inert object.
                    if keyboard.version() >= 4 {
                        keyboard.repeat_info(0, 0);
                    }
                }
            }
            wl_seat::Request::GetTouch { id } => {
                if !sent_capabilities.contains(wl_seat::Capability::Touch) {
                    seat.post_error(wl_seat::Error::MissingCapability, "missing touch capability");
                    return;
                }

                let touch_handle = self
                    .arc
                    .upgrade()
                    .and_then(|arc| arc.inner.lock().unwrap().touch.clone());

                let client_scale = state.client_compositor_state(client).clone_client_scale();
                let touch = data_init.init(
                    id,
                    TouchUserData {
                        arc: touch_handle
                            .as_ref()
                            .map_or_else(Weak::new, |h| Arc::downgrade(&h.arc)),
                        client_scale,
                    },
                );

                if let Some(h) = &touch_handle {
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

    fn destroyed(&self, _state: &mut D, _: ClientId, seat: &WlSeat) {
        if let Some(arc) = self.arc.upgrade() {
            arc.inner
                .lock()
                .unwrap()
                .known_seats
                .retain(|s| s.id() != seat.id());
        }
    }
}

impl<D> GlobalDispatch2<WlSeat, D> for SeatGlobalData<D>
where
    D: Dispatch<WlSeat, SeatUserData<D>>,
    D: Dispatch<WlKeyboard, KeyboardUserData<D>>,
    D: Dispatch<WlPointer, PointerUserData<D>>,
    D: Dispatch<WlTouch, TouchUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WlSeat>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let mut inner = self.arc.inner.lock().unwrap();

        let capabilities = inner.compute_caps();

        let data = SeatUserData {
            arc: Arc::downgrade(&self.arc),
            sent_capabilities: AtomicU32::new(u32::from(capabilities)),
        };

        let resource = data_init.init(resource, data);

        if resource.version() >= 2 {
            resource.name(self.arc.name.clone());
        }

        resource.capabilities(inner.compute_caps());
        inner.known_seats.push(resource.downgrade());
    }
}
