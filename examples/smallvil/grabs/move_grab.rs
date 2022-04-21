use smithay::{
    desktop::Window,
    utils::{Logical, Point},
    wayland::seat::{
        AxisFrame, ButtonEvent, MotionEvent, PointerGrab, PointerGrabStartData, PointerInnerHandle,
    },
};
use wayland_server::DisplayHandle;

use crate::Smallvil;

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<Smallvil> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Smallvil,
        _dh: &mut DisplayHandle<'_>,
        _handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &MotionEvent,
    ) {
        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        data.space
            .map_window(&self.window, new_location.to_i32_round(), true);
    }

    fn button(
        &mut self,
        _data: &mut Smallvil,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &ButtonEvent,
    ) {
        handle.button(dh, event.button, event.state, event.serial, event.time);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(dh, event.serial, event.time);
        }
    }

    fn axis(
        &mut self,
        _data: &mut Smallvil,
        cx: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        details: AxisFrame,
    ) {
        handle.axis(cx, details)
    }

    fn start_data(&self) -> &PointerGrabStartData {
        &self.start_data
    }
}
