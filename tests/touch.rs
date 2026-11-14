#![allow(unused_variables)]

use smithay::backend::input::{InputTime, KeyState};
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::input::pointer::*;
use smithay::input::touch::{
    DownEvent, FrameMarker, MotionEvent as TouchMotionEvent, OrientationEvent, ShapeEvent, TouchHandle,
    TouchTarget, UpEvent,
};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::utils::{IsAlive, Logical, Point, Serial};

struct State {
    seat_state: SeatState<Self>,
    motions: Vec<(u8, Point<f64, Logical>)>,
}

#[derive(Clone, Debug, PartialEq)]
struct Target(u8);
impl IsAlive for Target {
    fn alive(&self) -> bool {
        true
    }
}
impl KeyboardTarget<State> for Target {
    fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {}
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {}
    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
    }
    fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {}
}
impl PointerTarget<State> for Target {
    fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
    fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {}
    fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
    fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
    fn frame(&self, seat: &Seat<State>, data: &mut State) {}
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: InputTime) {}
    fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {}
    fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {}
    fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {}
    fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {}
    fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {}
    fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {}
    fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {}
    fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {}
}
impl TouchTarget<State> for Target {
    fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent) {}
    fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent) {}
    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &TouchMotionEvent) {
        data.motions.push((self.0, event.location));
    }
    fn frame(&self, seat: &Seat<State>, data: &mut State, marker: FrameMarker) {}
    fn cancel(&self, seat: &Seat<State>, data: &mut State, marker: FrameMarker) {}
    fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent) {}
    fn orientation(&self, seat: &Seat<State>, data: &mut State, event: &OrientationEvent) {}
    fn last_frame(&self, seat: &Seat<State>, data: &mut State) -> Option<FrameMarker> {
        None
    }
}
impl SeatHandler for State {
    type KeyboardFocus = Target;
    type PointerFocus = Target;
    type TouchFocus = Target;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

fn pressed() -> (State, TouchHandle<State>) {
    let mut state = State {
        seat_state: SeatState::new(),
        motions: vec![],
    };
    let mut seat = state.seat_state.new_seat("test");
    let touch = seat.add_touch();
    touch.down(
        &mut state,
        Some((Target(1), (100.0, 100.0).into())),
        &DownEvent {
            slot: Some(0).into(),
            location: (110.0, 110.0).into(),
            serial: 1.into(),
            time: InputTime::from_millis(1),
        },
    );
    touch.frame(&mut state);
    assert!(touch.is_grabbed());
    (state, touch)
}

fn move_to(state: &mut State, touch: &TouchHandle<State>, focus: Option<(Target, Point<f64, Logical>)>) {
    touch.motion(
        state,
        focus,
        &TouchMotionEvent {
            slot: Some(0).into(),
            location: (135.0, 110.0).into(),
            time: InputTime::from_millis(2),
        },
    );
    touch.frame(state);
}

#[test]
fn implicit_grab_uses_and_retains_current_surface_origin() {
    let (mut state, touch) = pressed();
    move_to(&mut state, &touch, Some((Target(1), (120.0, 100.0).into())));
    move_to(&mut state, &touch, None);
    move_to(&mut state, &touch, Some((Target(2), (500.0, 500.0).into())));
    assert_eq!(state.motions, vec![(1, (15.0, 10.0).into()); 3]);
    assert_eq!(
        touch.grab_start_data().unwrap().focus.unwrap().1,
        (100.0, 100.0).into()
    );
}

#[test]
fn without_implicit_grab_current_origin_and_fallback_work() {
    let (mut state, touch) = pressed();
    touch.unset_grab(&mut state);
    move_to(&mut state, &touch, Some((Target(1), (120.0, 100.0).into())));
    move_to(&mut state, &touch, None);
    move_to(&mut state, &touch, Some((Target(2), (500.0, 500.0).into())));
    assert_eq!(state.motions, vec![(1, (15.0, 10.0).into()); 3]);
}

#[test]
fn implicit_grab_keeps_target_and_origin_for_other_or_missing_focus() {
    let (mut state, touch) = pressed();
    move_to(&mut state, &touch, Some((Target(2), (500.0, 500.0).into())));
    move_to(&mut state, &touch, None);
    assert_eq!(state.motions, vec![(1, (35.0, 10.0).into()); 2]);
}
