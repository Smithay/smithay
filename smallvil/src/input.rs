use smithay::{
    backend::input::{
        Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
        PointerButtonEvent, PointerMotionAbsoluteEvent,
    },
    reexports::wayland_server::{protocol::wl_pointer, Display},
    wayland::{
        seat::{AxisFrame, ButtonEvent, FilterResult, MotionEvent},
        SERIAL_COUNTER,
    },
};

use crate::state::Smallvil;

impl Smallvil {
    pub fn process_input_event<I: InputBackend>(
        &mut self,
        display: &mut Display<Smallvil>,
        event: InputEvent<I>,
    ) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let dh = &mut display.handle();

                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time(&event);

                self.seat.get_keyboard().unwrap().input::<(), _>(
                    dh,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
                    |_, _| FilterResult::Forward,
                );
            }
            InputEvent::PointerMotion { .. } => {}
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self.space.outputs().next().unwrap();

                let output_geo = self.space.output_geometry(output).unwrap();

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                self.pointer_location = pos;

                let serial = SERIAL_COUNTER.next_serial();

                let under = self.surface_under_pointer();

                let dh = &mut display.handle();
                self.seat.get_pointer().unwrap().motion(
                    self,
                    dh,
                    &MotionEvent {
                        location: pos,
                        focus: under,
                        serial,
                        time: event.time(),
                    },
                );
            }
            InputEvent::PointerButton { event, .. } => {
                let dh = &mut display.handle();
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();

                let serial = SERIAL_COUNTER.next_serial();

                let button = event.button_code();

                // TODO: From trait
                let button_state = match event.state() {
                    ButtonState::Pressed => wl_pointer::ButtonState::Pressed,
                    ButtonState::Released => wl_pointer::ButtonState::Released,
                };

                if wl_pointer::ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some(window) = self.space.window_under(self.pointer_location).cloned() {
                        self.space.raise_window(&window, true);
                        keyboard.set_focus(dh, Some(window.toplevel().wl_surface()), serial);
                        window.set_activated(true);
                        window.configure();
                    } else {
                        self.space.windows().for_each(|window| {
                            window.set_activated(false);
                            window.configure();
                        });
                        keyboard.set_focus(dh, None, serial);
                    }
                };

                pointer.button(
                    self,
                    dh,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time(),
                    },
                );
            }
            InputEvent::PointerAxis { event, .. } => {
                // TODO: From trait
                let source = match event.source() {
                    AxisSource::Continuous => wl_pointer::AxisSource::Continuous,
                    AxisSource::Finger => wl_pointer::AxisSource::Finger,
                    AxisSource::Wheel | AxisSource::WheelTilt => wl_pointer::AxisSource::Wheel,
                };
                let horizontal_amount = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_discrete(Axis::Horizontal).unwrap() * 3.0);
                let vertical_amount = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_discrete(Axis::Vertical).unwrap() * 3.0);
                let horizontal_amount_discrete = event.amount_discrete(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_discrete(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(wl_pointer::Axis::HorizontalScroll, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.discrete(wl_pointer::Axis::HorizontalScroll, discrete as i32);
                    }
                } else if source == wl_pointer::AxisSource::Finger {
                    frame = frame.stop(wl_pointer::Axis::HorizontalScroll);
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(wl_pointer::Axis::VerticalScroll, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.discrete(wl_pointer::Axis::VerticalScroll, discrete as i32);
                    }
                } else if source == wl_pointer::AxisSource::Finger {
                    frame = frame.stop(wl_pointer::Axis::VerticalScroll);
                }

                let dh = &mut display.handle();
                self.seat.get_pointer().unwrap().axis(self, dh, frame);
            }
            _ => {}
        }
    }
}
