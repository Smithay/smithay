use super::{KeyboardTarget, KeysymHandle, LedState, ModifiersState, XkbConfig};
use crate::{
    backend::input::{InputTime, KeyState},
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
        },
        touch::{DownEvent, FrameMarker, OrientationEvent, ShapeEvent, TouchTarget, UpEvent},
    },
    utils::{IsAlive, Serial},
};

#[derive(Debug, Clone, PartialEq)]
struct Target;

impl IsAlive for Target {
    fn alive(&self) -> bool {
        true
    }
}

impl KeyboardTarget<State> for Target {
    fn enter(&self, _: &Seat<State>, _: &mut State, _: Vec<KeysymHandle<'_>>, _: Serial) {}
    fn leave(&self, _: &Seat<State>, _: &mut State, _: Serial) {}
    fn key(&self, _: &Seat<State>, _: &mut State, _: KeysymHandle<'_>, _: KeyState, _: Serial, _: InputTime) {
    }
    fn modifiers(&self, _: &Seat<State>, _: &mut State, _: ModifiersState, _: Serial) {}
}

impl PointerTarget<State> for Target {
    fn enter(&self, _: &Seat<State>, _: &mut State, _: &MotionEvent) {}
    fn motion(&self, _: &Seat<State>, _: &mut State, _: &MotionEvent) {}
    fn relative_motion(&self, _: &Seat<State>, _: &mut State, _: &RelativeMotionEvent) {}
    fn button(&self, _: &Seat<State>, _: &mut State, _: &ButtonEvent) {}
    fn axis(&self, _: &Seat<State>, _: &mut State, _: AxisFrame) {}
    fn frame(&self, _: &Seat<State>, _: &mut State) {}
    fn gesture_swipe_begin(&self, _: &Seat<State>, _: &mut State, _: &GestureSwipeBeginEvent) {}
    fn gesture_swipe_update(&self, _: &Seat<State>, _: &mut State, _: &GestureSwipeUpdateEvent) {}
    fn gesture_swipe_end(&self, _: &Seat<State>, _: &mut State, _: &GestureSwipeEndEvent) {}
    fn gesture_pinch_begin(&self, _: &Seat<State>, _: &mut State, _: &GesturePinchBeginEvent) {}
    fn gesture_pinch_update(&self, _: &Seat<State>, _: &mut State, _: &GesturePinchUpdateEvent) {}
    fn gesture_pinch_end(&self, _: &Seat<State>, _: &mut State, _: &GesturePinchEndEvent) {}
    fn gesture_hold_begin(&self, _: &Seat<State>, _: &mut State, _: &GestureHoldBeginEvent) {}
    fn gesture_hold_end(&self, _: &Seat<State>, _: &mut State, _: &GestureHoldEndEvent) {}
    fn leave(&self, _: &Seat<State>, _: &mut State, _: Serial, _: InputTime) {}
}

impl TouchTarget<State> for Target {
    fn down(&self, _: &Seat<State>, _: &mut State, _: &DownEvent) {}
    fn up(&self, _: &Seat<State>, _: &mut State, _: &UpEvent) {}
    fn motion(&self, _: &Seat<State>, _: &mut State, _: &super::super::touch::MotionEvent) {}
    fn frame(&self, _: &Seat<State>, _: &mut State, _: FrameMarker) {}
    fn cancel(&self, _: &Seat<State>, _: &mut State, _: FrameMarker) {}
    fn shape(&self, _: &Seat<State>, _: &mut State, _: &ShapeEvent) {}
    fn orientation(&self, _: &Seat<State>, _: &mut State, _: &OrientationEvent) {}
    fn last_frame(&self, _: &Seat<State>, _: &mut State) -> Option<FrameMarker> {
        None
    }
}

struct State {
    seat_state: SeatState<State>,
    led_states: Vec<LedState>,
}

impl SeatHandler for State {
    type KeyboardFocus = Target;
    type PointerFocus = Target;
    type TouchFocus = Target;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn led_state_changed(&mut self, _: &Seat<Self>, led_state: LedState) {
        self.led_states.push(led_state);
    }
}

#[test]
fn set_modifier_state_notifies_led_state_changed() {
    let mut state = State {
        seat_state: SeatState::new(),
        led_states: Vec::new(),
    };
    let mut seat = state.seat_state.new_seat("seat-0");
    let keyboard = seat.add_keyboard(XkbConfig::default(), 200, 25).unwrap();

    let num_lock = ModifiersState {
        num_lock: true,
        ..Default::default()
    };
    keyboard.set_modifier_state(&mut state, num_lock);
    assert_eq!(state.led_states.len(), 1);
    assert_eq!(state.led_states[0].num, Some(true));

    // Nothing changed: no notification.
    assert_eq!(keyboard.set_modifier_state(&mut state, num_lock), 0);
    assert_eq!(state.led_states.len(), 1);

    // The modifiers changed, but no LED did: no notification.
    let shift = ModifiersState {
        shift: true,
        ..num_lock
    };
    assert_ne!(keyboard.set_modifier_state(&mut state, shift), 0);
    assert_eq!(state.led_states.len(), 1);
}
