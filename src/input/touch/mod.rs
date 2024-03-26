//! Touch-related types for smithay's input abstraction

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::{info_span, instrument};

use crate::backend::input::TouchSlot;
use crate::utils::{IsAlive, Logical, Point, Serial, SerialCounter};

use self::grab::GrabStatus;
pub use grab::{DefaultGrab, GrabStartData, TouchDownGrab, TouchGrab};

use super::{Seat, SeatHandler};

mod grab;

/// An handle to a touch handler
///
/// It can be cloned and all clones manipulate the same internal state.
///
/// This handle gives you access to an interface to send touch events to your
/// clients.
///
/// When sending events using this handle, they will be intercepted by a touch
/// grab if any is active. See the [`TouchGrab`] trait for details.
pub struct TouchHandle<D: SeatHandler> {
    pub(crate) inner: Arc<Mutex<TouchInternal<D>>>,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) known_instances:
        Arc<Mutex<Vec<(wayland_server::protocol::wl_touch::WlTouch, Option<Serial>)>>>,
    pub(crate) span: tracing::Span,
}

#[cfg(not(feature = "wayland_frontend"))]
impl<D: SeatHandler> fmt::Debug for TouchHandle<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TouchHandle").field("inner", &self.inner).finish()
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D: SeatHandler> fmt::Debug for TouchHandle<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TouchHandle")
            .field("inner", &self.inner)
            .field("known_instances", &self.known_instances)
            .finish()
    }
}

impl<D: SeatHandler> Clone for TouchHandle<D> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            #[cfg(feature = "wayland_frontend")]
            known_instances: self.known_instances.clone(),
            span: self.span.clone(),
        }
    }
}

impl<D: SeatHandler> std::hash::Hash for TouchHandle<D> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.inner).hash(state)
    }
}

impl<D: SeatHandler> std::cmp::PartialEq for TouchHandle<D> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl<D: SeatHandler> std::cmp::Eq for TouchHandle<D> {}

pub(crate) struct TouchInternal<D: SeatHandler> {
    focus: HashMap<TouchSlot, TouchSlotState<D>>,
    seq_counter: SerialCounter,
    default_grab: Box<dyn Fn() -> Box<dyn TouchGrab<D>> + Send + 'static>,
    grab: GrabStatus<D>,
}

struct TouchSlotState<D: SeatHandler> {
    focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
    pending: Serial,
    current: Option<Serial>,
}

impl<D: SeatHandler> fmt::Debug for TouchSlotState<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TouchSlotState")
            .field("focus", &self.focus)
            .field("pending", &self.pending)
            .field("current", &self.current)
            .finish()
    }
}

// image_callback does not implement debug, so we have to impl Debug manually
impl<D: SeatHandler> fmt::Debug for TouchInternal<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TouchInternal")
            .field("focus", &self.focus)
            .field("grab", &self.grab)
            .finish()
    }
}

/// Pointer motion event
#[derive(Debug, Clone)]
pub struct DownEvent {
    /// Slot of this event
    pub slot: TouchSlot,
    /// Location of the touch in compositor space
    pub location: Point<f64, Logical>,
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Pointer motion event
#[derive(Debug, Clone)]
pub struct UpEvent {
    /// Slot of this event
    pub slot: TouchSlot,
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Pointer motion event
#[derive(Debug, Clone)]
pub struct MotionEvent {
    /// Slot of this event
    pub slot: TouchSlot,
    /// Location of the touch in compositor space
    pub location: Point<f64, Logical>,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Pointer motion event
#[derive(Debug, Clone, Copy)]
pub struct ShapeEvent {
    /// Slot of this event
    pub slot: TouchSlot,
    /// Length of the major axis in surface-local coordinates
    pub major: f64,
    /// Length of the minor axis in surface-local coordinates
    pub minor: f64,
}

/// Pointer motion event
#[derive(Debug, Clone, Copy)]
pub struct OrientationEvent {
    /// Slot of this event
    pub slot: TouchSlot,
    /// Angle between major axis and positive surface y-axis in degrees
    pub orientation: f64,
}

/// Trait representing object that can receive touch interactions
pub trait TouchTarget<D>: IsAlive + fmt::Debug + Send
where
    D: SeatHandler,
{
    /// A new touch point has appeared on the target.
    ///
    /// This touch point is assigned a unique ID. Future events from this touch point reference this ID.
    /// The ID ceases to be valid after a touch up event and may be reused in the future.
    fn down(&self, seat: &Seat<D>, data: &mut D, event: &DownEvent, seq: Serial);

    /// The touch point has disappeared.
    ///
    /// No further events will be sent for this touch point and the touch point's ID
    /// is released and may be reused in a future touch down event.
    fn up(&self, seat: &Seat<D>, data: &mut D, event: &UpEvent, seq: Serial);

    /// A touch point has changed coordinates.
    fn motion(&self, seat: &Seat<D>, data: &mut D, event: &MotionEvent, seq: Serial);

    /// Indicates the end of a set of events that logically belong together.
    fn frame(&self, seat: &Seat<D>, data: &mut D, seq: Serial);

    /// Touch session cancelled.
    ///
    /// Touch cancellation applies to all touch points currently active on this target.
    /// The client is responsible for finalizing the touch points, future touch points on
    /// this target may reuse the touch point ID.
    fn cancel(&self, seat: &Seat<D>, data: &mut D, seq: Serial);

    /// Sent when a touch point has changed its shape.
    ///
    /// A touch point shape is approximated by an ellipse through the major and minor axis length.
    /// The major axis length describes the longer diameter of the ellipse, while the minor axis
    /// length describes the shorter diameter. Major and minor are orthogonal and both are specified
    /// in surface-local coordinates. The center of the ellipse is always at the touch point location
    /// as reported by [`TouchTarget::down`] or [`TouchTarget::motion`].
    fn shape(&self, seat: &Seat<D>, data: &mut D, event: &ShapeEvent, seq: Serial);

    /// Sent when a touch point has changed its orientation.
    ///
    /// The orientation describes the clockwise angle of a touch point's major axis to the positive surface
    /// y-axis and is normalized to the -180 to +180 degree range. The granularity of orientation depends
    /// on the touch device, some devices only support binary rotation values between 0 and 90 degrees.
    fn orientation(&self, seat: &Seat<D>, data: &mut D, event: &OrientationEvent, seq: Serial);
}

impl<D: SeatHandler + 'static> TouchHandle<D> {
    pub(crate) fn new<F>(default_grab: F) -> TouchHandle<D>
    where
        F: Fn() -> Box<dyn TouchGrab<D>> + Send + 'static,
    {
        TouchHandle {
            inner: Arc::new(Mutex::new(TouchInternal::new(default_grab))),
            #[cfg(feature = "wayland_frontend")]
            known_instances: Arc::new(Mutex::new(Vec::new())),
            span: info_span!("input_touch"),
        }
    }

    /// Change the current grab on this touch to the provided grab
    ///
    /// Overwrites any current grab.
    #[instrument(level = "debug", parent = &self.span, skip(self, data, grab))]
    pub fn set_grab<G: TouchGrab<D> + 'static>(&self, data: &mut D, grab: G, serial: Serial) {
        let seat = self.get_seat(data);
        self.inner.lock().unwrap().set_grab(data, &seat, serial, grab);
    }

    /// Remove any current grab on this touch, resetting it to the default behavior
    #[instrument(level = "debug", parent = &self.span, skip(self, data))]
    pub fn unset_grab(&self, data: &mut D) {
        let seat = self.get_seat(data);
        self.inner.lock().unwrap().unset_grab(data, &seat);
    }

    /// Check if this touch is currently grabbed with this serial
    pub fn has_grab(&self, serial: Serial) -> bool {
        let guard = self.inner.lock().unwrap();
        match guard.grab {
            GrabStatus::Active(s, _) => s == serial,
            _ => false,
        }
    }

    /// Check if this touch is currently being grabbed
    pub fn is_grabbed(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        !matches!(guard.grab, GrabStatus::None)
    }

    /// Returns the start data for the grab, if any.
    pub fn grab_start_data(&self) -> Option<GrabStartData<D>> {
        let guard = self.inner.lock().unwrap();
        match &guard.grab {
            GrabStatus::Active(_, g) => Some(g.start_data().clone()),
            _ => None,
        }
    }

    /// Notify that a new touch point appeared
    ///
    /// You provide the location of the touch, in the form of:
    ///
    /// - The coordinates of the touch in the global compositor space
    /// - The surface on top of which the touch point is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the touch is not
    ///   on top of a client surface).
    pub fn down(
        &self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &DownEvent,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let seat = self.get_seat(data);
        let seq = inner.seq_counter.next_serial();
        inner.with_grab(&seat, |handle, grab| {
            grab.down(data, handle, focus, event, seq);
        });
    }

    /// Notify that a touch point disappeared
    pub fn up(&self, data: &mut D, event: &UpEvent) {
        let mut inner = self.inner.lock().unwrap();
        let seat = self.get_seat(data);
        let seq = inner.seq_counter.next_serial();
        inner.with_grab(&seat, |handle, grab| {
            grab.up(data, handle, event, seq);
        });
    }

    /// Notify that a touch point has changed coordinates.
    ///
    /// You provide the location of the touch, in the form of:
    ///
    /// - The coordinates of the touch in the global compositor space
    /// - The surface on top of which the touch point is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the touch is not
    ///   on top of a client surface).
    ///
    /// **Note** that this will **not** update the focus of the touch point, the focus
    /// is only set on [`TouchHandle::down`]. The focus provided to this function
    /// can be used to find DnD targets during touch motion.
    pub fn motion(
        &self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let seat = self.get_seat(data);
        let seq = inner.seq_counter.next_serial();
        inner.with_grab(&seat, |handle, grab| {
            grab.motion(data, handle, focus, event, seq);
        });
    }

    /// Notify about the end of a set of events that logically belong together.
    ///
    /// This needs to be called after one or move calls to [`TouchHandle::down`] or [`TouchHandle::motion`]
    pub fn frame(&self, data: &mut D) {
        let mut inner = self.inner.lock().unwrap();
        let seat = self.get_seat(data);
        let seq = inner.seq_counter.next_serial();
        inner.with_grab(&seat, |handle, grab| {
            grab.frame(data, handle, seq);
        });
    }

    /// Notify that the touch session has been cancelled.
    ///
    /// Use in case you decide the touch stream is a global gesture.
    /// This will remove all current focus targets, and no further events will be sent
    /// until a new touch point appears.
    pub fn cancel(&self, data: &mut D) {
        let mut inner = self.inner.lock().unwrap();
        let seat = self.get_seat(data);
        let seq = inner.seq_counter.next_serial();
        inner.with_grab(&seat, |handle, grab| {
            grab.cancel(data, handle, seq);
        });
    }

    fn get_seat(&self, data: &mut D) -> Seat<D> {
        let seat_state = data.seat_state();
        seat_state
            .seats
            .iter()
            .find(|seat| seat.get_touch().map(|h| &h == self).unwrap_or(false))
            .cloned()
            .unwrap()
    }
}

/// This inner handle is accessed from inside a pointer grab logic, and directly
/// sends event to the client
pub struct TouchInnerHandle<'a, D: SeatHandler> {
    inner: &'a mut TouchInternal<D>,
    seat: &'a Seat<D>,
}

impl<'a, D: SeatHandler> fmt::Debug for TouchInnerHandle<'a, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TouchInnerHandle")
            .field("inner", &self.inner)
            .field("seat", &self.seat.arc.name)
            .finish()
    }
}

impl<'a, D: SeatHandler + 'static> TouchInnerHandle<'a, D> {
    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: TouchGrab<D> + 'static>(&mut self, data: &mut D, serial: Serial, grab: G) {
        self.inner.set_grab(data, self.seat, serial, grab);
    }

    /// Remove any current grab on this pointer, resetting it to the default behavior
    ///
    /// This will also restore the focus of the underlying pointer if restore_focus
    /// is [`true`]
    pub fn unset_grab(&mut self, data: &mut D) {
        self.inner.unset_grab(data, self.seat);
    }

    /// Notify that a new touch point appeared
    ///
    /// You provide the location of the touch, in the form of:
    ///
    /// - The coordinates of the touch in the global compositor space
    /// - The surface on top of which the touch point is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the touch is not
    ///   on top of a client surface).
    pub fn down(
        &mut self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &DownEvent,
        seq: Serial,
    ) {
        self.inner.down(data, self.seat, focus, event, seq)
    }

    /// Notify that a touch point disappeared
    pub fn up(&mut self, data: &mut D, event: &UpEvent, seq: Serial) {
        self.inner.up(data, self.seat, event, seq)
    }

    /// Notify that a touch point has changed coordinates.
    ///
    /// You provide the location of the touch, in the form of:
    ///
    /// - The coordinates of the touch in the global compositor space
    /// - The surface on top of which the touch point is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the touch is not
    ///   on top of a client surface).
    ///
    /// **Note** that this will **not** update the focus of the touch point, the focus
    /// is only set on [`TouchHandle::down`]. The focus provided to this function
    /// can be used to find DnD targets during touch motion.
    pub fn motion(
        &mut self,
        data: &mut D,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &MotionEvent,
        seq: Serial,
    ) {
        self.inner.motion(data, self.seat, focus, event, seq)
    }

    /// Notify about the end of a set of events that logically belong together.
    ///
    /// This needs to be called after one or move calls to [`TouchHandle::down`] or [`TouchHandle::motion`]
    pub fn frame(&mut self, data: &mut D, seq: Serial) {
        self.inner.frame(data, self.seat, seq)
    }

    /// Notify that the touch session has been cancelled.
    ///
    /// Use in case you decide the touch stream is a global gesture.
    /// This will remove all current focus targets, and no further events will be sent
    /// until a new touch point appears.
    pub fn cancel(&mut self, data: &mut D, seq: Serial) {
        self.inner.cancel(data, self.seat, seq)
    }
}

impl<D: SeatHandler + 'static> TouchInternal<D> {
    fn new<F>(default_grab: F) -> Self
    where
        F: Fn() -> Box<dyn TouchGrab<D>> + Send + 'static,
    {
        Self {
            focus: Default::default(),
            seq_counter: SerialCounter::new(),
            default_grab: Box::new(default_grab),
            grab: GrabStatus::None,
        }
    }

    fn set_grab<G: TouchGrab<D> + 'static>(
        &mut self,
        _data: &mut D,
        _seat: &Seat<D>,
        serial: Serial,
        grab: G,
    ) {
        self.grab = GrabStatus::Active(serial, Box::new(grab));
    }

    fn unset_grab(&mut self, _data: &mut D, _seat: &Seat<D>) {
        self.grab = GrabStatus::None;
    }

    fn down(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &DownEvent,
        seq: Serial,
    ) {
        self.focus
            .entry(event.slot)
            .and_modify(|state| {
                state.pending = seq;
                state.focus = focus.clone();
            })
            .or_insert_with(|| TouchSlotState {
                focus,
                pending: seq,
                current: None,
            });
        let state = self.focus.get(&event.slot).unwrap();
        if let Some((focus, loc)) = state.focus.as_ref() {
            let mut new_event = event.clone();
            new_event.location -= loc.to_f64();
            focus.down(seat, data, &new_event, seq);
        }
    }

    fn up(&mut self, data: &mut D, seat: &Seat<D>, event: &UpEvent, seq: Serial) {
        let Some(state) = self.focus.get_mut(&event.slot) else {
            return;
        };
        state.pending = seq;
        if let Some((focus, _)) = state.focus.take() {
            focus.up(seat, data, event, seq);
        }
    }

    fn motion(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        _focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &MotionEvent,
        seq: Serial,
    ) {
        let Some(state) = self.focus.get_mut(&event.slot) else {
            return;
        };
        state.pending = seq;
        if let Some((focus, loc)) = state.focus.as_ref() {
            let mut new_event = event.clone();
            new_event.location -= loc.to_f64();
            focus.motion(seat, data, &new_event, seq);
        }
    }

    fn frame(&mut self, data: &mut D, seat: &Seat<D>, seq: Serial) {
        for state in self.focus.values_mut() {
            if state.current.map(|c| c >= state.pending).unwrap_or(false) {
                continue;
            }
            state.current = Some(seq);
            if let Some((focus, _)) = state.focus.as_ref() {
                focus.frame(seat, data, seq);
            }
        }
    }

    fn cancel(&mut self, data: &mut D, seat: &Seat<D>, seq: Serial) {
        for state in self.focus.values_mut() {
            if state.current.map(|c| c >= state.pending).unwrap_or(false) {
                continue;
            }
            state.current = Some(seq);
            if let Some((focus, _)) = state.focus.take() {
                focus.cancel(seat, data, seq);
            }
        }
    }

    fn with_grab<F>(&mut self, seat: &Seat<D>, f: F)
    where
        F: FnOnce(&mut TouchInnerHandle<'_, D>, &mut dyn TouchGrab<D>),
    {
        let mut grab = std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => panic!("Accessed a touch grab from within a touch grab access."),
            GrabStatus::Active(_, ref mut handler) => {
                // If this grab is associated with a surface that is no longer alive, discard it
                if let Some((ref focus, _)) = handler.start_data().focus {
                    if !focus.alive() {
                        self.grab = GrabStatus::None;
                        let mut default_grab = (self.default_grab)();
                        f(&mut TouchInnerHandle { inner: self, seat }, &mut *default_grab);
                        return;
                    }
                }
                f(&mut TouchInnerHandle { inner: self, seat }, &mut **handler);
            }
            GrabStatus::None => {
                let mut default_grab = (self.default_grab)();
                f(&mut TouchInnerHandle { inner: self, seat }, &mut *default_grab);
            }
        }

        if let GrabStatus::Borrowed = self.grab {
            // the grab has not been ended nor replaced, put it back in place
            self.grab = grab;
        }
    }
}
