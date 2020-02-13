use std::{error::Error, fmt};

use smithay::backend::input::*;

pub struct TestInputBackend {
    handler: Option<Box<dyn InputHandler<Self> + 'static>>,
    input_config: (),
    seat: Seat,
    pending_events: Vec<TestEvent>,
}

#[derive(Debug)]
pub enum TestInputBackendError {}

pub enum TestEvent {
    PointerButton(TestPointerButtonEvent),
    PointerMotionAbsolute(TestPointerMotionAbsoluteEvent),
}

pub struct TestPointerButtonEvent {
    pub time: u32,
    pub button: MouseButton,
    pub state: MouseButtonState,
}

pub struct TestPointerMotionAbsoluteEvent {
    pub time: u32,
    pub x: f64,
    pub y: f64,
}

impl TestInputBackend {
    pub fn new() -> Self {
        Self {
            handler: None,
            input_config: (),
            seat: Seat::new(
                0,
                "test",
                SeatCapabilities {
                    pointer: true,
                    keyboard: true,
                    touch: false,
                },
            ),
            pending_events: Vec::new(),
        }
    }

    pub fn push_event(&mut self, event: TestEvent) {
        self.pending_events.push(event);
    }
}

impl InputBackend for TestInputBackend {
    type InputConfig = ();
    type EventError = TestInputBackendError;
    type KeyboardKeyEvent = UnusedEvent;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = TestPointerButtonEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = TestPointerMotionAbsoluteEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;

    fn set_handler<H: InputHandler<Self> + 'static>(&mut self, mut handler: H) {
        self.clear_handler();

        handler.on_seat_created(&self.seat);
        self.handler = Some(Box::new(handler));
    }

    fn get_handler(&mut self) -> Option<&mut dyn InputHandler<Self>> {
        self.handler.as_deref_mut().map(|x| x as _)
    }

    fn clear_handler(&mut self) {
        if let Some(mut handler) = self.handler.take() {
            handler.on_seat_destroyed(&self.seat);
        }
    }

    fn input_config(&mut self) -> &mut <Self as InputBackend>::InputConfig {
        &mut self.input_config
    }

    fn dispatch_new_events(&mut self) -> Result<(), <Self as InputBackend>::EventError> {
        let handler = self.handler.as_deref_mut().unwrap();

        for event in self.pending_events.drain(..) {
            use TestEvent::*;
            match event {
                PointerButton(e) => handler.on_pointer_button(&self.seat, e),
                PointerMotionAbsolute(e) => handler.on_pointer_move_absolute(&self.seat, e),
            }
        }

        Ok(())
    }
}

impl fmt::Display for TestInputBackendError {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        unreachable!()
    }
}

impl Error for TestInputBackendError {}

impl Event for TestPointerButtonEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerButtonEvent for TestPointerButtonEvent {
    fn button(&self) -> MouseButton {
        self.button
    }

    fn state(&self) -> MouseButtonState {
        self.state
    }
}

impl Event for TestPointerMotionAbsoluteEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerMotionAbsoluteEvent for TestPointerMotionAbsoluteEvent {
    fn x(&self) -> f64 {
        self.x
    }

    fn y(&self) -> f64 {
        self.y
    }

    fn x_transformed(&self, _: u32) -> u32 {
        self.x as u32
    }

    fn y_transformed(&self, _: u32) -> u32 {
        self.y as u32
    }
}
