use std::fmt;

use crate::{
    backend::input::TouchSlot,
    input::SeatHandler,
    utils::{Logical, Point, Serial},
};

use super::{DownEvent, MotionEvent, OrientationEvent, ShapeEvent, TouchInnerHandle, UpEvent};

/// A trait to implement a touch grab
///
/// In some context, it is necessary to temporarily change the behavior of the touch handler. This is
/// typically known as a touch grab. A typical example would be, during a drag'n'drop operation,
/// the underlying surfaces will no longer receive classic pointer event, but rather special events.
///
/// This trait is the interface to intercept regular touch events and change them as needed, its
/// interface mimics the [`TouchHandle`](super::TouchHandle) interface.
///
/// Any interactions with [`TouchHandle`](super::TouchHandle)
/// should be done using [`TouchInnerHandle`], as handle is borrowed/locked before grab methods are called,
/// so calling methods on [`TouchHandle`](super::TouchHandle) would result in a deadlock.
///
/// If your logic decides that the grab should end, both [`TouchInnerHandle`]
/// and [`TouchHandle`](super::TouchHandle) have
/// a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the server),
/// the struct implementing this trait will be dropped. As such you should put clean-up logic in the destructor,
/// rather than trying to guess when the grab will end.
pub trait TouchGrab<D: SeatHandler>: Send {
    /// A new touch point appeared
    ///
    /// This method allows you attach additional behavior to a down event, possibly altering it.
    /// You generally will want to invoke [`TouchInnerHandle::down`] as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the down event never occurred.
    ///
    /// Some grabs (such as drag'n'drop, shell resize and motion) drop down events while they are active,
    /// the default touch grab will keep the focus on the surface that started the grab.
    fn down(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &DownEvent,
        seq: Serial,
    );
    /// A touch point disappeared
    ///
    /// This method allows you attach additional behavior to a up event, possibly altering it.
    /// You generally will want to invoke [`TouchInnerHandle::up`] as part of your processing.
    /// If you don't, the rest of the compositor will behave as if the up event never occurred.
    ///
    /// Some grabs (such as drag'n'drop, shell resize and motion) drop up events while they are active,
    /// but will end when the touch point that initiated the grab disappeared.
    fn up(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, event: &UpEvent, seq: Serial);

    /// A touch point has changed coordinates.
    ///
    /// This method allows you attach additional behavior to a motion event, possibly altering it.
    /// You generally will want to invoke [`TouchInnerHandle::motion`] as part of your processing.
    /// If you don't, the rest of the compositor will behave as if the motion event never occurred.
    ///
    /// **Note** that this is **not** intended to update the focus of the touch point, the focus
    /// is only set on a down event. The focus provided to this function can be used to find DnD
    /// targets during touch motion.
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &MotionEvent,
        seq: Serial,
    );

    /// Marks the end of a set of events that logically belong together.
    ///
    /// This method allows you attach additional behavior to a frame event, possibly altering it.
    /// You generally will want to invoke [`TouchInnerHandle::frame`] as part of your processing.
    /// If you don't, the rest of the compositor will behave as if the frame event never occurred.
    ///
    /// This will to be called after one or more calls to down/motion events.
    fn frame(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, seq: Serial);

    /// A touch session has been cancelled.
    ///
    /// This method allows you attach additional behavior to a cancel event, possibly altering it.
    /// You generally will want to invoke [`TouchInnerHandle::cancel`] as part of your processing.
    /// If you don't, the rest of the compositor will behave as if the cancel event never occurred.
    ///
    /// Usually called in case the compositor decides the touch stream is a global gesture.
    fn cancel(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, seq: Serial);

    /// A touch point has changed its shape.
    fn shape(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, event: &ShapeEvent, seq: Serial);

    /// A touch point has changed its orientation.
    fn orientation(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        event: &OrientationEvent,
        seq: Serial,
    );

    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData<D>;

    /// The grab has been unset or replaced with another grab.
    fn unset(&mut self, data: &mut D);
}

/// Data about the event that started the grab.
pub struct GrabStartData<D: SeatHandler> {
    /// The focused surface and its location, if any, at the start of the grab.
    ///
    /// The location coordinates are in the global compositor space.
    pub focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
    /// The touch point that initiated the grab.
    pub slot: TouchSlot,
    /// The location of the down event that initiated the grab, in the global compositor space.
    pub location: Point<f64, Logical>,
}

impl<D: SeatHandler + 'static> fmt::Debug for GrabStartData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrabStartData")
            .field("focus", &self.focus.as_ref().map(|_| "..."))
            .field("slot", &self.slot)
            .field("location", &self.location)
            .finish()
    }
}

impl<D: SeatHandler + 'static> Clone for GrabStartData<D> {
    fn clone(&self) -> Self {
        GrabStartData {
            focus: self.focus.clone(),
            slot: self.slot,
            location: self.location,
        }
    }
}

/// The default grab, the behavior when no particular grab is in progress
#[derive(Debug)]
pub struct DefaultGrab;

impl<D: SeatHandler + 'static> TouchGrab<D> for DefaultGrab {
    fn down(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &DownEvent,
        seq: Serial,
    ) {
        handle.down(data, focus.clone(), event, seq);
        handle.set_grab(
            self,
            data,
            event.serial,
            TouchDownGrab {
                start_data: GrabStartData {
                    focus,
                    slot: event.slot,
                    location: event.location,
                },
                touch_points: 1,
            },
        );
    }

    fn up(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, event: &UpEvent, seq: Serial) {
        handle.up(data, event, seq)
    }

    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &MotionEvent,
        seq: Serial,
    ) {
        handle.motion(data, focus, event, seq)
    }

    fn frame(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, seq: Serial) {
        handle.frame(data, seq)
    }

    fn cancel(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, seq: Serial) {
        handle.cancel(data, seq)
    }

    fn shape(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, event: &ShapeEvent, seq: Serial) {
        handle.shape(data, event, seq)
    }

    fn orientation(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        event: &OrientationEvent,
        seq: Serial,
    ) {
        handle.orientation(data, event, seq)
    }

    fn start_data(&self) -> &GrabStartData<D> {
        unreachable!()
    }

    fn unset(&mut self, _data: &mut D) {}
}

/// A touch down grab, basic grab started when an user touches a surface
/// to maintain it focused until the user releases the touch.
///
/// In case the user maintains several simultaneous touch points, release
/// the grab once all are released.
pub struct TouchDownGrab<D: SeatHandler> {
    /// Start date for this grab
    pub start_data: GrabStartData<D>,
    /// Currently active touch points
    pub touch_points: usize,
}

impl<D: SeatHandler + 'static> fmt::Debug for TouchDownGrab<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TouchDownGrab")
            .field("start_data", &self.start_data)
            .field("touch_points", &self.touch_points)
            .finish()
    }
}

impl<D: SeatHandler + 'static> TouchGrab<D> for TouchDownGrab<D> {
    fn down(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        _focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &DownEvent,
        seq: Serial,
    ) {
        handle.down(data, self.start_data.focus.clone(), event, seq);
        self.touch_points += 1;
    }

    fn up(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, event: &UpEvent, seq: Serial) {
        handle.up(data, event, seq);
        self.touch_points = self.touch_points.saturating_sub(1);
        if self.touch_points == 0 {
            handle.unset_grab(self, data);
        }
    }

    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        _focus: Option<(<D as SeatHandler>::TouchFocus, Point<i32, Logical>)>,
        event: &MotionEvent,
        seq: Serial,
    ) {
        handle.motion(data, self.start_data.focus.clone(), event, seq)
    }

    fn frame(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, seq: Serial) {
        handle.frame(data, seq)
    }

    fn cancel(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, seq: Serial) {
        handle.cancel(data, seq);
        handle.unset_grab(self, data);
    }

    fn shape(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, event: &ShapeEvent, seq: Serial) {
        handle.shape(data, event, seq)
    }

    fn orientation(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        event: &OrientationEvent,
        seq: Serial,
    ) {
        handle.orientation(data, event, seq)
    }

    fn start_data(&self) -> &GrabStartData<D> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut D) {}
}
