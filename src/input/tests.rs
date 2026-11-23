//! Tests for grabs that are active at the same time on one device.
//!
//! The grabs in here are small stand-ins that behave like the real ones in the way that matters
//! for the test, without needing a Wayland client: [`OwnerEvents`] treats the pointer like
//! `PopupPointerGrab`, [`Drag`] like `DnDGrab`, [`FocusLock`] treats the keyboard like
//! `PopupKeyboardGrab` and [`Filter`] like the keyboard grab of an input method.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use super::{
    Seat, SeatHandler, SeatState,
    keyboard::{
        FilterResult, GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle,
        KeyboardInnerHandle, KeyboardTarget, KeysymHandle, ModifiersState, XkbConfig,
    },
    pointer::{
        AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
        PointerHandle, PointerInnerHandle, PointerTarget, RelativeMotionEvent,
    },
    touch::{DownEvent, FrameMarker, MotionEvent as TouchMotionEvent, OrientationEvent, ShapeEvent, UpEvent},
};
use crate::{
    backend::input::{ButtonState, InputTime, KeyState, Keycode},
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER, Serial},
};

/// A surface, owned by a client.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    name: &'static str,
    client: u32,
}

const A: Target = Target { name: "a", client: 1 };
/// A second surface of the client that owns `A`, think of a popup.
const A_POPUP: Target = Target {
    name: "a-popup",
    client: 1,
};
const B: Target = Target { name: "b", client: 2 };

impl IsAlive for Target {
    fn alive(&self) -> bool {
        true
    }
}

struct State {
    seat_state: SeatState<Self>,
    /// What the clients were sent.
    log: Vec<String>,
}

impl State {
    fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
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

impl KeyboardTarget<State> for Target {
    fn enter(&self, _: &Seat<State>, data: &mut State, _: Vec<KeysymHandle<'_>>, _: Serial) {
        data.log.push(format!("enter {}", self.name));
    }
    fn leave(&self, _: &Seat<State>, data: &mut State, _: Serial) {
        data.log.push(format!("leave {}", self.name));
    }
    fn key(
        &self,
        _: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        _: Serial,
        _: InputTime,
    ) {
        data.log
            .push(format!("key {} {} {:?}", self.name, key.raw_code().raw(), state));
    }
    fn modifiers(&self, _: &Seat<State>, _: &mut State, _: ModifiersState, _: Serial) {}
}

impl PointerTarget<State> for Target {
    fn enter(&self, _: &Seat<State>, data: &mut State, _: &MotionEvent) {
        data.log.push(format!("enter {}", self.name));
    }
    fn motion(&self, _: &Seat<State>, data: &mut State, _: &MotionEvent) {
        data.log.push(format!("motion {}", self.name));
    }
    fn relative_motion(&self, _: &Seat<State>, _: &mut State, _: &RelativeMotionEvent) {}
    fn button(&self, _: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        data.log
            .push(format!("button {} {} {:?}", self.name, event.button, event.state));
    }
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
    fn leave(&self, _: &Seat<State>, data: &mut State, _: Serial, _: InputTime) {
        data.log.push(format!("leave {}", self.name));
    }
}

impl super::touch::TouchTarget<State> for Target {
    fn down(&self, _: &Seat<State>, _: &mut State, _: &DownEvent) {}
    fn up(&self, _: &Seat<State>, _: &mut State, _: &UpEvent) {}
    fn motion(&self, _: &Seat<State>, _: &mut State, _: &TouchMotionEvent) {}
    fn frame(&self, _: &Seat<State>, _: &mut State, _: FrameMarker) {}
    fn cancel(&self, _: &Seat<State>, _: &mut State, _: FrameMarker) {}
    fn shape(&self, _: &Seat<State>, _: &mut State, _: &ShapeEvent) {}
    fn orientation(&self, _: &Seat<State>, _: &mut State, _: &OrientationEvent) {}
    fn last_frame(&self, _: &Seat<State>, _: &mut State) -> Option<FrameMarker> {
        None
    }
}

fn setup() -> (State, PointerHandle<State>, KeyboardHandle<State>) {
    let mut state = State {
        seat_state: SeatState::new(),
        log: Vec::new(),
    };
    let mut seat = state.seat_state.new_seat("test");
    let pointer = seat.add_pointer();
    let keyboard = seat.add_keyboard(XkbConfig::default(), 200, 25).unwrap();
    (state, pointer, keyboard)
}

/// Where the test surfaces are on screen: one per 100 pixels.
fn under(x: f64) -> Option<(Target, Point<f64, Logical>)> {
    match x as i32 / 100 {
        0 => Some((A, (0.0, 0.0).into())),
        1 => Some((A_POPUP, (100.0, 0.0).into())),
        2 => Some((B, (200.0, 0.0).into())),
        _ => None,
    }
}

fn motion(state: &mut State, pointer: &PointerHandle<State>, x: f64) {
    let event = MotionEvent {
        location: (x, 0.0).into(),
        serial: SERIAL_COUNTER.next_serial(),
        time: InputTime::now(),
    };
    pointer.motion(state, under(x), &event);
}

fn button(state: &mut State, pointer: &PointerHandle<State>, button: u32, pressed: bool) -> Serial {
    let serial = SERIAL_COUNTER.next_serial();
    let event = ButtonEvent {
        serial,
        time: InputTime::now(),
        button,
        state: if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        },
    };
    pointer.button(state, &event);
    serial
}

fn key(state: &mut State, keyboard: &KeyboardHandle<State>, code: u32, pressed: bool) {
    let key_state = if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    keyboard.input::<(), _>(
        state,
        Keycode::new(code),
        key_state,
        SERIAL_COUNTER.next_serial(),
        InputTime::now(),
        |_, _, _| FilterResult::Forward,
    );
}

macro_rules! forward_the_rest {
    () => {
        fn relative_motion(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            focus: Option<(Target, Point<f64, Logical>)>,
            event: &RelativeMotionEvent,
        ) {
            handle.relative_motion(data, focus, event);
        }
        fn axis(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, details: AxisFrame) {
            handle.axis(data, details);
        }
        fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
            handle.frame(data);
        }
        fn gesture_swipe_begin(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureSwipeBeginEvent,
        ) {
            handle.gesture_swipe_begin(data, event);
        }
        fn gesture_swipe_update(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureSwipeUpdateEvent,
        ) {
            handle.gesture_swipe_update(data, event);
        }
        fn gesture_swipe_end(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureSwipeEndEvent,
        ) {
            handle.gesture_swipe_end(data, event);
        }
        fn gesture_pinch_begin(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GesturePinchBeginEvent,
        ) {
            handle.gesture_pinch_begin(data, event);
        }
        fn gesture_pinch_update(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GesturePinchUpdateEvent,
        ) {
            handle.gesture_pinch_update(data, event);
        }
        fn gesture_pinch_end(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GesturePinchEndEvent,
        ) {
            handle.gesture_pinch_end(data, event);
        }
        fn gesture_hold_begin(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureHoldBeginEvent,
        ) {
            handle.gesture_hold_begin(data, event);
        }
        fn gesture_hold_end(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureHoldEndEvent,
        ) {
            handle.gesture_hold_end(data, event);
        }
    };
}

/// Treats the pointer the way `PopupPointerGrab` does: the focus may only be on the surfaces of one
/// client, and a press anywhere else ends the grab.
struct OwnerEvents {
    client: u32,
    start_data: PointerGrabStartData<State>,
    unset: Arc<AtomicUsize>,
}

impl OwnerEvents {
    fn new(client: u32) -> (Self, Arc<AtomicUsize>) {
        let unset = Arc::new(AtomicUsize::new(0));
        let grab = Self {
            client,
            start_data: PointerGrabStartData {
                focus: None,
                button: 0,
                location: (0.0, 0.0).into(),
            },
            unset: unset.clone(),
        };
        (grab, unset)
    }
}

impl PointerGrab<State> for OwnerEvents {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(Target, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        let focus = focus.filter(|(target, _)| target.client == self.client);
        handle.motion(data, focus, event);
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        let inside = handle
            .current_focus()
            .is_some_and(|(target, _)| target.client == self.client);
        if event.state == ButtonState::Pressed && !inside {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
        handle.button(data, event);
    }

    forward_the_rest!();

    fn start_data(&self) -> &PointerGrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _: &mut State) {
        self.unset.fetch_add(1, Ordering::SeqCst);
    }
}

/// Treats the pointer the way `DnDGrab` does: nothing has the pointer focus while it is active, it
/// follows what is under the pointer by itself, and it ends when the last button is released.
struct Drag {
    start_data: PointerGrabStartData<State>,
    /// What the drag was over, motion by motion.
    over: Arc<Mutex<Vec<Option<&'static str>>>>,
}

impl PointerGrab<State> for Drag {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(Target, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        self.over
            .lock()
            .unwrap()
            .push(focus.map(|(target, _)| target.name));
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    forward_the_rest!();

    fn start_data(&self) -> &PointerGrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _: &mut State) {}
}

/// Treats the keyboard the way `PopupKeyboardGrab` does: the focus stays on one surface.
struct FocusLock {
    target: Target,
    ended: Arc<AtomicBool>,
    unset: Arc<AtomicUsize>,
}

impl FocusLock {
    fn new(target: Target) -> (Self, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let ended = Arc::new(AtomicBool::new(false));
        let unset = Arc::new(AtomicUsize::new(0));
        let grab = Self {
            target,
            ended: ended.clone(),
            unset: unset.clone(),
        };
        (grab, ended, unset)
    }
}

impl KeyboardGrab<State> for FocusLock {
    fn input(
        &mut self,
        data: &mut State,
        handle: &mut KeyboardInnerHandle<'_, State>,
        keycode: Keycode,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: InputTime,
    ) {
        if self.ended.load(Ordering::SeqCst) {
            handle.unset_grab(self, data, serial, false);
        } else {
            handle.set_focus(data, Some(self.target.clone()), serial);
        }
        handle.input(data, keycode, state, modifiers, serial, time);
    }

    fn set_focus(
        &mut self,
        data: &mut State,
        handle: &mut KeyboardInnerHandle<'_, State>,
        focus: Option<Target>,
        serial: Serial,
    ) {
        if self.ended.load(Ordering::SeqCst) {
            handle.set_focus(data, focus, serial);
            handle.unset_grab(self, data, serial, false);
        } else if focus.as_ref() == Some(&self.target) {
            handle.set_focus(data, focus, serial);
        }
    }

    fn start_data(&self) -> &KeyboardGrabStartData<State> {
        &KeyboardGrabStartData { focus: None }
    }

    fn unset(&mut self, _: &mut State) {
        self.unset.fetch_add(1, Ordering::SeqCst);
    }
}

/// Treats the keyboard the way the keyboard grab of an input method does: it takes the keys, and
/// stays for as long as its owner wants it to.
struct Filter {
    taken: Arc<Mutex<Vec<u32>>>,
}

impl KeyboardGrab<State> for Filter {
    fn input(
        &mut self,
        _: &mut State,
        _: &mut KeyboardInnerHandle<'_, State>,
        keycode: Keycode,
        _: KeyState,
        _: Option<ModifiersState>,
        _: Serial,
        _: InputTime,
    ) {
        self.taken.lock().unwrap().push(keycode.raw());
    }

    fn set_focus(
        &mut self,
        data: &mut State,
        handle: &mut KeyboardInnerHandle<'_, State>,
        focus: Option<Target>,
        serial: Serial,
    ) {
        handle.set_focus(data, focus, serial);
    }

    fn start_data(&self) -> &KeyboardGrabStartData<State> {
        &KeyboardGrabStartData { focus: None }
    }

    fn unset(&mut self, _: &mut State) {}

    fn persistent(&self) -> bool {
        true
    }
}

/// Takes every key, the way a compositor mode like a window switcher does.
struct Modal {
    taken: Arc<Mutex<Vec<u32>>>,
}

impl KeyboardGrab<State> for Modal {
    fn input(
        &mut self,
        _: &mut State,
        _: &mut KeyboardInnerHandle<'_, State>,
        keycode: Keycode,
        _: KeyState,
        _: Option<ModifiersState>,
        _: Serial,
        _: InputTime,
    ) {
        self.taken.lock().unwrap().push(keycode.raw());
    }

    fn set_focus(
        &mut self,
        data: &mut State,
        handle: &mut KeyboardInnerHandle<'_, State>,
        focus: Option<Target>,
        serial: Serial,
    ) {
        handle.set_focus(data, focus, serial);
    }

    fn start_data(&self) -> &KeyboardGrabStartData<State> {
        &KeyboardGrabStartData { focus: None }
    }

    fn unset(&mut self, _: &mut State) {}
}

/// The check every compositor took over from anvil before it hands out a popup grab.
fn popup_grab_is_denied(keyboard: &KeyboardHandle<State>, serial: Serial, previous: Option<Serial>) -> bool {
    keyboard.is_grabbed() && !(keyboard.has_grab(serial) || keyboard.has_grab(previous.unwrap_or(serial)))
}

fn serial() -> Serial {
    SERIAL_COUNTER.next_serial()
}

#[test]
fn a_popup_can_be_opened_while_an_input_method_holds_the_keyboard() {
    let (mut state, _pointer, keyboard) = setup();
    keyboard.set_focus(&mut state, Some(A), serial());

    // The input method grabs the keyboard, because a text field of `A` has the focus.
    let taken = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_grab(&mut state, Filter { taken: taken.clone() }, serial());
    key(&mut state, &keyboard, 38, true);
    key(&mut state, &keyboard, 38, false);
    assert_eq!(*taken.lock().unwrap(), [38, 38]);
    assert!(!state.take_log().iter().any(|l| l.starts_with("key")));

    // `A` asks for a popup grab. The input method is no reason to deny it.
    let popup_serial = serial();
    assert!(!keyboard.is_grabbed());
    assert!(!popup_grab_is_denied(&keyboard, popup_serial, None));
    keyboard.set_focus(&mut state, Some(A_POPUP), popup_serial);
    let (popup, _ended, _unset) = FocusLock::new(A_POPUP);
    keyboard.set_grab(&mut state, popup, popup_serial);
    assert!(keyboard.is_grabbed());
    assert!(keyboard.has_grab(popup_serial));
    assert_eq!(state.take_log(), ["leave a", "enter a-popup"]);

    // Both grabs do their part: the popup keeps the focus, the input method gets the keys.
    keyboard.set_focus(&mut state, Some(B), serial());
    assert_eq!(keyboard.current_focus(), Some(A_POPUP));
    key(&mut state, &keyboard, 39, true);
    assert_eq!(*taken.lock().unwrap(), [38, 38, 39]);
    assert!(state.take_log().is_empty());

    // What the input method does not want it sends back. That reaches the popup.
    keyboard.input_forward_bypassing_grab(
        &mut state,
        Keycode::new(39),
        KeyState::Pressed,
        serial(),
        InputTime::now(),
        false,
    );
    assert_eq!(state.take_log(), ["key a-popup 39 Pressed"]);

    // The popup is dismissed the way `PopupPointerGrab` does it. The input method stays.
    keyboard.unset_grab(&mut state);
    keyboard.set_focus(&mut state, Some(A), serial());
    assert!(!keyboard.is_grabbed());
    assert!(keyboard.has_grab_of::<Filter>());
    assert_eq!(state.take_log(), ["leave a-popup", "enter a"]);
    key(&mut state, &keyboard, 40, true);
    assert_eq!(*taken.lock().unwrap(), [38, 38, 39, 40]);
    assert!(state.take_log().is_empty());
}

#[test]
fn an_input_method_can_come_and_go_while_a_popup_is_open() {
    let (mut state, _pointer, keyboard) = setup();
    keyboard.set_focus(&mut state, Some(A_POPUP), serial());
    let (popup, ended, popup_unset) = FocusLock::new(A_POPUP);
    keyboard.set_grab(&mut state, popup, serial());
    state.take_log();

    // The popup has a text field, the input method grabs the keyboard for it.
    let taken = Arc::new(Mutex::new(Vec::new()));
    keyboard.set_grab(&mut state, Filter { taken: taken.clone() }, serial());
    assert_eq!(popup_unset.load(Ordering::SeqCst), 0);
    key(&mut state, &keyboard, 38, true);
    assert_eq!(*taken.lock().unwrap(), [38]);
    assert!(state.take_log().is_empty());

    // The input method lets go. That ends its own grab and nothing else.
    assert!(keyboard.unset_grab_of::<Filter>(&mut state));
    assert!(!keyboard.unset_grab_of::<Filter>(&mut state));
    assert_eq!(popup_unset.load(Ordering::SeqCst), 0);
    assert!(keyboard.is_grabbed());
    keyboard.set_focus(&mut state, Some(B), serial());
    assert_eq!(keyboard.current_focus(), Some(A_POPUP));
    key(&mut state, &keyboard, 38, false);
    assert_eq!(state.take_log(), ["key a-popup 38 Released"]);

    // The client destroys the popup. Its grab notices on the next event and ends itself.
    ended.store(true, Ordering::SeqCst);
    keyboard.set_focus(&mut state, Some(B), serial());
    assert_eq!(popup_unset.load(Ordering::SeqCst), 1);
    assert!(!keyboard.is_grabbed());
    assert_eq!(keyboard.current_focus(), Some(B));
}

#[test]
fn interactions_come_before_an_input_method_whichever_was_first() {
    for input_method_first in [true, false] {
        let (mut state, _pointer, keyboard) = setup();
        keyboard.set_focus(&mut state, Some(A), serial());
        let to_input_method = Arc::new(Mutex::new(Vec::new()));
        let to_mode = Arc::new(Mutex::new(Vec::new()));
        let filter = Filter {
            taken: to_input_method.clone(),
        };
        let mode = Modal {
            taken: to_mode.clone(),
        };

        if input_method_first {
            keyboard.set_grab(&mut state, filter, serial());
            keyboard.set_grab(&mut state, mode, serial());
        } else {
            keyboard.set_grab(&mut state, mode, serial());
            keyboard.set_grab(&mut state, filter, serial());
        }
        assert!(keyboard.has_grab_of::<Filter>());
        assert!(keyboard.has_grab_of::<Modal>());

        // The compositor's mode gets the keys, the input method does not see them.
        key(&mut state, &keyboard, 38, true);
        assert_eq!(*to_mode.lock().unwrap(), [38]);
        assert!(to_input_method.lock().unwrap().is_empty());

        // Once the mode is over, the input method is still there.
        keyboard.unset_grab(&mut state);
        key(&mut state, &keyboard, 38, false);
        assert_eq!(*to_mode.lock().unwrap(), [38]);
        assert_eq!(*to_input_method.lock().unwrap(), [38]);
    }
}

#[test]
fn a_grab_replaces_the_one_of_its_own_type() {
    let (mut state, _pointer, keyboard) = setup();

    let first_serial = serial();
    let (first, _, first_unset) = FocusLock::new(A_POPUP);
    keyboard.set_grab(&mut state, first, first_serial);

    // A nested popup: the same kind of grab again, with a new serial.
    let second_serial = serial();
    let (second, _, second_unset) = FocusLock::new(A_POPUP);
    keyboard.set_grab(&mut state, second, second_serial);

    assert_eq!(first_unset.load(Ordering::SeqCst), 1);
    assert_eq!(second_unset.load(Ordering::SeqCst), 0);
    assert!(!keyboard.has_grab(first_serial));
    assert!(keyboard.has_grab(second_serial));
    assert_eq!(keyboard.with_grab(|serial, _| serial), Some(second_serial));
}

#[test]
fn a_click_alone_behaves_as_it_always_did() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 50.0);
    assert!(!pointer.is_grabbed());

    // The first button that goes down starts the implicit grab, further ones do not.
    let first = button(&mut state, &pointer, 0x110, true);
    let second = button(&mut state, &pointer, 0x111, true);
    assert!(pointer.is_grabbed());
    assert!(pointer.has_grab(first));
    assert!(!pointer.has_grab(second));
    assert_eq!(
        pointer.grab_start_data().unwrap().focus.map(|(target, _)| target),
        Some(A)
    );

    // It keeps the focus where the press was for as long as a button is held.
    state.take_log();
    motion(&mut state, &pointer, 250.0);
    button(&mut state, &pointer, 0x110, false);
    assert_eq!(state.take_log(), ["motion a", "button a 272 Released"]);
    assert!(pointer.has_grab(first));
    button(&mut state, &pointer, 0x111, false);
    assert!(!pointer.is_grabbed());

    motion(&mut state, &pointer, 250.0);
    assert_eq!(pointer.current_focus(), Some(B));
}

#[test]
fn a_press_inside_a_popup_starts_an_implicit_grab() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 150.0);
    let popup_serial = serial();
    let (popup, popup_unset) = OwnerEvents::new(1);
    pointer.set_grab(&mut state, popup, popup_serial, Focus::Keep);
    state.take_log();

    // Press on the popup, say on a slider, and drag off it, over another client.
    let press = button(&mut state, &pointer, 0x110, true);
    assert!(pointer.has_grab(press));
    assert!(pointer.has_grab(popup_serial));
    let start_data = pointer.grab_start_data_for(press).unwrap();
    assert_eq!(start_data.focus.map(|(target, _)| target), Some(A_POPUP));
    assert_eq!(start_data.button, 0x110);
    // The press is also what the single-grab accessors report, as code that goes on to start a
    // move or a drag from `has_grab(serial)` and `grab_start_data()` expects.
    assert_eq!(pointer.with_grab(|serial, _| serial), Some(press));

    motion(&mut state, &pointer, 250.0);
    button(&mut state, &pointer, 0x110, false);
    assert_eq!(
        state.take_log(),
        [
            "button a-popup 272 Pressed",
            "motion a-popup",
            "button a-popup 272 Released"
        ]
    );

    // The release ended the implicit grab, and only that one.
    assert!(!pointer.has_grab(press));
    assert!(pointer.has_grab(popup_serial));
    assert_eq!(popup_unset.load(Ordering::SeqCst), 0);
    motion(&mut state, &pointer, 250.0);
    assert_eq!(pointer.current_focus(), None);
    motion(&mut state, &pointer, 50.0);
    assert_eq!(pointer.current_focus(), Some(A));
}

#[test]
fn a_drag_can_start_inside_a_popup_and_leave_it() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 150.0);
    let popup_serial = serial();
    let (popup, popup_unset) = OwnerEvents::new(1);
    pointer.set_grab(&mut state, popup, popup_serial, Focus::Keep);

    // `wl_data_device.start_drag` with the serial of a press inside the popup.
    let press = button(&mut state, &pointer, 0x110, true);
    assert!(pointer.has_grab(press));
    let over = Arc::new(Mutex::new(Vec::new()));
    let drag = Drag {
        start_data: pointer.grab_start_data_for(press).unwrap(),
        over: over.clone(),
    };
    state.take_log();
    pointer.set_grab(&mut state, drag, press, Focus::Clear);
    assert_eq!(state.take_log(), ["leave a-popup"]);
    assert!(pointer.has_grab_of::<Drag>());
    assert!(!pointer.has_grab_of::<super::pointer::ClickGrab<State>>());
    assert_eq!(popup_unset.load(Ordering::SeqCst), 0);

    // The drag sees what is really under the pointer, other clients included, while nothing has
    // the pointer focus.
    motion(&mut state, &pointer, 50.0);
    motion(&mut state, &pointer, 250.0);
    assert_eq!(*over.lock().unwrap(), [Some("a"), Some("b")]);
    assert_eq!(pointer.current_focus(), None);

    // Dropping ends the drag. The popup is still open and has its grab back.
    button(&mut state, &pointer, 0x110, false);
    assert!(!pointer.has_grab_of::<Drag>());
    assert!(pointer.has_grab(popup_serial));
    assert_eq!(pointer.current_focus(), None);
    motion(&mut state, &pointer, 150.0);
    assert_eq!(pointer.current_focus(), Some(A_POPUP));
    motion(&mut state, &pointer, 250.0);
    assert_eq!(pointer.current_focus(), None);
}

#[test]
fn a_second_button_during_a_drag_is_not_a_new_click() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 50.0);
    let press = button(&mut state, &pointer, 0x110, true);
    let drag = Drag {
        start_data: pointer.grab_start_data().unwrap(),
        over: Default::default(),
    };
    pointer.set_grab(&mut state, drag, press, Focus::Clear);

    let other = button(&mut state, &pointer, 0x111, true);
    assert!(!pointer.has_grab(other));
    assert!(!pointer.has_grab_of::<super::pointer::ClickGrab<State>>());
    button(&mut state, &pointer, 0x111, false);
    assert!(pointer.has_grab_of::<Drag>());
    button(&mut state, &pointer, 0x110, false);
    assert!(!pointer.is_grabbed());
}

#[test]
fn a_new_interaction_replaces_the_one_in_progress() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 150.0);
    let popup_serial = serial();
    let (popup, popup_unset) = OwnerEvents::new(1);
    pointer.set_grab(&mut state, popup, popup_serial, Focus::Keep);

    // Something that does not come out of the popup's own input, say a compositor gesture.
    let gesture_serial = serial();
    let gesture = Drag {
        start_data: PointerGrabStartData {
            focus: None,
            button: 0,
            location: (0.0, 0.0).into(),
        },
        over: Default::default(),
    };
    pointer.set_grab(&mut state, gesture, gesture_serial, Focus::Clear);

    // As ever, that ends what was there.
    assert_eq!(popup_unset.load(Ordering::SeqCst), 1);
    assert!(!pointer.has_grab(popup_serial));
    assert_eq!(pointer.with_grab(|serial, _| serial), Some(gesture_serial));
}

#[test]
fn a_press_that_no_client_was_sent_starts_no_implicit_grab() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 50.0);

    // A compositor gesture that is installed ahead of the press that triggers it.
    let gesture_serial = serial();
    let gesture = Drag {
        start_data: PointerGrabStartData {
            focus: None,
            button: 0x110,
            location: (50.0, 0.0).into(),
        },
        over: Default::default(),
    };
    pointer.set_grab(&mut state, gesture, gesture_serial, Focus::Clear);
    state.take_log();

    let press = button(&mut state, &pointer, 0x110, true);
    assert!(!pointer.has_grab(press));
    assert!(!pointer.has_grab_of::<super::pointer::ClickGrab<State>>());
    assert_eq!(pointer.with_grab(|serial, _| serial), Some(gesture_serial));
    assert!(pointer.with_grab(|_, grab| grab.is::<Drag>()).unwrap());

    button(&mut state, &pointer, 0x110, false);
    assert!(!pointer.is_grabbed());
    assert_eq!(pointer.current_focus(), Some(A));
    assert_eq!(state.take_log(), ["enter a"]);
}

#[test]
fn a_press_outside_a_popup_dismisses_it_and_goes_through() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 150.0);
    let (popup, popup_unset) = OwnerEvents::new(1);
    pointer.set_grab(&mut state, popup, serial(), Focus::Keep);
    motion(&mut state, &pointer, 250.0);
    assert_eq!(pointer.current_focus(), None);
    state.take_log();

    let press = button(&mut state, &pointer, 0x110, true);
    assert_eq!(state.take_log(), ["enter b", "button b 272 Pressed"]);
    assert_eq!(popup_unset.load(Ordering::SeqCst), 1);
    assert!(!pointer.has_grab_of::<OwnerEvents>());
    assert!(pointer.has_grab(press));
    assert_eq!(
        pointer.grab_start_data().unwrap().focus.map(|(target, _)| target),
        Some(B)
    );
}

#[test]
fn cancelling_the_grabs_restores_the_focus() {
    let (mut state, pointer, _keyboard) = setup();
    motion(&mut state, &pointer, 50.0);
    let press = button(&mut state, &pointer, 0x110, true);
    let drag = Drag {
        start_data: pointer.grab_start_data().unwrap(),
        over: Default::default(),
    };
    pointer.set_grab(&mut state, drag, press, Focus::Clear);
    motion(&mut state, &pointer, 250.0);
    assert_eq!(pointer.current_focus(), None);

    pointer.unset_grab(&mut state, serial(), InputTime::now());
    assert!(!pointer.is_grabbed());
    assert_eq!(pointer.current_focus(), Some(B));
}
