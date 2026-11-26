//! Compositor-side key repeat helper
//!
//! ```no_run
//! # use smithay::backend::input::{KeyState, InputTime};
//! # use smithay::input::{SeatHandler, keyboard::{FilterResult, KeyboardHandle, Keycode, repeat::KeyRepeat}};
//! # fn example<D: SeatHandler + 'static>(
//! #     key_repeat: &mut KeyRepeat<D>,
//! #     keyboard: &KeyboardHandle<D>,
//! #     keycode: Keycode,
//! #     state: KeyState,
//! #     time: InputTime,
//! # ) {
//!
//! // For every key event coming from the input backend (after passing it to `KeyboardHandle::input`):
//! let kbd = keyboard.clone();
//! key_repeat.key_event(keyboard, keycode, state, time, move |data, keycode, time| {
//!     let serial = smithay::utils::SERIAL_COUNTER.next_serial();
//!     kbd.input_repeat::<(), _>(data, keycode, serial, time, |_, _, _| FilterResult::Forward);
//! });
//! # }
//! ```

use std::{
    fmt,
    time::{Duration, Instant},
};

use calloop::{
    LoopHandle, RegistrationToken,
    timer::{TimeoutAction, Timer},
};

use super::{KeyboardHandle, Keycode};
use crate::backend::input::{InputTime, KeyState};
use crate::input::SeatHandler;

/// Drives compositor-side key repeat with a calloop timer.
///
/// The repeat timer is removed on Drop.
pub struct KeyRepeat<D: 'static> {
    loop_handle: LoopHandle<'static, D>,
    /// The timer and the key it repeats
    timer: Option<(RegistrationToken, Keycode)>,
}

impl<D> fmt::Debug for KeyRepeat<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyRepeat").field("timer", &self.timer).finish()
    }
}

impl<D: SeatHandler + 'static> KeyRepeat<D> {
    /// Create a new key repeat helper
    pub fn new(loop_handle: LoopHandle<'static, D>) -> Self {
        Self {
            loop_handle,
            timer: None,
        }
    }

    /// Start or stop key repeat for a key event
    ///
    /// Should be called for every key event coming from the input backend.
    /// Does nothing unless [`KeyboardHandle::set_compositor_side_repeat`] is enabled.
    ///
    /// `on_repeat` is expected to call [`KeyboardHandle::input_repeat`].
    pub fn key_event(
        &mut self,
        keyboard: &KeyboardHandle<D>,
        keycode: Keycode,
        state: KeyState,
        time: InputTime,
        mut on_repeat: impl FnMut(&mut D, Keycode, InputTime) + 'static,
    ) {
        if state == KeyState::Released {
            if let Some((_, repeating)) = self.timer
                && repeating == keycode
            {
                self.stop();
            }
            return;
        }

        if !keyboard.is_compositor_side_repeat_on() || !keyboard.key_repeats(keycode) {
            return;
        }

        self.stop();

        let (rate, delay) = keyboard.repeat_info();
        if rate <= 0 {
            return;
        }

        let delay = Duration::from_millis(delay.max(0) as u64);
        let interval = (Duration::from_secs(1) / rate as u32).max(Duration::from_millis(1));

        let mut deadline = Instant::now() + delay;
        let mut time = InputTime::from_micros(time.micros() + delay.as_micros() as u64);
        let token = self
            .loop_handle
            .insert_source(Timer::from_deadline(deadline), move |_, _, data| {
                on_repeat(data, keycode, time);
                // I'm doing this deadline thingy because I'm afraid a normal callop timer might accumulate latency over time
                deadline += interval;
                time = InputTime::from_micros(time.micros() + interval.as_micros() as u64);
                TimeoutAction::ToInstant(deadline)
            })
            .expect("Failed to insert key repeat timer");

        self.timer = Some((token, keycode));
    }

    /// Stop the ongoing key repeat
    pub fn stop(&mut self) {
        if let Some((token, _)) = self.timer.take() {
            self.loop_handle.remove(token);
        }
    }

    /// Whether a key is currently being repeated
    pub fn is_repeating(&self) -> bool {
        self.timer.is_some()
    }
}

impl<D: 'static> Drop for KeyRepeat<D> {
    fn drop(&mut self) {
        if let Some((token, _)) = self.timer.take() {
            self.loop_handle.remove(token);
        }
    }
}
