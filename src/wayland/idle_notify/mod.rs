//! This interface allows clients to monitor user idle status.
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! use smithay::delegate_idle_notify;
//! use smithay::wayland::idle_notify::{IdleNotifierState, IdleNotifierHandler};
//! # use smithay::input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus};
//! # use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
//!
//! struct State { idle_notifier: IdleNotifierState<Self> }
//! # let mut event_loop = smithay::reexports::calloop::EventLoop::<State>::try_new().unwrap();
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the idle_notifier state
//! let idle_notifier = IdleNotifierState::<State>::new(
//!     &display.handle(),
//!     event_loop.handle(),
//! );
//!
//! let state = State { idle_notifier };
//!
//! // Implement the necessary trait
//! # impl SeatHandler for State {
//! #     type KeyboardFocus = WlSurface;
//! #     type PointerFocus = WlSurface;
//! #     type TouchFocus = WlSurface;
//! #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
//! #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//! #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! # }
//! impl IdleNotifierHandler for State {
//!     fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
//!         &mut self.idle_notifier
//!     }
//! }
//! delegate_idle_notify!(State);
//!
//! // On input you should notify the idle_notifier
//! // state.idle_notifier.notify_activity(&seat);
//! ```

use std::{
    sync::{
        atomic::{self, AtomicBool},
        Mutex,
    },
    time::Duration,
};

use calloop::{timer::TimeoutAction, LoopHandle, RegistrationToken};
use wayland_protocols::ext::idle_notify::v1::server::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
};
use wayland_server::{
    backend::{ClientId, GlobalId},
    protocol::wl_seat::WlSeat,
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::input::{Seat, SeatHandler};

/// Handler trait for ext-idle-notify
pub trait IdleNotifierHandler: Sized {
    /// [`IdleNotifierState`] getter
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self>;
}

/// User data of the [`ExtIdleNotificationV1`] resource
#[derive(Debug)]
pub struct IdleNotificationUserData {
    seat: WlSeat,
    is_idle: AtomicBool,
    timeout: Duration,
    timer_token: Mutex<Option<RegistrationToken>>,

    /// If listener was created with `get_input_idle_notification`
    ignore_inhibitor: bool,
}

impl IdleNotificationUserData {
    #[inline]
    fn take_timer_token(&self) -> Option<RegistrationToken> {
        self.timer_token.lock().unwrap().take()
    }

    #[inline]
    fn set_timer_token(&self, idle: Option<RegistrationToken>) {
        *self.timer_token.lock().unwrap() = idle;
    }

    #[inline]
    fn set_idle(&self, idle: bool) {
        self.is_idle.store(idle, atomic::Ordering::Release);
    }

    #[inline]
    fn is_idle(&self) -> bool {
        self.is_idle.load(atomic::Ordering::Acquire)
    }
}

#[derive(Debug, Default)]
struct IdleNotifierSeatState {
    notifications: Vec<ExtIdleNotificationV1>,
    idle_pending: bool,
}

/// State of ext-idle-notify module
#[derive(Debug)]
pub struct IdleNotifierState<D> {
    global: GlobalId,
    seat_state: smallvec::SmallVec<[(WlSeat, IdleNotifierSeatState); 4]>,
    loop_handle: LoopHandle<'static, D>,
    is_inhibited: bool,
}

impl<D: IdleNotifierHandler> IdleNotifierState<D> {
    /// Create new [`ExtIdleNotifierV1`] global.
    pub fn new(display: &DisplayHandle, loop_handle: LoopHandle<'static, D>) -> Self
    where
        D: GlobalDispatch<ExtIdleNotifierV1, ()>,
        D: Dispatch<ExtIdleNotifierV1, ()>,
        D: Dispatch<ExtIdleNotificationV1, IdleNotificationUserData>,
        D: IdleNotifierHandler,
        D: 'static,
    {
        let global = display.create_global::<D, ExtIdleNotifierV1, _>(2, ());
        Self {
            global,
            seat_state: Default::default(),
            loop_handle,
            is_inhibited: false,
        }
    }

    /// Inhibit entering idle state, eg. by the idle-inhibit protocol
    pub fn set_is_inhibited(&mut self, is_inhibited: bool) {
        if self.is_inhibited == is_inhibited {
            return;
        }

        self.is_inhibited = is_inhibited;

        for notification in self.notifications() {
            let data = notification.data::<IdleNotificationUserData>().unwrap();

            if data.ignore_inhibitor {
                continue;
            }

            if is_inhibited {
                if data.is_idle() {
                    notification.resumed();
                    data.set_idle(false);
                }

                if let Some(token) = data.take_timer_token() {
                    self.loop_handle.remove(token);
                }
            } else {
                self.reinsert_timer(notification);
            }
        }
    }

    /// Is idle state inhibited, eg. by the idle-inhibit protocol
    pub fn is_inhibited(&mut self) -> bool {
        self.is_inhibited
    }

    /// Should be called whenever activity occurs on a seat, eg. mouse/keyboard input.
    ///
    /// You may want to use [`Self::notify_activity`] instead which accepts a [`Seat`].
    pub fn notify_activity_for_wl_seat(&mut self, seat: &WlSeat) {
        let Some(seat_index) = self.seat_state_index(seat) else {
            return;
        };

        let (_, seat_state) = unsafe { self.seat_state.get_unchecked(seat_index) };
        for notification in &seat_state.notifications {
            let data = notification.data::<IdleNotificationUserData>().unwrap();

            if data.is_idle() {
                notification.resumed();
                data.set_idle(false);
            }

            if let Some(token) = data.take_timer_token() {
                self.loop_handle.remove(token);
            }
        }

        if seat_state.idle_pending {
            return;
        }

        let seat = seat.clone();
        self.loop_handle.insert_idle(move |state| {
            let idle_notifier_state = state.idle_notifier_state();
            let Some(seat_index) = idle_notifier_state.seat_state_index(&seat) else {
                return;
            };

            let (_, seat_state) = unsafe { idle_notifier_state.seat_state.get_unchecked(seat_index) };
            for notification in &seat_state.notifications {
                idle_notifier_state.reinsert_timer(notification);
            }

            let (_, seat_state) = unsafe { idle_notifier_state.seat_state.get_unchecked_mut(seat_index) };
            seat_state.idle_pending = false;
        });

        let (_, seat_state) = unsafe { self.seat_state.get_unchecked_mut(seat_index) };
        seat_state.idle_pending = true;
    }

    /// Returns the [`ExtIdleNotifierV1`] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    fn seat_state_index(&self, seat: &WlSeat) -> Option<usize> {
        self.seat_state
            .iter()
            .enumerate()
            .find_map(|(index, (s, _))| if s == seat { Some(index) } else { None })
    }

    fn seat_state_mut(&mut self, seat: &WlSeat) -> Option<&mut IdleNotifierSeatState> {
        let index = self.seat_state_index(seat)?;
        unsafe { Some(&mut self.seat_state.get_unchecked_mut(index).1) }
    }

    fn notifications(&self) -> impl Iterator<Item = &ExtIdleNotificationV1> {
        self.seat_state
            .iter()
            .flat_map(|(_, state)| state.notifications.iter())
    }

    fn reinsert_timer(&self, notification: &ExtIdleNotificationV1) {
        let data = notification.data::<IdleNotificationUserData>().unwrap();

        if let Some(token) = data.take_timer_token() {
            self.loop_handle.remove(token);
        }

        if !data.ignore_inhibitor && self.is_inhibited {
            return;
        }

        let token = self
            .loop_handle
            .insert_source(calloop::timer::Timer::from_duration(data.timeout), {
                let idle_notification = notification.clone();
                move |_, _, state| {
                    let data = idle_notification.data::<IdleNotificationUserData>().unwrap();

                    let is_inhibited = !data.ignore_inhibitor && state.idle_notifier_state().is_inhibited;
                    let is_idle_already = data.is_idle();

                    if !is_inhibited && !is_idle_already {
                        idle_notification.idled();
                        data.set_idle(true);
                    }

                    data.set_timer_token(None);
                    TimeoutAction::Drop
                }
            });

        data.set_timer_token(token.ok());
    }
}

impl<D: IdleNotifierHandler + SeatHandler> IdleNotifierState<D> {
    /// Should be called whenever activity occurs on a seat, eg. mouse/keyboard input.
    pub fn notify_activity(&mut self, seat: &Seat<D>) {
        for seat in &seat.arc.inner.lock().unwrap().known_seats {
            if let Ok(seat) = seat.upgrade() {
                self.notify_activity_for_wl_seat(&seat);
            }
        }
    }
}

impl<D> GlobalDispatch<ExtIdleNotifierV1, (), D> for IdleNotifierState<D>
where
    D: GlobalDispatch<ExtIdleNotifierV1, ()>,
    D: Dispatch<ExtIdleNotifierV1, ()>,
    D: Dispatch<ExtIdleNotificationV1, IdleNotificationUserData>,
    D: IdleNotifierHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtIdleNotifierV1>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ExtIdleNotifierV1, (), D> for IdleNotifierState<D>
where
    D: GlobalDispatch<ExtIdleNotifierV1, ()>,
    D: Dispatch<ExtIdleNotifierV1, ()>,
    D: Dispatch<ExtIdleNotificationV1, IdleNotificationUserData>,
    D: IdleNotifierHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &ExtIdleNotifierV1,
        request: ext_idle_notifier_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_idle_notifier_v1::Request::GetIdleNotification { id, timeout, seat } => {
                let timeout = Duration::from_millis(timeout as u64);

                let idle_notification = data_init.init(
                    id,
                    IdleNotificationUserData {
                        seat: seat.clone(),
                        is_idle: AtomicBool::new(false),
                        timeout,
                        timer_token: Mutex::new(None),
                        ignore_inhibitor: false,
                    },
                );

                let idle_notifier_state = state.idle_notifier_state();
                idle_notifier_state.reinsert_timer(&idle_notification);

                if let Some(seat_state) = idle_notifier_state.seat_state_mut(&seat) {
                    seat_state.notifications.push(idle_notification);
                } else {
                    let seat_state = IdleNotifierSeatState {
                        notifications: vec![idle_notification],
                        ..Default::default()
                    };
                    idle_notifier_state.seat_state.push((seat, seat_state));
                };
            }
            ext_idle_notifier_v1::Request::GetInputIdleNotification { id, timeout, seat } => {
                let timeout = Duration::from_millis(timeout as u64);

                let idle_notification = data_init.init(
                    id,
                    IdleNotificationUserData {
                        seat: seat.clone(),
                        is_idle: AtomicBool::new(false),
                        timeout,
                        timer_token: Mutex::new(None),
                        ignore_inhibitor: true,
                    },
                );

                let idle_notifier_state = state.idle_notifier_state();
                idle_notifier_state.reinsert_timer(&idle_notification);

                if let Some(seat_state) = idle_notifier_state.seat_state_mut(&seat) {
                    seat_state.notifications.push(idle_notification);
                } else {
                    let seat_state = IdleNotifierSeatState {
                        notifications: vec![idle_notification],
                        ..Default::default()
                    };
                    idle_notifier_state.seat_state.push((seat, seat_state));
                };
            }
            ext_idle_notifier_v1::Request::Destroy => {}
            _ => unimplemented!(),
        }
    }
}

impl<D> Dispatch<ExtIdleNotificationV1, IdleNotificationUserData, D> for IdleNotifierState<D>
where
    D: Dispatch<ExtIdleNotificationV1, IdleNotificationUserData>,
    D: IdleNotifierHandler,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ExtIdleNotificationV1,
        request: ext_idle_notification_v1::Request,
        _data: &IdleNotificationUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_idle_notification_v1::Request::Destroy => {}
            _ => unimplemented!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        notification: &ExtIdleNotificationV1,
        data: &IdleNotificationUserData,
    ) {
        let idle_notifier_state = state.idle_notifier_state();

        let Some(seat_state) = idle_notifier_state.seat_state_mut(&data.seat) else {
            return;
        };

        seat_state.notifications.retain(|x| x != notification);
        idle_notifier_state
            .seat_state
            .retain(|(seat, state)| !state.notifications.is_empty() && seat.is_alive());
    }
}

/// Macro to delegate implementation of the ext idle notify protocol
#[macro_export]
macro_rules! delegate_idle_notify {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        const _: () = {
            use $crate::{
                reexports::{
                    wayland_protocols::ext::idle_notify::v1::server::{
                        ext_idle_notification_v1::ExtIdleNotificationV1,
                        ext_idle_notifier_v1::ExtIdleNotifierV1,
                    },
                    wayland_server::{delegate_dispatch, delegate_global_dispatch},
                },
                wayland::idle_notify::{IdleNotificationUserData, IdleNotifierState},
            };

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtIdleNotifierV1: ()] => IdleNotifierState<$ty>
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtIdleNotifierV1: ()] => IdleNotifierState<$ty>
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ExtIdleNotificationV1: IdleNotificationUserData] => IdleNotifierState<$ty>
            );
        };
    };
}
