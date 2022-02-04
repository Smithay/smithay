use std::fmt;

use wayland_server::{
    protocol::{wl_pointer::ButtonState, wl_surface::WlSurface},
    DisplayHandle,
};

use crate::{
    utils::{Logical, Point},
    wayland::Serial,
};

use super::{AxisFrame, PointerInnerHandle};

/// A trait to implement a pointer grab
///
/// In some context, it is necessary to temporarily change the behavior of the pointer. This is
/// typically known as a pointer grab. A typical example would be, during a drag'n'drop operation,
/// the underlying surfaces will no longer receive classic pointer event, but rather special events.
///
/// This trait is the interface to intercept regular pointer events and change them as needed, its
/// interface mimics the [`PointerHandle`] interface.
///
/// If your logic decides that the grab should end, both [`PointerInnerHandle`] and [`PointerHandle`] have
/// a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the server),
/// the struct implementing this trait will be dropped. As such you should put clean-up logic in the destructor,
/// rather than trying to guess when the grab will end.
pub trait PointerGrab: Send + Sync {
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
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    );
    /// A button press was reported
    ///
    /// This method allows you attach additional behavior to a button event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::button()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the button event never occurred.
    fn button(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    );
    /// An axis scroll was reported
    ///
    /// This method allows you attach additional behavior to an axis event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::axis()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the axis event never occurred.
    fn axis(&mut self, dh: &mut DisplayHandle<'_>, handle: &mut PointerInnerHandle<'_>, details: AxisFrame);
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

pub(super) enum GrabStatus {
    None,
    Active(Serial, Box<dyn PointerGrab>),
    Borrowed,
}

// PointerGrab is a trait, so we have to impl Debug manually
impl fmt::Debug for GrabStatus {
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

impl PointerGrab for DefaultGrab {
    fn motion(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        handle.motion(dh, location, focus, serial, time);
    }

    fn button(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        handle.button(dh, button, state, serial, time);
        handle.set_grab(
            dh,
            serial,
            time,
            ClickGrab {
                start_data: GrabStartData {
                    focus: handle.current_focus().cloned(),
                    button,
                    location: handle.current_location(),
                },
            },
        );
    }

    fn axis(&mut self, dh: &mut DisplayHandle<'_>, handle: &mut PointerInnerHandle<'_>, details: AxisFrame) {
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

impl PointerGrab for ClickGrab {
    fn motion(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        _focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        handle.motion(dh, location, self.start_data.focus.clone(), serial, time);
    }

    fn button(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        handle.button(dh, button, state, serial, time);
        if handle.current_pressed().is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab(dh, serial, time);
        }
    }

    fn axis(&mut self, dh: &mut DisplayHandle<'_>, handle: &mut PointerInnerHandle<'_>, details: AxisFrame) {
        handle.axis(dh, details);
    }

    fn start_data(&self) -> &GrabStartData {
        &self.start_data
    }
}
