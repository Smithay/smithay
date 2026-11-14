use super::{KeyboardHandle, KeyboardTarget, KeysymHandle, Layout, ModifiersState, XkbConfig};
use crate::{
    backend::input::{InputTime, KeyState},
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
        },
        touch::{self, DownEvent, FrameMarker, OrientationEvent, ShapeEvent, TouchTarget, UpEvent},
    },
    utils::{IsAlive, Serial},
};

/// A focus that ignores everything.
#[derive(Debug, Clone, PartialEq)]
struct Focus;

impl IsAlive for Focus {
    fn alive(&self) -> bool {
        true
    }
}

impl KeyboardTarget<State> for Focus {
    fn enter(&self, _: &Seat<State>, _: &mut State, _: Vec<KeysymHandle<'_>>, _: Serial) {}
    fn leave(&self, _: &Seat<State>, _: &mut State, _: Serial) {}
    fn key(&self, _: &Seat<State>, _: &mut State, _: KeysymHandle<'_>, _: KeyState, _: Serial, _: InputTime) {
    }
    fn modifiers(&self, _: &Seat<State>, _: &mut State, _: ModifiersState, _: Serial) {}
}

impl PointerTarget<State> for Focus {
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

impl TouchTarget<State> for Focus {
    fn down(&self, _: &Seat<State>, _: &mut State, _: &DownEvent) {}
    fn up(&self, _: &Seat<State>, _: &mut State, _: &UpEvent) {}
    fn motion(&self, _: &Seat<State>, _: &mut State, _: &touch::MotionEvent) {}
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
}

impl SeatHandler for State {
    type KeyboardFocus = Focus;
    type PointerFocus = Focus;
    type TouchFocus = Focus;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

fn active_layout(keyboard: &KeyboardHandle<State>, state: &mut State) -> Layout {
    keyboard.with_xkb_state(state, |context| context.xkb().lock().unwrap().active_layout())
}

#[test]
fn set_modifier_state_keeps_layout() {
    let mut state = State {
        seat_state: SeatState::new(),
    };
    let mut seat = state.seat_state.new_seat("seat-0");
    let config = XkbConfig {
        layout: "us,de",
        ..Default::default()
    };
    let keyboard = seat.add_keyboard(config, 200, 25).unwrap();

    keyboard.with_xkb_state(&mut state, |mut context| context.set_layout(Layout(1)));
    assert_eq!(active_layout(&keyboard, &mut state), Layout(1));

    let caps_lock = ModifiersState {
        caps_lock: true,
        ..keyboard.modifier_state()
    };
    assert_ne!(keyboard.set_modifier_state(caps_lock), 0);
    assert!(keyboard.modifier_state().caps_lock);
    assert_eq!(keyboard.modifier_state().serialized.layout_effective, 1);
    assert_eq!(active_layout(&keyboard, &mut state), Layout(1));
}
