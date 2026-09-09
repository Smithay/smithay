use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::*;
use crate::input::{
    SeatState,
    pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
    },
    touch::{
        DownEvent, FrameMarker, MotionEvent as TouchMotionEvent, OrientationEvent, ShapeEvent, TouchTarget,
        UpEvent,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestTarget;

impl IsAlive for TestTarget {
    fn alive(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetEvent {
    Modifiers,
    Key(Keycode, KeyState),
}

struct TestState {
    seat_state: SeatState<Self>,
    target_events: Vec<TargetEvent>,
    entered_keys: Vec<Vec<Keycode>>,
}

impl TestState {
    fn new() -> Self {
        Self {
            seat_state: SeatState::new(),
            target_events: Vec::new(),
            entered_keys: Vec::new(),
        }
    }
}

impl SeatHandler for TestState {
    type KeyboardFocus = TestTarget;
    type PointerFocus = TestTarget;
    type TouchFocus = TestTarget;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

impl KeyboardTarget<TestState> for TestTarget {
    fn enter(
        &self,
        _seat: &Seat<TestState>,
        data: &mut TestState,
        keys: Vec<KeysymHandle<'_>>,
        _serial: Serial,
    ) {
        data.entered_keys
            .push(keys.into_iter().map(|key| key.raw_code()).collect());
    }

    fn leave(&self, _seat: &Seat<TestState>, _data: &mut TestState, _serial: Serial) {}

    fn key(
        &self,
        _seat: &Seat<TestState>,
        data: &mut TestState,
        key: KeysymHandle<'_>,
        state: KeyState,
        _serial: Serial,
        _time: InputTime,
    ) {
        data.target_events.push(TargetEvent::Key(key.raw_code(), state));
    }

    fn modifiers(
        &self,
        _seat: &Seat<TestState>,
        data: &mut TestState,
        _modifiers: ModifiersState,
        _serial: Serial,
    ) {
        data.target_events.push(TargetEvent::Modifiers);
    }
}

impl PointerTarget<TestState> for TestTarget {
    fn enter(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &MotionEvent) {}
    fn motion(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &MotionEvent) {}
    fn relative_motion(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &RelativeMotionEvent) {}
    fn button(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &ButtonEvent) {}
    fn axis(&self, _seat: &Seat<TestState>, _data: &mut TestState, _frame: AxisFrame) {}
    fn frame(&self, _seat: &Seat<TestState>, _data: &mut TestState) {}
    fn gesture_swipe_begin(
        &self,
        _seat: &Seat<TestState>,
        _data: &mut TestState,
        _event: &GestureSwipeBeginEvent,
    ) {
    }
    fn gesture_swipe_update(
        &self,
        _seat: &Seat<TestState>,
        _data: &mut TestState,
        _event: &GestureSwipeUpdateEvent,
    ) {
    }
    fn gesture_swipe_end(
        &self,
        _seat: &Seat<TestState>,
        _data: &mut TestState,
        _event: &GestureSwipeEndEvent,
    ) {
    }
    fn gesture_pinch_begin(
        &self,
        _seat: &Seat<TestState>,
        _data: &mut TestState,
        _event: &GesturePinchBeginEvent,
    ) {
    }
    fn gesture_pinch_update(
        &self,
        _seat: &Seat<TestState>,
        _data: &mut TestState,
        _event: &GesturePinchUpdateEvent,
    ) {
    }
    fn gesture_pinch_end(
        &self,
        _seat: &Seat<TestState>,
        _data: &mut TestState,
        _event: &GesturePinchEndEvent,
    ) {
    }
    fn gesture_hold_begin(
        &self,
        _seat: &Seat<TestState>,
        _data: &mut TestState,
        _event: &GestureHoldBeginEvent,
    ) {
    }
    fn gesture_hold_end(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &GestureHoldEndEvent) {
    }
    fn leave(&self, _seat: &Seat<TestState>, _data: &mut TestState, _serial: Serial, _time: InputTime) {}
}

impl TouchTarget<TestState> for TestTarget {
    fn down(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &DownEvent) {}
    fn up(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &UpEvent) {}
    fn motion(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &TouchMotionEvent) {}
    fn frame(&self, _seat: &Seat<TestState>, _data: &mut TestState, _marker: FrameMarker) {}
    fn cancel(&self, _seat: &Seat<TestState>, _data: &mut TestState, _marker: FrameMarker) {}
    fn shape(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &ShapeEvent) {}
    fn orientation(&self, _seat: &Seat<TestState>, _data: &mut TestState, _event: &OrientationEvent) {}
    fn last_frame(&self, _seat: &Seat<TestState>, _data: &mut TestState) -> Option<FrameMarker> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InterceptorEvent {
    source: KeyboardSource,
    keycode: Keycode,
    state: KeyState,
    modifiers: bool,
}

struct RecordingInterceptor {
    events: Arc<Mutex<Vec<InterceptorEvent>>>,
    results: VecDeque<KeyboardInputInterception>,
}

impl RecordingInterceptor {
    fn new(
        events: Arc<Mutex<Vec<InterceptorEvent>>>,
        results: impl IntoIterator<Item = KeyboardInputInterception>,
    ) -> Self {
        Self {
            events,
            results: results.into_iter().collect(),
        }
    }
}

impl KeyboardInputInterceptor<TestState> for RecordingInterceptor {
    fn input(
        &mut self,
        _data: &mut TestState,
        _seat: &Seat<TestState>,
        source: KeyboardSource,
        keycode: Keycode,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        _serial: Serial,
        _time: InputTime,
    ) -> KeyboardInputInterception {
        self.events.lock().unwrap().push(InterceptorEvent {
            source,
            keycode,
            state,
            modifiers: modifiers.is_some(),
        });
        self.results
            .pop_front()
            .unwrap_or(KeyboardInputInterception::Forward)
    }
}

struct TestRoutingGrab {
    start_data: GrabStartData<TestState>,
    calls: Arc<AtomicUsize>,
    forward: bool,
}

impl TestRoutingGrab {
    fn new(calls: Arc<AtomicUsize>, forward: bool) -> Self {
        Self {
            start_data: GrabStartData {
                focus: Some(TestTarget),
            },
            calls,
            forward,
        }
    }
}

impl KeyboardGrab<TestState> for TestRoutingGrab {
    fn input(
        &mut self,
        data: &mut TestState,
        handle: &mut KeyboardInnerHandle<'_, TestState>,
        keycode: Keycode,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: InputTime,
    ) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.forward {
            handle.input(data, keycode, state, modifiers, serial, time);
        }
    }

    fn set_focus(
        &mut self,
        data: &mut TestState,
        handle: &mut KeyboardInnerHandle<'_, TestState>,
        focus: Option<TestTarget>,
        serial: Serial,
    ) {
        handle.set_focus(data, focus, serial);
    }

    fn start_data(&self) -> &GrabStartData<TestState> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut TestState) {}
}

fn setup() -> (TestState, Seat<TestState>, KeyboardHandle<TestState>) {
    let mut state = TestState::new();
    let mut seat = state.seat_state.new_seat("test-seat");
    let keyboard = seat
        .add_keyboard(XkbConfig::default(), 200, 25)
        .expect("failed to initialize test keyboard");
    keyboard.set_focus(&mut state, Some(TestTarget), SERIAL_COUNTER.next_serial());
    state.target_events.clear();
    state.entered_keys.clear();
    (state, seat, keyboard)
}

fn forward_input_from_source(
    state: &mut TestState,
    keyboard: &KeyboardHandle<TestState>,
    source: KeyboardSource,
    keycode: Keycode,
    key_state: KeyState,
) {
    keyboard.input_from_source(
        source,
        state,
        keycode,
        key_state,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        |_, _, _| FilterResult::<()>::Forward,
    );
}

#[test]
fn input_interceptor_does_not_change_grab_state() {
    let (mut state, _seat, keyboard) = setup();
    let events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    assert!(!keyboard.is_grabbed());

    let serial = SERIAL_COUNTER.next_serial();
    let calls = Arc::new(AtomicUsize::new(0));
    keyboard.set_grab(&mut state, TestRoutingGrab::new(calls, true), serial);
    assert!(keyboard.is_grabbed());
    assert!(keyboard.has_grab(serial));

    keyboard.unset_grab(&mut state);
    assert!(!keyboard.is_grabbed());
    assert!(!keyboard.has_grab(serial));

    keyboard.input_forward(
        &mut state,
        Keycode::new(29),
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(state.target_events.is_empty());
}

#[test]
fn forwarded_press_without_focus_is_advertised_on_later_enter() {
    let (mut state, _seat, keyboard) = setup();
    keyboard.set_focus(&mut state, None, SERIAL_COUNTER.next_serial());

    let keycode = Keycode::new(43);
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Pressed,
    );

    assert!(state.target_events.is_empty());
    assert!(state.entered_keys.is_empty());
    assert!(
        keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .forwarded_pressed_keys
            .contains(&keycode)
    );

    keyboard.set_focus(&mut state, Some(TestTarget), SERIAL_COUNTER.next_serial());
    assert_eq!(state.entered_keys, vec![vec![keycode]]);
    state.target_events.clear();

    let interceptor_events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        interceptor_events.clone(),
        [KeyboardInputInterception::Intercept],
    ));
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Released,
    );

    assert!(interceptor_events.lock().unwrap().is_empty());
    assert_eq!(
        state.target_events,
        vec![TargetEvent::Key(keycode, KeyState::Released)]
    );
    assert!(
        !keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .forwarded_pressed_keys
            .contains(&keycode)
    );
}

#[test]
fn routing_grab_forwards_into_interceptor() {
    let (mut state, _seat, keyboard) = setup();
    let interceptor_events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        interceptor_events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    let routing_calls = Arc::new(AtomicUsize::new(0));
    keyboard.set_grab(
        &mut state,
        TestRoutingGrab::new(routing_calls.clone(), true),
        SERIAL_COUNTER.next_serial(),
    );

    let keycode = Keycode::new(30);
    keyboard.input_forward(
        &mut state,
        keycode,
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );

    assert_eq!(routing_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        *interceptor_events.lock().unwrap(),
        vec![InterceptorEvent {
            source: KeyboardSource::Physical,
            keycode,
            state: KeyState::Pressed,
            modifiers: false,
        }]
    );
    assert!(state.target_events.is_empty());
}

#[test]
fn routing_grab_can_consume_before_interceptor() {
    let (mut state, _seat, keyboard) = setup();
    let interceptor_events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        interceptor_events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    let routing_calls = Arc::new(AtomicUsize::new(0));
    keyboard.set_grab(
        &mut state,
        TestRoutingGrab::new(routing_calls.clone(), false),
        SERIAL_COUNTER.next_serial(),
    );

    keyboard.input_forward(
        &mut state,
        Keycode::new(31),
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );

    assert_eq!(routing_calls.load(Ordering::Relaxed), 1);
    assert!(interceptor_events.lock().unwrap().is_empty());
    assert!(state.target_events.is_empty());
}

#[test]
fn routing_grab_consumed_release_clears_intercepted_pressed_state() {
    let (mut state, _seat, keyboard) = setup();
    let events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    let keycode = Keycode::new(41);
    keyboard.input_forward(
        &mut state,
        keycode,
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );
    assert!(
        keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .intercepted_pressed_keys
            .contains_key(&keycode)
    );

    let routing_calls = Arc::new(AtomicUsize::new(0));
    keyboard.set_grab(
        &mut state,
        TestRoutingGrab::new(routing_calls.clone(), false),
        SERIAL_COUNTER.next_serial(),
    );
    keyboard.input_forward(
        &mut state,
        keycode,
        KeyState::Released,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );

    assert_eq!(routing_calls.load(Ordering::Relaxed), 1);
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(state.target_events.is_empty());
    assert!(
        !keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .intercepted_pressed_keys
            .contains_key(&keycode)
    );
}

#[test]
fn consumed_release_clears_forwarded_pressed_state() {
    let (mut state, _seat, keyboard) = setup();
    let keycode = Keycode::new(32);

    keyboard.input_forward(
        &mut state,
        keycode,
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );
    assert!(
        keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .forwarded_pressed_keys
            .contains(&keycode)
    );

    keyboard.set_grab(
        &mut state,
        TestRoutingGrab::new(Arc::new(AtomicUsize::new(0)), false),
        SERIAL_COUNTER.next_serial(),
    );
    keyboard.input_forward(
        &mut state,
        keycode,
        KeyState::Released,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );

    assert!(
        !keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .forwarded_pressed_keys
            .contains(&keycode)
    );
    assert_eq!(
        state.target_events,
        vec![TargetEvent::Key(keycode, KeyState::Pressed)]
    );
}

#[test]
fn interceptor_does_not_steal_release_for_forwarded_press() {
    let (mut state, _seat, keyboard) = setup();
    let keycode = Keycode::new(33);

    keyboard.input_forward(
        &mut state,
        keycode,
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );

    let interceptor_events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        interceptor_events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    keyboard.input_forward(
        &mut state,
        keycode,
        KeyState::Released,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );

    assert!(interceptor_events.lock().unwrap().is_empty());
    assert_eq!(
        state.target_events,
        vec![
            TargetEvent::Key(keycode, KeyState::Pressed),
            TargetEvent::Key(keycode, KeyState::Released),
        ]
    );

    let intercepted_key = Keycode::new(34);
    keyboard.input_forward(
        &mut state,
        intercepted_key,
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );
    assert_eq!(interceptor_events.lock().unwrap().len(), 1);
    assert_eq!(state.target_events.len(), 2);
}

#[test]
fn intercepted_release_does_not_leak_after_interceptor_is_unset() {
    let (mut state, _seat, keyboard) = setup();
    let events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    let keycode = Keycode::new(37);
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Pressed,
    );
    keyboard.unset_input_interceptor();
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Released,
    );

    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(state.target_events.is_empty());
    assert!(
        !keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .intercepted_pressed_keys
            .contains_key(&keycode)
    );
}

#[test]
fn filtered_release_does_not_poison_next_key_cycle() {
    let (mut state, _seat, keyboard) = setup();
    let events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    let keycode = Keycode::new(42);
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Pressed,
    );

    let intercepted = keyboard.input_from_source(
        KeyboardSource::Physical,
        &mut state,
        keycode,
        KeyState::Released,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        |_, _, _| FilterResult::Intercept(()),
    );
    assert_eq!(intercepted, Some(()));
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(state.target_events.is_empty());

    // The release never reached post-routing dispatch, so ownership is cleaned when the next
    // actual down transition starts rather than being allowed to capture the next cycle's release.
    keyboard.unset_input_interceptor();
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Pressed,
    );
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Released,
    );

    assert_eq!(
        state.target_events,
        vec![
            TargetEvent::Key(keycode, KeyState::Pressed),
            TargetEvent::Key(keycode, KeyState::Released),
        ]
    );
    assert!(
        !keyboard
            .arc
            .internal
            .lock()
            .unwrap()
            .intercepted_pressed_keys
            .contains_key(&keycode)
    );
}

#[test]
fn replacing_interceptor_does_not_rehome_held_release() {
    let (mut state, _seat, keyboard) = setup();
    let old_events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        old_events.clone(),
        [KeyboardInputInterception::Intercept],
    ));

    let keycode = Keycode::new(38);
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Pressed,
    );

    let new_events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        new_events.clone(),
        [KeyboardInputInterception::Intercept],
    ));
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Released,
    );

    assert_eq!(old_events.lock().unwrap().len(), 1);
    assert!(new_events.lock().unwrap().is_empty());
    assert!(state.target_events.is_empty());

    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        Keycode::new(39),
        KeyState::Pressed,
    );
    assert_eq!(new_events.lock().unwrap().len(), 1);
}

#[test]
fn intercepted_press_release_stays_paired_across_sources() {
    let (mut state, _seat, keyboard) = setup();
    let events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        events.clone(),
        [
            KeyboardInputInterception::Intercept,
            KeyboardInputInterception::Forward,
        ],
    ));

    let keycode = Keycode::new(40);
    let auxiliary = KeyboardSource::new_auxiliary();
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Pressed,
    );
    forward_input_from_source(&mut state, &keyboard, auxiliary, keycode, KeyState::Pressed);
    forward_input_from_source(
        &mut state,
        &keyboard,
        KeyboardSource::Physical,
        keycode,
        KeyState::Released,
    );
    forward_input_from_source(&mut state, &keyboard, auxiliary, keycode, KeyState::Released);

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            InterceptorEvent {
                source: KeyboardSource::Physical,
                keycode,
                state: KeyState::Pressed,
                modifiers: false,
            },
            InterceptorEvent {
                source: KeyboardSource::Physical,
                keycode,
                state: KeyState::Released,
                modifiers: false,
            },
        ]
    );
    assert!(state.target_events.is_empty());
}

#[test]
fn intercepted_modifiers_resync_and_source_survives_routing() {
    let (mut state, _seat, keyboard) = setup();
    let interceptor_events = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_input_interceptor(RecordingInterceptor::new(
        interceptor_events.clone(),
        [
            KeyboardInputInterception::Intercept,
            KeyboardInputInterception::Forward,
        ],
    ));

    keyboard.input_forward(
        &mut state,
        Keycode::new(35),
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        true,
    );
    assert!(state.target_events.is_empty());

    let source = KeyboardSource::new_auxiliary();
    let keycode = Keycode::new(36);
    keyboard.input_forward_from_source(
        source,
        &mut state,
        keycode,
        KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        false,
    );

    let events = interceptor_events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].source, KeyboardSource::Physical);
    assert!(events[0].modifiers);
    assert_eq!(events[1].source, source);
    assert!(!events[1].modifiers);
    drop(events);

    assert_eq!(
        state.target_events,
        vec![
            TargetEvent::Modifiers,
            TargetEvent::Key(keycode, KeyState::Pressed)
        ]
    );
}
