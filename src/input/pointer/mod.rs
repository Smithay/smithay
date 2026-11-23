//! Pointer-related types for smithay's input abstraction

use std::{
    any::TypeId,
    fmt,
    sync::{Arc, Mutex},
};

use crate::{
    backend::input::{Axis, AxisRelativeDirection, AxisSource, ButtonState, InputTime},
    input::{
        Seat, SeatHandler,
        grab_stack::{Deferred, GrabEntry, GrabStack, NewGrab, Position, split_next},
    },
    utils::Serial,
    utils::{IsAlive, Logical, Point},
};

mod cursor_image;
pub use cursor_icon::CursorIcon;
pub use cursor_image::{CursorImageAttributes, CursorImageStatus, CursorImageSurfaceData};

mod grab;
use grab::DefaultGrab;
pub use grab::{ClickGrab, GrabStartData, PointerGrab};
use tracing::{info_span, instrument};

/// An handle to a pointer handler
///
/// It can be cloned and all clones manipulate the same internal state.
///
/// This handle gives you access to an interface to send pointer events to your
/// clients.
///
/// When sending events using this handle, they will be intercepted by a pointer
/// grab if any is active. See the [`PointerGrab`] trait for details.
pub struct PointerHandle<D: SeatHandler> {
    pub(crate) arc: Arc<PointerRc<D>>,
}

type FocusAt<D> = Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>;
type PointerGrabStack<D> = GrabStack<dyn PointerGrab<D>, FocusAt<D>>;

pub(crate) struct PointerRc<D: SeatHandler> {
    pub(crate) inner: Mutex<PointerInternal<D>>,
    // NOTE: If both grab and internal mutex are held, grab should be acquired first.
    // And internal mutex should not be held over a callback.
    grab: Mutex<PointerGrabStack<D>>,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) wl_pointer: crate::wayland::seat::pointer::WlPointerHandle,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) wp_pointer_gestures: crate::wayland::pointer_gestures::WpPointerGesturePointerHandle,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) wp_relative: crate::wayland::relative_pointer::WpRelativePointerHandle,
    pub(crate) span: tracing::Span,
}

#[cfg(not(feature = "wayland_frontend"))]
impl<D: SeatHandler> fmt::Debug for PointerHandle<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerHandle")
            .field("inner", &self.arc.inner)
            .field("grab", &self.arc.grab)
            .finish()
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D: SeatHandler> fmt::Debug for PointerHandle<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerHandle")
            .field("inner", &self.arc.inner)
            .field("grab", &self.arc.grab)
            .field("wl_seat", &self.arc.wl_pointer)
            .field("wp_pointer_gestures", &self.arc.wp_pointer_gestures)
            .field("wp_relative", &self.arc.wp_relative)
            .finish()
    }
}

impl<D: SeatHandler> Clone for PointerHandle<D> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl<D: SeatHandler> std::hash::Hash for PointerHandle<D> {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.arc).hash(state)
    }
}

impl<D: SeatHandler> std::cmp::PartialEq for PointerHandle<D> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}

impl<D: SeatHandler> std::cmp::Eq for PointerHandle<D> {}

/// Trait representing object that can receive pointer interactions
pub trait PointerTarget<D>: IsAlive + fmt::Debug + Send
where
    D: SeatHandler,
{
    /// A pointer of a given seat entered this handler
    fn enter(&self, seat: &Seat<D>, data: &mut D, event: &MotionEvent);
    /// A pointer of a given seat moved over this handler
    fn motion(&self, seat: &Seat<D>, data: &mut D, event: &MotionEvent);
    /// A pointer of a given seat that provides relative motion moved over this handler
    fn relative_motion(&self, seat: &Seat<D>, data: &mut D, event: &RelativeMotionEvent);
    /// A pointer of a given seat clicked a button
    fn button(&self, seat: &Seat<D>, data: &mut D, event: &ButtonEvent);
    /// A pointer of a given seat scrolled on an axis
    fn axis(&self, seat: &Seat<D>, data: &mut D, frame: AxisFrame);
    /// End of a pointer frame
    fn frame(&self, seat: &Seat<D>, data: &mut D);
    /// A pointer of a given seat started a swipe gesture
    fn gesture_swipe_begin(&self, seat: &Seat<D>, data: &mut D, event: &GestureSwipeBeginEvent);
    /// A pointer of a given seat updated a swipe gesture
    fn gesture_swipe_update(&self, seat: &Seat<D>, data: &mut D, event: &GestureSwipeUpdateEvent);
    /// A pointer of a given seat ended a swipe gesture
    fn gesture_swipe_end(&self, seat: &Seat<D>, data: &mut D, event: &GestureSwipeEndEvent);
    /// A pointer of a given seat started a pinch gesture
    fn gesture_pinch_begin(&self, seat: &Seat<D>, data: &mut D, event: &GesturePinchBeginEvent);
    /// A pointer of a given seat updated a pinch gesture
    fn gesture_pinch_update(&self, seat: &Seat<D>, data: &mut D, event: &GesturePinchUpdateEvent);
    /// A pointer of a given seat ended a pinch gesture
    fn gesture_pinch_end(&self, seat: &Seat<D>, data: &mut D, event: &GesturePinchEndEvent);
    /// A pointer of a given seat started a hold gesture
    fn gesture_hold_begin(&self, seat: &Seat<D>, data: &mut D, event: &GestureHoldBeginEvent);
    /// A pointer of a given seat ended a hold gesture
    fn gesture_hold_end(&self, seat: &Seat<D>, data: &mut D, event: &GestureHoldEndEvent);
    /// A pointer of a given seat left this handler
    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial, time: InputTime);
    /// A pointer of a given seat moved from another handler to this handler
    fn replace(
        &self,
        replaced: <D as SeatHandler>::PointerFocus,
        seat: &Seat<D>,
        data: &mut D,
        event: &MotionEvent,
    ) {
        PointerTarget::<D>::leave(&replaced, seat, data, event.serial, event.time);
        data.cursor_image(seat, CursorImageStatus::default_named());
        PointerTarget::<D>::enter(self, seat, data, event);
    }
}

impl<D: SeatHandler + 'static> PointerHandle<D> {
    pub(crate) fn new() -> PointerHandle<D> {
        PointerHandle {
            arc: Arc::new(PointerRc {
                inner: Mutex::new(PointerInternal::new()),
                grab: Mutex::new(GrabStack::new()),
                #[cfg(feature = "wayland_frontend")]
                wl_pointer: crate::wayland::seat::pointer::WlPointerHandle::default(),
                #[cfg(feature = "wayland_frontend")]
                wp_pointer_gestures: crate::wayland::pointer_gestures::WpPointerGesturePointerHandle::default(
                ),
                #[cfg(feature = "wayland_frontend")]
                wp_relative: crate::wayland::relative_pointer::WpRelativePointerHandle::default(),
                span: info_span!("input_pointer"),
            }),
        }
    }

    /// Install a grab on this pointer
    ///
    /// The grab is put on top of the grabs that are already active: it sees every event first,
    /// and what it forwards through its [`PointerInnerHandle`] reaches the grabs below it.
    ///
    /// `serial` names the interaction the grab belongs to. If a grab with that serial is active,
    /// typically the implicit grab of the button press a client is responding to, the new grab
    /// takes its place and every other grab stays. Otherwise the new grab replaces the grabs that
    /// are active. [Persistent](PointerGrab::persistent) grabs are not affected either way.
    ///
    /// If focus is set to [`Focus::Clear`] any currently focused surface will be unfocused.
    #[instrument(level = "debug", parent = &self.arc.span, skip(self, data, grab))]
    pub fn set_grab<G: PointerGrab<D> + 'static>(&self, data: &mut D, grab: G, serial: Serial, focus: Focus) {
        let seat = self.get_seat(data);
        let mut stack = self.arc.grab.lock().unwrap();

        let type_id = TypeId::of::<G>();
        let persistent = grab.persistent();
        for mut displaced in stack.take_displaced(serial, type_id, persistent) {
            displaced.handler.unset(data);
        }

        let (pending_focus, location) = {
            let inner = self.arc.inner.lock().unwrap();
            (inner.pending_focus.clone(), inner.location)
        };
        let seq = stack.push(NewGrab {
            serial,
            type_id,
            persistent,
            pending_focus,
            handler: Box::new(grab),
        });

        if matches!(focus, Focus::Clear) {
            let event = MotionEvent {
                location,
                serial,
                time: InputTime::now(),
            };
            self.run_below(&mut stack, &seat, data, seq, |data, handle| {
                handle.motion(data, None, &event)
            });
        }
    }

    /// Remove the grabs on this pointer, resetting it to the default behavior
    ///
    /// [Persistent](PointerGrab::persistent) grabs are left in place, use
    /// [`unset_grab_of`](Self::unset_grab_of) to remove one of those.
    #[instrument(level = "debug", parent = &self.arc.span, skip(self, data))]
    pub fn unset_grab(&self, data: &mut D, serial: Serial, time: InputTime) {
        let removed = self.arc.grab.lock().unwrap().take_where(|e| !e.meta.persistent);
        for mut grab in removed {
            grab.handler.unset(data);
        }

        // restore the focus
        let (focus, location) = {
            let inner = self.arc.inner.lock().unwrap();
            (inner.pending_focus.clone(), inner.location)
        };
        self.with_root(data, |data, root| {
            root.motion(
                data,
                focus,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            )
        });
    }

    /// Remove the grab of type `G` from this pointer, leaving every other grab in place
    ///
    /// The focus is handed back to the grabs below it. Returns whether there was such a grab.
    #[instrument(level = "debug", parent = &self.arc.span, skip(self, data))]
    pub fn unset_grab_of<G: PointerGrab<D> + 'static>(
        &self,
        data: &mut D,
        serial: Serial,
        time: InputTime,
    ) -> bool {
        let seat = self.get_seat(data);
        let mut stack = self.arc.grab.lock().unwrap();

        let type_id = TypeId::of::<G>();
        let Some(entry) = stack.entries.iter_mut().find(|e| e.meta.type_id == type_id) else {
            return false;
        };
        entry.handler.unset(data);
        entry.meta.removed = true;
        let seq = entry.meta.seq;
        let focus = entry.meta.pending_focus.clone();

        let location = self.arc.inner.lock().unwrap().location;
        let event = MotionEvent {
            location,
            serial,
            time,
        };
        self.run_below(&mut stack, &seat, data, seq, |data, handle| {
            handle.motion(data, focus, &event)
        });
        true
    }

    /// Check if this pointer is currently grabbed with this serial
    pub fn has_grab(&self, serial: Serial) -> bool {
        let stack = self.arc.grab.lock().unwrap();
        stack.entries.iter().any(|e| e.meta.serial == serial)
    }

    /// Check if a grab of type `G` is active on this pointer
    pub fn has_grab_of<G: PointerGrab<D> + 'static>(&self) -> bool {
        let stack = self.arc.grab.lock().unwrap();
        stack.entries.iter().any(|e| e.meta.type_id == TypeId::of::<G>())
    }

    /// Check if this pointer is currently being grabbed
    ///
    /// [Persistent](PointerGrab::persistent) grabs do not count.
    pub fn is_grabbed(&self) -> bool {
        self.arc.grab.lock().unwrap().current().is_some()
    }

    /// Returns the start data for the grab, if any.
    ///
    /// See [`with_grab`](Self::with_grab) for which grab that is when several are active.
    pub fn grab_start_data(&self) -> Option<GrabStartData<D>> {
        self.with_grab(|_, g| g.start_data().clone())
    }

    /// Returns the start data for the grab that was started with `serial`, if any.
    pub fn grab_start_data_for(&self, serial: Serial) -> Option<GrabStartData<D>> {
        let stack = self.arc.grab.lock().unwrap();
        stack
            .entries
            .iter()
            .find(|e| e.meta.serial == serial)
            .map(|e| e.handler.start_data().clone())
    }

    /// Calls `f` with the active grab, if any.
    ///
    /// If several grabs are active, this is the most recently started one,
    /// [persistent](PointerGrab::persistent) grabs aside.
    pub fn with_grab<T>(&self, f: impl FnOnce(Serial, &dyn PointerGrab<D>) -> T) -> Option<T> {
        let stack = self.arc.grab.lock().unwrap();
        stack.current().map(|e| f(e.meta.serial, &*e.handler))
    }

    /// Run `f` with a handle that stands above every grab, which is where events enter.
    fn with_root<R>(&self, data: &mut D, f: impl FnOnce(&mut D, &mut PointerInnerHandle<'_, D>) -> R) -> R {
        let mut stack = self.arc.grab.lock().unwrap();

        // Discard the grabs that are associated with a surface that is no longer alive
        let dead = stack.take_where(|e| {
            e.handler
                .start_data()
                .focus
                .as_ref()
                .is_some_and(|(focus, _)| !focus.alive())
        });
        for mut grab in dead {
            grab.handler.unset(data);
        }

        let seat = self.get_seat(data);
        let mut deferred = Vec::new();
        let ret = {
            let mut root = PointerInnerHandle {
                inner: &self.arc.inner,
                seat: &seat,
                position: Position::Root,
                below: &mut stack.entries[..],
                deferred: &mut deferred,
                under_interaction: false,
            };
            f(data, &mut root)
        };

        for mut displaced in stack.finish(deferred) {
            displaced.handler.unset(data);
        }
        ret
    }

    /// Run `f` with the handle of the grab `seq`, that is on the grabs below it.
    fn run_below<R>(
        &self,
        stack: &mut PointerGrabStack<D>,
        seat: &Seat<D>,
        data: &mut D,
        seq: u64,
        f: impl FnOnce(&mut D, &mut PointerInnerHandle<'_, D>) -> R,
    ) -> Option<R> {
        let mut deferred = Vec::new();
        let ret = {
            let (meta, below) = stack.split_at_seq(seq)?;
            let mut handle = PointerInnerHandle {
                inner: &self.arc.inner,
                seat,
                position: Position::Grab(meta),
                below,
                deferred: &mut deferred,
                under_interaction: false,
            };
            f(data, &mut handle)
        };

        for mut displaced in stack.finish(deferred) {
            displaced.handler.unset(data);
        }
        Some(ret)
    }

    /// Notify that the pointer moved
    ///
    /// You provide the new location of the pointer, in the form of:
    ///
    /// - The coordinates of the pointer in the global compositor space
    /// - The surface on top of which the cursor is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the pointer is not
    ///   on top of a client surface).
    ///
    /// This will internally take care of notifying the appropriate client objects
    /// of enter/motion/leave events.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data, focus), fields(focus = ?focus.as_ref().map(|(_, loc)| ("...", loc))))]
    pub fn motion(
        &self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        self.with_root(data, |data, root| {
            root.inner.lock().unwrap().pending_focus.clone_from(&focus);
            root.motion(data, focus, event);
        });
    }

    /// Notify about relative pointer motion
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the relative pointer protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data, focus), fields(focus = ?focus.as_ref().map(|(_, loc)| ("...", loc))))]
    pub fn relative_motion(
        &self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        self.with_root(data, |data, root| {
            root.inner.lock().unwrap().pending_focus.clone_from(&focus);
            root.relative_motion(data, focus, event);
        });
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn button(&self, data: &mut D, event: &ButtonEvent) {
        self.with_root(data, |data, handle| {
            match event.state {
                ButtonState::Pressed => {
                    handle.inner.lock().unwrap().pressed_buttons.push(event.button);
                }
                ButtonState::Released => {
                    handle
                        .inner
                        .lock()
                        .unwrap()
                        .pressed_buttons
                        .retain(|b| *b != event.button);
                }
            }
            handle.button(data, event);
        });
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple scroll events as if they happened in the same instance.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn axis(&self, data: &mut D, details: AxisFrame) {
        self.with_root(data, |data, root| root.axis(data, details));
    }

    /// End of a pointer frame
    ///
    /// A frame groups associated events. This terminates the frame.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn frame(&self, data: &mut D) {
        self.with_root(data, |data, root| root.frame(data));
    }

    /// Notify about swipe gesture begin
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_swipe_begin(&self, data: &mut D, event: &GestureSwipeBeginEvent) {
        self.with_root(data, |data, root| root.gesture_swipe_begin(data, event));
    }

    /// Notify about swipe gesture update
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_swipe_update(&self, data: &mut D, event: &GestureSwipeUpdateEvent) {
        self.with_root(data, |data, root| root.gesture_swipe_update(data, event));
    }

    /// Notify about swipe gesture end
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_swipe_end(&self, data: &mut D, event: &GestureSwipeEndEvent) {
        self.with_root(data, |data, root| root.gesture_swipe_end(data, event));
    }

    /// Notify about pinch gesture begin
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_pinch_begin(&self, data: &mut D, event: &GesturePinchBeginEvent) {
        self.with_root(data, |data, root| root.gesture_pinch_begin(data, event));
    }

    /// Notify about pinch gesture update
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_pinch_update(&self, data: &mut D, event: &GesturePinchUpdateEvent) {
        self.with_root(data, |data, root| root.gesture_pinch_update(data, event));
    }

    /// Notify about pinch gesture end
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_pinch_end(&self, data: &mut D, event: &GesturePinchEndEvent) {
        self.with_root(data, |data, root| root.gesture_pinch_end(data, event));
    }

    /// Notify about hold gesture begin
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_hold_begin(&self, data: &mut D, event: &GestureHoldBeginEvent) {
        self.with_root(data, |data, root| root.gesture_hold_begin(data, event));
    }

    /// Notify about hold gesture end
    ///
    /// This will internally send the appropriate event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    #[instrument(level = "trace", parent = &self.arc.span, skip(self, data))]
    pub fn gesture_hold_end(&self, data: &mut D, event: &GestureHoldEndEvent) {
        self.with_root(data, |data, root| root.gesture_hold_end(data, event));
    }

    /// Access the current location of this pointer in the global space
    pub fn current_location(&self) -> Point<f64, Logical> {
        self.arc.inner.lock().unwrap().location
    }

    /// Update the current location of this pointer in the global space,
    /// without sending any event and without updating the focus.
    ///
    /// If you want to update the location, and update the focus,
    /// and send events, use [Self::motion] instead of this.
    ///
    /// This is useful when the pointer is only moved by relative events,
    /// such as when a pointer lock is held by the focused surface.
    /// The client can give us a cursor position hint, which corresponds to
    /// the actual location the client may be rendering a pointer at.
    /// This position hint should be used as the initial location
    /// when the pointer lock is deactivated.
    ///
    /// The next time [Self::motion] is called, the focus will be
    /// updated accordingly as if this function was never called.
    /// Clients will never be notified of a location hint.
    pub fn set_location(&self, location: Point<f64, Logical>) {
        self.arc.inner.lock().unwrap().location = location;
    }

    /// Access the [`Serial`] of the last `pointer_enter` event, if that focus is still active.
    ///
    /// In other words this will return `None` again, once a `pointer_leave` event occurred.
    #[cfg(feature = "wayland_frontend")]
    pub fn last_enter(&self) -> Option<Serial> {
        *self.arc.wl_pointer.last_enter.lock().unwrap()
    }

    fn get_seat(&self, data: &mut D) -> Seat<D> {
        let seat_state = data.seat_state();
        seat_state
            .seats
            .iter()
            .find(|seat| seat.get_pointer().map(|h| &h == self).unwrap_or(false))
            .cloned()
            .unwrap()
    }
}

impl<D> PointerHandle<D>
where
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: Clone,
{
    /// Retrieve the current pointer focus
    pub fn current_focus(&self) -> Option<<D as SeatHandler>::PointerFocus> {
        self.arc
            .inner
            .lock()
            .unwrap()
            .focus
            .clone()
            .map(|(focus, _)| focus)
    }
}

/// This inner handle is accessed from inside a pointer grab logic
///
/// It stands for everything below the grab it is handed to: its event methods pass an event on to
/// the next grab that is active on this pointer, and, below the last of them, send it to the client.
pub struct PointerInnerHandle<'a, D: SeatHandler> {
    inner: &'a Mutex<PointerInternal<D>>,
    seat: &'a Seat<D>,
    position: Position<'a, FocusAt<D>>,
    below: &'a mut [GrabEntry<dyn PointerGrab<D>, FocusAt<D>>],
    deferred: &'a mut Vec<Deferred<dyn PointerGrab<D>, FocusAt<D>>>,
    /// Whether the event got here by being passed on by an explicit grab that is still active.
    under_interaction: bool,
}

/// What an event that is passed on reaches next.
enum Next<'a, D: SeatHandler> {
    Grab(&'a mut dyn PointerGrab<D>, PointerInnerHandle<'a, D>),
    Default(PointerInnerHandle<'a, D>),
    Device(&'a Mutex<PointerInternal<D>>, &'a Seat<D>),
}

impl<D: SeatHandler> fmt::Debug for PointerInnerHandle<'_, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerInnerHandle")
            .field("inner", &self.inner)
            .field("seat", &self.seat.arc.name)
            .finish()
    }
}

/// Pass an event on to whatever comes next.
macro_rules! pass_on {
    ($self:ident, $data:ident, $method:ident($($arg:expr),*), |$inner:ident, $seat:ident| $device:expr) => {
        match $self.descend() {
            Next::Grab(grab, mut handle) => grab.$method($data, &mut handle, $($arg),*),
            Next::Default(mut handle) => DefaultGrab.$method($data, &mut handle, $($arg),*),
            Next::Device($inner, $seat) => $device,
        }
    };
}

impl<D: SeatHandler + 'static> PointerInnerHandle<'_, D> {
    fn descend(&mut self) -> Next<'_, D> {
        let under_interaction = match &self.position {
            // The implicit grab is what the default grab handed over to, and like the default
            // grab it has nothing below it but the pointer itself.
            Position::Default => return Next::Device(self.inner, self.seat),
            Position::Grab(meta) if meta.implicit => return Next::Device(self.inner, self.seat),
            Position::Grab(meta) => self.under_interaction || !(meta.removed || meta.persistent),
            Position::Root => false,
        };

        match split_next(&mut *self.below, false) {
            Some((meta, grab, below)) => Next::Grab(
                grab,
                PointerInnerHandle {
                    inner: self.inner,
                    seat: self.seat,
                    position: Position::Grab(meta),
                    below,
                    deferred: &mut *self.deferred,
                    under_interaction,
                },
            ),
            None => Next::Default(PointerInnerHandle {
                inner: self.inner,
                seat: self.seat,
                position: Position::Default,
                below: &mut [],
                deferred: &mut *self.deferred,
                under_interaction,
            }),
        }
    }

    /// The focus candidate that last reached the grab this handle belongs to.
    fn pending_focus(&self) -> FocusAt<D> {
        match &self.position {
            Position::Grab(meta) => meta.pending_focus.clone(),
            _ => self.inner.lock().unwrap().pending_focus.clone(),
        }
    }

    fn remember(&mut self, focus: &FocusAt<D>) {
        if let Position::Grab(meta) = &mut self.position {
            meta.pending_focus.clone_from(focus);
        }
    }

    /// Change the grab this handle belongs to to the provided grab
    ///
    /// The new grab takes the place of the current one among the active grabs. Called by the
    /// default grab, this installs the implicit grab.
    pub fn set_grab<G: PointerGrab<D> + 'static>(
        &mut self,
        handler: &mut dyn PointerGrab<D>,
        data: &mut D,
        serial: Serial,
        focus: Focus,
        grab: G,
    ) {
        handler.unset(data);

        let new = NewGrab {
            serial,
            type_id: TypeId::of::<G>(),
            persistent: grab.persistent(),
            pending_focus: self.pending_focus(),
            handler: Box::new(grab) as Box<dyn PointerGrab<D>>,
        };
        match &mut self.position {
            Position::Grab(meta) => {
                meta.removed = true;
                self.deferred.push(Deferred::Replace(meta.seq, new));
            }
            Position::Default | Position::Root => self.deferred.push(Deferred::Implicit(new)),
        }

        if matches!(focus, Focus::Clear) {
            let location = self.current_location();
            self.motion(
                data,
                None,
                &MotionEvent {
                    location,
                    serial,
                    time: InputTime::now(),
                },
            );
        }
    }

    /// End the grab this handle belongs to, leaving every other grab in place
    ///
    /// This will also restore the focus of the underlying pointer if restore_focus
    /// is [`true`]. The grabs below, if any, get to decide on it again.
    pub fn unset_grab(
        &mut self,
        handler: &mut dyn PointerGrab<D>,
        data: &mut D,
        serial: Serial,
        time: InputTime,
        restore_focus: bool,
    ) {
        handler.unset(data);
        if let Position::Grab(meta) = &mut self.position {
            meta.removed = true;
        }

        if restore_focus {
            let focus = self.pending_focus();
            let location = self.current_location();
            self.motion(
                data,
                focus,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            );
        }
    }

    /// Access the current focus of this pointer
    pub fn current_focus(&self) -> Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)> {
        self.inner.lock().unwrap().focus.clone()
    }

    /// Access the current location of this pointer in the global space
    pub fn current_location(&self) -> Point<f64, Logical> {
        self.inner.lock().unwrap().location
    }

    /// A list of the currently physically pressed buttons
    ///
    /// This still includes buttons that your grab have intercepted and not sent
    /// to the client.
    pub fn current_pressed(&self) -> Vec<u32> {
        self.inner.lock().unwrap().pressed_buttons.clone()
    }

    /// Notify that the pointer moved
    ///
    /// You provide the new location of the pointer, in the form of:
    ///
    /// - The coordinates of the pointer in the global compositor space
    /// - The surface on top of which the cursor is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the pointer is not
    ///   on top of a client surface).
    ///
    /// This passes the motion on to the grabs below, which may alter it again, and will finally
    /// take care of notifying the appropriate client objects of enter/motion/leave events.
    pub fn motion(
        &mut self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        match self.descend() {
            Next::Grab(grab, mut handle) => {
                handle.remember(&focus);
                grab.motion(data, &mut handle, focus, event)
            }
            Next::Default(mut handle) => DefaultGrab.motion(data, &mut handle, focus, event),
            Next::Device(inner, seat) => inner.lock().unwrap().motion(data, seat, focus, event),
        }
    }

    /// Notify about relative pointer motion
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the relative pointer protocol.
    pub fn relative_motion(
        &mut self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        match self.descend() {
            Next::Grab(grab, mut handle) => {
                handle.remember(&focus);
                grab.relative_motion(data, &mut handle, focus, event)
            }
            Next::Default(mut handle) => DefaultGrab.relative_motion(data, &mut handle, focus, event),
            Next::Device(inner, seat) => inner.lock().unwrap().relative_motion(data, seat, focus, event),
        }
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&mut self, data: &mut D, event: &ButtonEvent) {
        pass_on!(self, data, button(event), |inner, seat| {
            if let Some((focused, _)) = inner.lock().unwrap().focus.as_mut() {
                focused.button(seat, data, event);
            }
        })
    }

    /// Notify that an axis was scrolled
    ///
    /// This will internally send the appropriate axis events to the client
    /// objects matching with the currently focused surface.
    pub fn axis(&mut self, data: &mut D, details: AxisFrame) {
        pass_on!(self, data, axis(details), |inner, seat| {
            if let Some((focused, _)) = inner.lock().unwrap().focus.as_mut() {
                focused.axis(seat, data, details);
            }
        })
    }

    /// End of a pointer frame
    ///
    /// This will internally send the appropriate frame event to the client
    /// objects matching with the currently focused surface.
    pub fn frame(&mut self, data: &mut D) {
        pass_on!(self, data, frame(), |inner, seat| {
            if let Some((focused, _)) = inner.lock().unwrap().focus.as_mut() {
                focused.frame(seat, data);
            }
        })
    }

    /// Notify about swipe gesture begin
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_swipe_begin(&mut self, data: &mut D, event: &GestureSwipeBeginEvent) {
        pass_on!(self, data, gesture_swipe_begin(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_swipe_begin(data, seat, event))
    }

    /// Notify about swipe gesture update
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_swipe_update(&mut self, data: &mut D, event: &GestureSwipeUpdateEvent) {
        pass_on!(self, data, gesture_swipe_update(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_swipe_update(data, seat, event))
    }

    /// Notify about swipe gesture end
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_swipe_end(&mut self, data: &mut D, event: &GestureSwipeEndEvent) {
        pass_on!(self, data, gesture_swipe_end(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_swipe_end(data, seat, event))
    }

    /// Notify about pinch gesture begin
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_pinch_begin(&mut self, data: &mut D, event: &GesturePinchBeginEvent) {
        pass_on!(self, data, gesture_pinch_begin(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_pinch_begin(data, seat, event))
    }

    /// Notify about pinch gesture update
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_pinch_update(&mut self, data: &mut D, event: &GesturePinchUpdateEvent) {
        pass_on!(self, data, gesture_pinch_update(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_pinch_update(data, seat, event))
    }

    /// Notify about pinch gesture end
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_pinch_end(&mut self, data: &mut D, event: &GesturePinchEndEvent) {
        pass_on!(self, data, gesture_pinch_end(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_pinch_end(data, seat, event))
    }

    /// Notify about hold gesture begin
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_hold_begin(&mut self, data: &mut D, event: &GestureHoldBeginEvent) {
        pass_on!(self, data, gesture_hold_begin(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_hold_begin(data, seat, event))
    }

    /// Notify about hold gesture end
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface, if the client uses
    /// the pointer gestures protocol.
    pub fn gesture_hold_end(&mut self, data: &mut D, event: &GestureHoldEndEvent) {
        pass_on!(self, data, gesture_hold_end(event), |inner, seat| inner
            .lock()
            .unwrap()
            .gesture_hold_end(data, seat, event))
    }
}

pub(crate) struct PointerInternal<D: SeatHandler> {
    pub(crate) focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
    pending_focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
    pub(crate) location: Point<f64, Logical>,
    pressed_buttons: Vec<u32>,
}

// image_callback does not implement debug, so we have to impl Debug manually
impl<D: SeatHandler> fmt::Debug for PointerInternal<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerInternal")
            .field("focus", &self.focus)
            .field("pending_focus", &self.pending_focus)
            .field("location", &self.location)
            .field("pressed_buttons", &self.pressed_buttons)
            .field("image_callback", &"...")
            .finish()
    }
}

impl<D: SeatHandler + 'static> PointerInternal<D> {
    fn new() -> Self {
        Self {
            focus: None,
            pending_focus: None,
            location: (0.0, 0.0).into(),
            pressed_buttons: Vec::new(),
        }
    }

    fn motion(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        self.location = event.location;
        if let Some((focus, loc)) = focus {
            let event = MotionEvent {
                location: event.location - loc,
                serial: event.serial,
                time: event.time,
            };
            let old_focus = self.focus.replace((focus.clone(), loc));
            match (focus, old_focus) {
                (focus, Some((old_focus, _))) if focus == old_focus => {
                    // we were on top of a target and remained on it
                    focus.motion(seat, data, &event);
                }
                (focus, Some((old_focus, _))) => {
                    // the target has been replaced
                    focus.replace(old_focus, seat, data, &event);
                }
                (focus, None) => {
                    // we entered a new target
                    focus.enter(seat, data, &event);
                }
            };
        } else if let Some((old_focus, _)) = self.focus.take() {
            old_focus.leave(seat, data, event.serial, event.time);
            data.cursor_image(seat, CursorImageStatus::default_named());
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        _focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.relative_motion(seat, data, event);
        }
    }

    fn gesture_swipe_begin(&mut self, data: &mut D, seat: &Seat<D>, event: &GestureSwipeBeginEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_swipe_begin(seat, data, event);
        }
    }

    fn gesture_swipe_update(&mut self, data: &mut D, seat: &Seat<D>, event: &GestureSwipeUpdateEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_swipe_update(seat, data, event);
        }
    }

    fn gesture_swipe_end(&mut self, data: &mut D, seat: &Seat<D>, event: &GestureSwipeEndEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_swipe_end(seat, data, event);
        }
    }

    fn gesture_pinch_begin(&mut self, data: &mut D, seat: &Seat<D>, event: &GesturePinchBeginEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_pinch_begin(seat, data, event);
        }
    }

    fn gesture_pinch_update(&mut self, data: &mut D, seat: &Seat<D>, event: &GesturePinchUpdateEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_pinch_update(seat, data, event);
        }
    }

    fn gesture_pinch_end(&mut self, data: &mut D, seat: &Seat<D>, event: &GesturePinchEndEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_pinch_end(seat, data, event);
        }
    }

    fn gesture_hold_begin(&mut self, data: &mut D, seat: &Seat<D>, event: &GestureHoldBeginEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_hold_begin(seat, data, event);
        }
    }

    fn gesture_hold_end(&mut self, data: &mut D, seat: &Seat<D>, event: &GestureHoldEndEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.gesture_hold_end(seat, data, event);
        }
    }
}

/// Defines the focus behavior for [`PointerHandle::set_grab`]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Focus {
    /// Keep the current focus
    Keep,
    /// Clear the current focus
    Clear,
}

/// Pointer motion event
#[derive(Debug, Clone)]
pub struct MotionEvent {
    /// Location of the pointer in compositor space
    pub location: Point<f64, Logical>,
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
}

/// Relative pointer motion event
#[derive(Debug, Clone)]
pub struct RelativeMotionEvent {
    /// Motional vector
    pub delta: Point<f64, Logical>,
    /// Unaccelerated motion vector
    pub delta_unaccel: Point<f64, Logical>,
    /// Timestamp in microseconds
    pub time: InputTime,
}

/// Pointer button event
///
/// Mouse button click and release notifications.
/// The location of the click is given by the last motion or enter event.
#[derive(Debug, Clone, Copy)]
pub struct ButtonEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp with microsecond granularity, with an undefined base.
    pub time: InputTime,
    /// Button that produced the event
    ///
    /// The button is a button code as defined in the
    /// Linux kernel's linux/input-event-codes.h header file, e.g. BTN_LEFT.
    ///
    /// Any 16-bit button code value is reserved for future additions to the kernel's event code list. All other button codes above 0xFFFF are currently undefined but may be used in future versions of this protocol.
    pub button: u32,
    /// Physical state of the button
    pub state: ButtonState,
}

/// A frame of pointer axis events.
/// Frames of axis events should be considers as one logical action.
///
/// Can be used with the builder pattern, e.g.:
///
/// ```ignore
/// AxisFrame::new()
///     .source(AxisSource::Wheel)
///     .discrete(Axis::Vertical, 6)
///     .value(Axis::Vertical, 30, time)
///     .stop(Axis::Vertical);
/// ```
#[must_use = "AxisFrame uses a builder-like pattern, so its result must be used"]
#[derive(Copy, Clone, Debug)]
pub struct AxisFrame {
    /// Source of the axis event, if known
    pub source: Option<AxisSource>,
    /// Direction of the physical motion that caused axis event
    pub relative_direction: (AxisRelativeDirection, AxisRelativeDirection),
    /// Time of the axis event
    pub time: InputTime,
    /// Raw scroll value per axis of the event
    pub axis: (f64, f64),
    /// Discrete representation of scroll value per axis, if available
    pub v120: Option<(i32, i32)>,
    /// If the axis is considered having stopped movement
    ///
    /// Only useful in conjunction of AxisSource::Finger events
    pub stop: (bool, bool),
}

/// Gesture swipe begin event
#[derive(Debug, Clone)]
pub struct GestureSwipeBeginEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Number of fingers of the event
    pub fingers: u32,
}

/// Gesture swipe update event
#[derive(Debug, Clone)]
pub struct GestureSwipeUpdateEvent {
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Offset of the logical center of the gesture relative to the previous event
    pub delta: Point<f64, Logical>,
}

/// Gesture swipe end event
#[derive(Debug, Clone)]
pub struct GestureSwipeEndEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Whether the gesture was cancelled
    pub cancelled: bool,
}

/// Gesture pinch begin event
#[derive(Debug, Clone)]
pub struct GesturePinchBeginEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Number of fingers of the event
    pub fingers: u32,
}

/// Gesture pinch update event
#[derive(Debug, Clone)]
pub struct GesturePinchUpdateEvent {
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Offset of the logical center of the gesture relative to the previous event
    pub delta: Point<f64, Logical>,
    /// Absolute scale compared to the begin event
    pub scale: f64,
    /// Relative angle in degrees clockwise compared to the previous event
    pub rotation: f64,
}

/// Gesture pinch end event
#[derive(Debug, Clone)]
pub struct GesturePinchEndEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Whether the gesture was cancelled
    pub cancelled: bool,
}

/// Gesture hold begin event
#[derive(Debug, Clone)]
pub struct GestureHoldBeginEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Number of fingers of the event
    pub fingers: u32,
}

/// Gesture hold end event
#[derive(Debug, Clone)]
pub struct GestureHoldEndEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with microsecond granularity
    pub time: InputTime,
    /// Whether the gesture was cancelled
    pub cancelled: bool,
}

impl AxisFrame {
    /// Create a new frame of axis events
    pub fn new(time: InputTime) -> Self {
        AxisFrame {
            source: None,
            relative_direction: (AxisRelativeDirection::Identical, AxisRelativeDirection::Identical),
            time,
            axis: (0.0, 0.0),
            v120: None,
            stop: (false, false),
        }
    }

    /// Specify the source of the axis events
    ///
    /// This event is optional, if no source is known, you can ignore this call.
    /// Only one source event is allowed per frame.
    ///
    /// Using the [`AxisSource::Finger`] requires a stop event to be send,
    /// when the user lifts off the finger (not necessarily in the same frame).
    pub fn source(mut self, source: AxisSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Specify the direction of the physical motion, relative to axis direction
    pub fn relative_direction(mut self, axis: Axis, relative_direction: AxisRelativeDirection) -> Self {
        match axis {
            Axis::Horizontal => {
                self.relative_direction.0 = relative_direction;
            }
            Axis::Vertical => {
                self.relative_direction.1 = relative_direction;
            }
        };
        self
    }

    /// Specify discrete scrolling steps additionally to the computed value.
    ///
    /// This event is optional and gives the client additional information about
    /// the nature of the axis event. E.g. a scroll wheel might issue separate steps,
    /// while a touchpad may never issue this event as it has no steps.
    pub fn v120(mut self, axis: Axis, steps: i32) -> Self {
        let v120 = self.v120.get_or_insert_with(Default::default);
        match axis {
            Axis::Horizontal => {
                v120.0 = steps;
            }
            Axis::Vertical => {
                v120.1 = steps;
            }
        };
        self
    }

    /// The actual scroll value. This event is the only required one, but can also
    /// be send multiple times. The values off one frame will be accumulated by the client.
    pub fn value(mut self, axis: Axis, value: f64) -> Self {
        match axis {
            Axis::Horizontal => {
                self.axis.0 += value;
            }
            Axis::Vertical => {
                self.axis.1 += value;
            }
        };
        self
    }

    /// Notification of stop of scrolling on an axis.
    ///
    /// This event is required for sources of the [`AxisSource::Finger`] type
    /// and otherwise optional.
    pub fn stop(mut self, axis: Axis) -> Self {
        match axis {
            Axis::Horizontal => {
                self.stop.0 = true;
            }
            Axis::Vertical => {
                self.stop.1 = true;
            }
        };
        self
    }
}
