//! This interface allows clients to monitor user idle status.
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
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
//!
//! // On input you should notify the idle_notifier
//! // state.idle_notifier.notify_activity(&seat);
//! ```

use std::{
    collections::HashMap,
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
pub trait IdleNotifierHandler: Sized + 'static {
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
}

impl IdleNotificationUserData {
    fn take_timer_token(&self) -> Option<RegistrationToken> {
        self.timer_token.lock().unwrap().take()
    }

    fn set_timer_token(&self, idle: Option<RegistrationToken>) {
        *self.timer_token.lock().unwrap() = idle;
    }

    fn set_idle(&self, idle: bool) {
        self.is_idle.store(idle, atomic::Ordering::Release);
    }

    fn is_idle(&self) -> bool {
        self.is_idle.load(atomic::Ordering::Acquire)
    }
}

/// State of ext-idle-notify module
#[derive(Debug)]
pub struct IdleNotifierState<D> {
    global: GlobalId,
    notifications: HashMap<WlSeat, Vec<ExtIdleNotificationV1>>,
    loop_handle: LoopHandle<'static, D>,
    is_inhibited: bool,
}

impl<D: IdleNotifierHandler> IdleNotifierState<D> {
    /// Create new [`ExtIdleNotifierV1`] global.
    pub fn new(display: &DisplayHandle, loop_handle: LoopHandle<'static, D>) -> Self
    where
        D: IdleNotifierHandler,
    {
        let global = display.create_delegated_global::<D, ExtIdleNotifierV1, _, Self>(1, ());
        Self {
            global,
            notifications: HashMap::new(),
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
            if is_inhibited {
                let data = Self::notification_user_data(notification).unwrap();

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
        let Some(notifications) = self.notifications.get(seat) else {
            return;
        };

        for notification in notifications {
            let data = Self::notification_user_data(notification).unwrap();

            if data.is_idle() {
                notification.resumed();
                data.set_idle(false);
            }

            self.reinsert_timer(notification);
        }
    }

    /// Returns the [`ExtIdleNotifierV1`] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    fn notifications(&self) -> impl Iterator<Item = &ExtIdleNotificationV1> {
        self.notifications.values().flatten()
    }

    fn reinsert_timer(&self, notification: &ExtIdleNotificationV1) {
        let data = Self::notification_user_data(notification).unwrap();

        if let Some(token) = data.take_timer_token() {
            self.loop_handle.remove(token);
        }

        if self.is_inhibited {
            return;
        }

        let token = self
            .loop_handle
            .insert_source(calloop::timer::Timer::from_duration(data.timeout), {
                let idle_notification = notification.clone();
                move |_, _, state| {
                    let data = Self::notification_user_data(&idle_notification).unwrap();

                    if !state.idle_notifier_state().is_inhibited && !data.is_idle() {
                        idle_notification.idled();
                        data.set_idle(true);
                    }

                    data.set_timer_token(None);
                    TimeoutAction::Drop
                }
            });

        data.set_timer_token(token.ok());
    }

    fn notification_user_data(res: &ExtIdleNotificationV1) -> Option<&IdleNotificationUserData> {
        res.delegated_data::<_, Self>()
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
    D: IdleNotifierHandler,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtIdleNotifierV1>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }
}

impl<D> Dispatch<ExtIdleNotifierV1, (), D> for IdleNotifierState<D>
where
    D: IdleNotifierHandler,
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

                let idle_notifier_state = state.idle_notifier_state();

                let idle_notification = data_init.init_delegated::<_, _, Self>(
                    id,
                    IdleNotificationUserData {
                        seat: seat.clone(),
                        is_idle: AtomicBool::new(false),
                        timeout,
                        timer_token: Mutex::new(None),
                    },
                );

                idle_notifier_state.reinsert_timer(&idle_notification);

                state
                    .idle_notifier_state()
                    .notifications
                    .entry(seat)
                    .or_default()
                    .push(idle_notification);
            }
            ext_idle_notifier_v1::Request::Destroy => {}
            _ => unimplemented!(),
        }
    }
}

impl<D> Dispatch<ExtIdleNotificationV1, IdleNotificationUserData, D> for IdleNotifierState<D>
where
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
        let state = state.idle_notifier_state();
        if let Some(notifications) = state.notifications.get_mut(&data.seat) {
            notifications.retain(|x| x != notification);
        }

        state
            .notifications
            .retain(|seat, notifications| !notifications.is_empty() && seat.is_alive());
    }
}

/// Macro to delegate implementation of the ext idle notify protocol
#[deprecated(note = "No longer needed, this is now NOP")]
#[macro_export]
macro_rules! delegate_idle_notify {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {};
}
