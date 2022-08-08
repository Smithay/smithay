use std::fmt;

use crate::{
    backend::input::ButtonState,
    input::SeatHandler,
    utils::{Logical, Point},
    wayland::Serial,
};

use super::{AxisFrame, ButtonEvent, Focus, MotionEvent, PointerHandler, PointerInnerHandle};

/// A trait to implement a pointer grab
///
/// In some context, it is necessary to temporarily change the behavior of the pointer. This is
/// typically known as a pointer grab. A typical example would be, during a drag'n'drop operation,
/// the underlying surfaces will no longer receive classic pointer event, but rather special events.
///
/// This trait is the interface to intercept regular pointer events and change them as needed, its
/// interface mimics the [`PointerHandle`](super::PointerHandle) interface.
///
/// Any interactions with [`PointerHandle`](super::PointerHandle)
/// should be done using [`PointerInnerHandle`], as handle is borrowed/locked before grab methods are called,
/// so calling methods on [`PointerHandle`](super::PointerHandle) would result in a deadlock.
///
/// If your logic decides that the grab should end, both [`PointerInnerHandle`]
/// and [`PointerHandle`](super::PointerHandle) have
/// a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the server),
/// the struct implementing this trait will be dropped. As such you should put clean-up logic in the destructor,
/// rather than trying to guess when the grab will end.
pub trait PointerGrab<D: SeatHandler>: Send {
    /// A motion was reported
    ///
    /// This method allows you attach additional behavior to a motion event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::motion()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the motion event never occurred.
    ///
    /// Some grabs (such as drag'n'drop, shell resize and motion) unset the focus while they are active,
    /// this is achieved by just setting the focus to `None` when invoking `PointerInnerHandle::motion()`.
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(Box<dyn PointerHandler<D>>, Point<i32, Logical>)>,
        event: &MotionEvent,
    );
    /// A button press was reported
    ///
    /// This method allows you attach additional behavior to a button event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::button()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the button event never occurred.
    fn button(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, event: &ButtonEvent);
    /// An axis scroll was reported
    ///
    /// This method allows you attach additional behavior to an axis event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::axis()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the axis event never occurred.
    fn axis(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, details: AxisFrame);
    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData<D>;
}

/// Data about the event that started the grab.
pub struct GrabStartData<D> {
    /// The focused surface and its location, if any, at the start of the grab.
    ///
    /// The location coordinates are in the global compositor space.
    pub focus: Option<(Box<dyn PointerHandler<D>>, Point<i32, Logical>)>,
    /// The button that initiated the grab.
    pub button: u32,
    /// The location of the click that initiated the grab, in the global compositor space.
    pub location: Point<f64, Logical>,
}

impl<D: SeatHandler + 'static> fmt::Debug for GrabStartData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrabStartData")
            .field("focus", &self.focus.as_ref().map(|_| "..."))
            .field("button", &self.button)
            .field("location", &self.location)
            .finish()
    }
}

impl<D: SeatHandler + 'static> Clone for GrabStartData<D> {
    fn clone(&self) -> Self {
        GrabStartData {
            focus: self
                .focus
                .as_ref()
                .map(|(handler, loc)| (handler.clone_handler(), loc.clone())),
            button: self.button,
            location: self.location.clone(),
        }
    }
}

pub(super) enum GrabStatus<D> {
    None,
    Active(Serial, Box<dyn PointerGrab<D>>),
    Borrowed,
}

// PointerGrab is a trait, so we have to impl Debug manually
impl<D> fmt::Debug for GrabStatus<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrabStatus::None => f.debug_tuple("GrabStatus::None").finish(),
            GrabStatus::Active(serial, _) => f.debug_tuple("GrabStatus::Active").field(&serial).finish(),
            GrabStatus::Borrowed => f.debug_tuple("GrabStatus::Borrowed").finish(),
        }
    }
}

// The default grab, the behavior when no particular grab is in progress
pub(super) struct DefaultGrab;

impl<D: SeatHandler + 'static> PointerGrab<D> for DefaultGrab {
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(Box<dyn PointerHandler<D>>, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, focus, event);
    }

    fn button(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, event: &ButtonEvent) {
        handle.button(data, event);
        if event.state == ButtonState::Pressed {
            handle.set_grab(
                data,
                event.serial,
                Focus::Keep,
                ClickGrab {
                    start_data: GrabStartData {
                        focus: handle.current_focus().map(|(h, p)| (h.clone_handler(), p)),
                        button: event.button,
                        location: handle.current_location(),
                    },
                },
            );
        }
    }

    fn axis(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn start_data(&self) -> &GrabStartData<D> {
        unreachable!()
    }
}

// A click grab, basic grab started when an user clicks a surface
// to maintain it focused until the user releases the click.
//
// In case the user maintains several simultaneous clicks, release
// the grab once all are released.
struct ClickGrab<D> {
    start_data: GrabStartData<D>,
}

impl<D: SeatHandler + 'static> PointerGrab<D> for ClickGrab<D> {
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        _focus: Option<(Box<dyn PointerHandler<D>>, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(
            data,
            self.start_data
                .focus
                .as_ref()
                .map(|(h, p)| (h.clone_handler(), *p)),
            event,
        );
    }

    fn button(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab(data, event.serial, event.time);
        }
    }

    fn axis(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn start_data(&self) -> &GrabStartData<D> {
        &self.start_data
    }
}
