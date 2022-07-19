use crate::Smallvil;
use smithay::{
    desktop::Window,
    reexports::wayland_server::DisplayHandle,
    utils::{Logical, Point},
    wayland::seat::{
        AxisFrame, ButtonEvent, MotionEvent, PointerGrab, PointerGrabStartData, PointerInnerHandle,
    },
};

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<Smallvil> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Smallvil,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(event.location, None, event.serial, event.time);

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        data.space
            .map_window(&self.window, new_location.to_i32_round(), None, true);
    }

    fn button(
        &mut self,
        _data: &mut Smallvil,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &ButtonEvent,
    ) {
        handle.button(event.button, event.state, event.serial, event.time);

        // The button is a button code as defined in the
        // Linux kernel's linux/input-event-codes.h header file, e.g. BTN_LEFT.
        const BTN_LEFT: u32 = 0x110;

        if !handle.current_pressed().contains(&BTN_LEFT) {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(event.serial, event.time);
        }
    }

    fn axis(
        &mut self,
        _data: &mut Smallvil,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        details: AxisFrame,
    ) {
        handle.axis(details)
    }

    fn start_data(&self) -> &PointerGrabStartData {
        &self.start_data
    }
}
