use std::fmt;

use wayland_server::{
    protocol::{wl_pointer::ButtonState, wl_surface::WlSurface},
    DisplayHandle,
};

use crate::{
    utils::{Logical, Point},
    wayland::{seat2::SeatHandler, Serial},
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
pub trait PointerGrab<D>: Send + Sync {
    /// A motion was reported
    fn motion(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    );
    /// A button press was reported
    fn button(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    );
    /// An axis scroll was reported
    fn axis(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
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

impl<D> PointerGrab<D> for DefaultGrab {
    fn motion(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        handle.motion(cx, seat_handler, location, focus, serial, time);
    }

    fn button(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        handle.button(cx, button, state, serial, time);
        handle.set_grab(
            serial,
            ClickGrab {
                start_data: GrabStartData {
                    focus: handle.current_focus().cloned(),
                    button,
                    location: handle.current_location(),
                },
            },
        );
    }

    fn axis(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        details: AxisFrame,
    ) {
        handle.axis(cx, details);
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

impl<D> PointerGrab<D> for ClickGrab {
    fn motion(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        location: Point<f64, Logical>,
        _focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        handle.motion(
            cx,
            seat_handler,
            location,
            self.start_data.focus.clone(),
            serial,
            time,
        );
    }

    fn button(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        handle.button(cx, button, state, serial, time);
        if handle.current_pressed().is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab(cx, seat_handler, serial, time);
        }
    }

    fn axis(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        seat_handler: &mut dyn SeatHandler<D>,
        handle: &mut PointerInnerHandle<'_, D>,
        details: AxisFrame,
    ) {
        handle.axis(cx, details);
    }

    fn start_data(&self) -> &GrabStartData {
        &self.start_data
    }
}
