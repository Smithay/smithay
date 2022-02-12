use std::fmt;

use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};

use crate::{
    utils::{Logical, Point},
    wayland::Serial,
};

use super::{
    events::{ButtonEvent, MotionEvent},
    AxisFrame, PointerInnerHandle,
};

/// A trait to implement a pointer grab
///
/// In some context, it is necessary to temporarily change the behavior of the pointer. This is
/// typically known as a pointer grab. A typical example would be, during a drag'n'drop operation,
/// the underlying surfaces will no longer receive classic pointer event, but rather special events.
///
/// This trait is the interface to intercept regular pointer events and change them as needed, its
/// interface mimics the [`PointerHandle`] interface.
///
/// Any interactions with [`PointerHandle`] should be done using [`PointerInnerHandle`],
/// as handle is borrowed/locked before grab methods are called,
/// so calling methods on [`PointerHandle`] would result in a deadlock.
///
/// If your logic decides that the grab should end, both [`PointerInnerHandle`] and [`PointerHandle`] have
/// a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the server),
/// the struct implementing this trait will be dropped. As such you should put clean-up logic in the destructor,
/// rather than trying to guess when the grab will end.
pub trait PointerGrab<T>: Send + Sync {
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
        data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        event: &MotionEvent,
    );
    /// A button press was reported
    ///
    /// This method allows you attach additional behavior to a button event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::button()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the button event never occurred.
    fn button(
        &mut self,
        data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        event: &ButtonEvent,
    );
    /// An axis scroll was reported
    ///
    /// This method allows you attach additional behavior to an axis event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::axis()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the axis event never occurred.
    fn axis(
        &mut self,
        data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        details: AxisFrame,
    );
    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData;
}

/// Data about the event that started the grab.
#[derive(Debug, Clone)]
pub struct GrabStartData {
    /// The focused surface and its location, if any, at the start of the grab.
    ///
    /// The location coordinates are in the global compositor space.
    pub focus: Option<(WlSurface, Point<i32, Logical>)>,
    /// The button that initiated the grab.
    pub button: u32,
    /// The location of the click that initiated the grab, in the global compositor space.
    pub location: Point<f64, Logical>,
}

pub(super) enum GrabStatus<T> {
    None,
    Active(Serial, Box<dyn PointerGrab<T>>),
    Borrowed,
}

// PointerGrab is a trait, so we have to impl Debug manually
impl<T> fmt::Debug for GrabStatus<T> {
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

impl<T> PointerGrab<T> for DefaultGrab {
    fn motion(
        &mut self,
        _data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        event: &MotionEvent,
    ) {
        handle.motion(dh, event.location, event.focus.clone(), event.serial, event.time);
    }

    fn button(
        &mut self,
        _data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        event: &ButtonEvent,
    ) {
        handle.button(dh, event.button, event.state, event.serial, event.time);
        handle.set_grab(
            dh,
            event.serial,
            event.time,
            ClickGrab {
                start_data: GrabStartData {
                    focus: handle.current_focus().cloned(),
                    button: event.button,
                    location: handle.current_location(),
                },
            },
        );
    }

    fn axis(
        &mut self,
        _data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        details: AxisFrame,
    ) {
        handle.axis(dh, details);
    }

    fn start_data(&self) -> &GrabStartData {
        unreachable!()
    }
}

// A click grab, basic grab started when an user clicks a surface
// to maintain it focused until the user releases the click.
//
// In case the user maintains several simultaneous clicks, release
// the grab once all are released.
struct ClickGrab {
    start_data: GrabStartData,
}

impl<T> PointerGrab<T> for ClickGrab {
    fn motion(
        &mut self,
        _data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        event: &MotionEvent,
    ) {
        handle.motion(
            dh,
            event.location,
            self.start_data.focus.clone(),
            event.serial,
            event.time,
        );
    }

    fn button(
        &mut self,
        _data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        event: &ButtonEvent,
    ) {
        handle.button(dh, event.button, event.state, event.serial, event.time);
        if handle.current_pressed().is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab(dh, event.serial, event.time);
        }
    }

    fn axis(
        &mut self,
        _data: &mut T,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, T>,
        details: AxisFrame,
    ) {
        handle.axis(dh, details);
    }

    fn start_data(&self) -> &GrabStartData {
        &self.start_data
    }
}
