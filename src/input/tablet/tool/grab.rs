use std::fmt;

use downcast_rs::{Downcast, impl_downcast};

use crate::{
    input::{
        pointer::Focus,
        tablet::{
            TabletSeatHandler,
            tool::{
                AxisFrame, ButtonEvent, DownEvent, MotionEvent, ProximityInEvent, ProximityOutEvent,
                TabletToolInnerHandle, UpEvent,
            },
        },
    },
    utils::{Logical, Point},
};

/// A trait to implemenent a tablet tool grab
///
/// In some context, it is necessary to temporarily change the behavior of the tablet tool. This is
/// typically known as a grab. A typical example would be, during a drag'n'drop operation, the
/// underlying surfaces will no longer receive classic tool event, but rather special events.
///
/// This trait is the interface to intercept regular tool events and change them as needed. Its
/// interface mimics the [`TabletToolHandle`] interface.
///
/// Any interaction with [`TabletToolHandle`] should be done using [`TabletToolInnerHandle`] as
/// handle is borrowed;pcled before grab methods are called, so calling methods on
/// [`TabletToolHandle`] would result in a deadlock.
///
/// Ig your logic decides that the grab should end, both [`TabletToolInnerHandle`] and
/// [`TabletToolHandle`] have a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the
/// server), the struct implemented this trait will be dropped. As such, you should put clean-up
/// logic in the destructor, rather than trying to guess when the grab will end.
///
/// [`TabletToolHandle`]: super::TabletToolHandle
pub trait TabletToolGrab<D: TabletSeatHandler + 'static>: Send + Downcast {
    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData<D>;

    /// A physical proximity_in was reported.
    ///
    /// This method allows you to attach additional behavior to a proximity in event, possibly
    /// altering it. You generally will want to invoke `TabletToolInnerHandle::proximity_in()` as part of
    /// your processing. If you don't the rest of the compositor will behave as if the event never
    /// occurred.
    ///
    /// Some grabs (such as drag'n'drop, shell resize and motion) unset the focus while they are
    /// active, this is achieved by just setting the focus to None when invoking
    /// `TabletToolInnerHandle::proximity_in()`.
    ///
    /// This event is only relevant for default grabs.
    fn proximity_in(
        &mut self,
        data: &mut D,
        handle: &mut TabletToolInnerHandle<'_, D>,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &ProximityInEvent,
    ) {
        handle.proximity_in(data, focus, event);
    }

    /// A physical proximity_out was reported.
    ///
    /// This method allows you to attach additional behavior to a proximity out event. You generally
    /// want to invoke `TabletToolInnerHandle::proximity_out()` as part of your processing.
    fn proximity_out(
        &mut self,
        data: &mut D,
        handle: &mut TabletToolInnerHandle<'_, D>,
        event: &ProximityOutEvent,
    ) {
        handle.proximity_out(data, event);
    }

    /// A motion was reported
    ///
    /// This method allows you to attach additional behavior to a motion event, possibly altering
    /// it. You generally want to invoke `TabletToolInnerHandle::motion()` as part of your
    /// processing. If you don't, the rest of the compositor will behave as if the event never
    /// occurred.
    ///
    /// Some grabs (such as drag'n'drop, shell resize and motion) unset the focus while they are
    /// active, this is achieved by just setting the focus to `None` when invoking
    /// `TabletToolInnerHandle::motion()`.
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut TabletToolInnerHandle<'_, D>,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    );

    /// A tip down was reported.
    ///
    /// This method allows you to attach additional behavior to a down event, possibly altering it.
    /// You generally will want to invoke `TabletToolInnerHandle::down()` as part of your processing.
    /// If you don't, the rest of the compositor will behave as if the down event never occurred.
    fn down(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &DownEvent);

    /// A tip up was reported.
    ///
    /// This method allows you to attach additional behavior to an up event, possibly altering it.
    /// You generally will want to invoke `TabletToolInnerHandle::up()` as part of your processing.
    /// If you don't, the rest of the compositor will behave as if the down event never occurred.
    fn up(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &UpEvent);

    /// A button press was reported
    ///
    /// This method allows you to attach additional behavior to an up event, possibly altering it.
    /// You generally will want to invoke `TabletToolInnerHandle::button()` as part of your
    /// processing. If you don't, the rest of the compositor will behave as if the down event never
    /// occurred.
    fn button(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &ButtonEvent);

    /// An axis frame was reported
    ///
    /// This method allows you to attach additional behavior to an up event, possibly altering it.
    /// You generally will want to invoke `TabletToolInnerHandle::axis()` as part of your
    /// processing. If you don't, the rest of the compositor will behave as if the down event never
    /// occurred.
    fn axis(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, frame: AxisFrame);

    /// End of a tablet tool frame
    ///
    /// A frame groups associated events. This terminate the frame.
    fn frame(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, time: u32);

    /// The grab has been unset or replaced with another grab.
    fn unset(&mut self, data: &mut D);
}

impl_downcast!(TabletToolGrab<D> where D: TabletSeatHandler);

/// Event that caused the grab to start.
#[derive(Debug, Clone, Copy)]
pub enum GrabTrigger {
    /// Grab was initiated following a ProximityIn event.
    Proximity,
    /// Grab was initiated following a TipDown event.
    Tip,
    /// Grab was initiated following a Button event.
    Button(u32),
}

/// Data about the event that started the grab.
pub struct GrabStartData<D: TabletSeatHandler> {
    /// The focused surface and its location, if any, at the start of the grab.
    ///
    /// The location coordinates are in the global compositor space.
    pub focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
    /// The event that triggered the grab.
    pub trigger: GrabTrigger,
    /// The location of the tool when the grab was initiated, in the global compositor space.
    pub location: Point<f64, Logical>,
}

impl<D: TabletSeatHandler> fmt::Debug for GrabStartData<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrabStartData")
            .field("focus", &self.focus.as_ref().map(|_| "..."))
            .field("trigger", &self.trigger)
            .field("location", &self.location)
            .finish()
    }
}

impl<D: TabletSeatHandler> Clone for GrabStartData<D> {
    fn clone(&self) -> Self {
        GrabStartData {
            focus: self.focus.clone(),
            trigger: self.trigger,
            location: self.location,
        }
    }
}

/// Tablet Tool default grab.
#[derive(Debug)]
pub struct DefaultGrab;

impl<D: TabletSeatHandler + 'static> TabletToolGrab<D> for DefaultGrab {
    fn start_data(&self) -> &GrabStartData<D> {
        unreachable!();
    }

    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut TabletToolInnerHandle<'_, D>,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, focus, event);
    }

    fn down(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &DownEvent) {
        handle.down(data, event);

        handle.set_grab(
            self,
            data,
            event.time,
            event.serial,
            Focus::Keep,
            DownGrab {
                start_data: GrabStartData {
                    focus: handle.current_focus(),
                    trigger: GrabTrigger::Tip,
                    location: handle.current_location(),
                },
            },
        );
    }

    fn up(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &UpEvent) {
        handle.up(data, event);
    }

    fn button(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &ButtonEvent) {
        handle.button(data, event);
    }

    fn axis(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, frame: AxisFrame) {
        handle.axis(data, frame);
    }

    fn frame(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, time: u32) {
        handle.frame(data, time);
    }

    fn unset(&mut self, _data: &mut D) {}
}

/// A down grab, basic grab started when a user touch the tablet with a tool above a surface, to
/// maintain it focused until the user lift the tool.
pub struct DownGrab<D: TabletSeatHandler> {
    start_data: GrabStartData<D>,
}

impl<D: TabletSeatHandler + 'static> fmt::Debug for DownGrab<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownGrab")
            .field("start_data", &self.start_data)
            .finish()
    }
}

impl<D: TabletSeatHandler + 'static> TabletToolGrab<D> for DownGrab<D> {
    fn start_data(&self) -> &GrabStartData<D> {
        &self.start_data
    }

    fn proximity_in(
        &mut self,
        data: &mut D,
        handle: &mut TabletToolInnerHandle<'_, D>,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &ProximityInEvent,
    ) {
        handle.proximity_in(data, focus, event);
    }

    fn proximity_out(
        &mut self,
        data: &mut D,
        handle: &mut TabletToolInnerHandle<'_, D>,
        event: &ProximityOutEvent,
    ) {
        handle.proximity_out(data, event);
    }

    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut TabletToolInnerHandle<'_, D>,
        _focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, self.start_data.focus.clone(), event);
    }

    fn down(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &DownEvent) {
        handle.down(data, event);
    }

    fn up(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &UpEvent) {
        handle.up(data, event);
        handle.unset_grab(self, data, event.serial, event.time, true);
    }

    fn button(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, event: &ButtonEvent) {
        handle.button(data, event);
    }

    fn axis(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, frame: AxisFrame) {
        handle.axis(data, frame);
    }

    fn frame(&mut self, data: &mut D, handle: &mut TabletToolInnerHandle<'_, D>, time: u32) {
        handle.frame(data, time);
    }

    fn unset(&mut self, _data: &mut D) {}
}
