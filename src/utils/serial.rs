use std::sync::atomic::{AtomicU32, Ordering};

/// A global [`SerialCounter`] for use in your compositor.
///
/// Is is also used internally by some parts of Smithay.
pub static SERIAL_COUNTER: SerialCounter = SerialCounter {
    serial: AtomicU32::new(1),
};

/// A serial type, whose comparison takes into account the wrapping-around behavior of the
/// underlying counter.
#[derive(Debug, Copy, Clone)]
pub struct Serial(pub(crate) u32);

impl PartialEq for Serial {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Serial {}

impl PartialOrd for Serial {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let distance = if self.0 > other.0 {
            self.0 - other.0
        } else {
            other.0 - self.0
        };
        if distance < u32::MAX / 2 {
            self.0.partial_cmp(&other.0)
        } else {
            // wrap-around occurred, invert comparison
            other.0.partial_cmp(&self.0)
        }
    }
}

impl From<u32> for Serial {
    fn from(n: u32) -> Self {
        Serial(n)
    }
}

impl From<Serial> for u32 {
    fn from(serial: Serial) -> u32 {
        serial.0
    }
}

impl Serial {
    /// Checks if a serial was generated after or is equal to another given serial
    pub fn is_no_older_than(&self, other: &Serial) -> bool {
        other <= self
    }
}

/// A counter for generating serials, for use in the client protocol
///
/// A global instance of this counter is available as the `SERIAL_COUNTER`
/// static. It is recommended to only use this global counter to ensure the
/// uniqueness of serials.
///
/// The counter will wrap around on overflow, ensuring it can run for as long
/// as needed.
#[derive(Debug)]
pub struct SerialCounter {
    serial: AtomicU32,
}

impl SerialCounter {
    /// Retrieve the next serial from the counter
    pub fn next_serial(&self) -> Serial {
        let _ = self
            .serial
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::SeqCst);
        Serial(self.serial.fetch_add(1, Ordering::AcqRel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_serial_counter(initial_value: u32) -> SerialCounter {
        SerialCounter {
            serial: AtomicU32::new(initial_value),
        }
    }

    #[test]
    #[allow(clippy::eq_op)]
    fn serial_equals_self() {
        let counter = create_serial_counter(0);
        let serial = counter.next_serial();
        assert!(serial == serial);
    }

    #[test]
    fn consecutive_serials() {
        let counter = create_serial_counter(0);
        let serial1 = counter.next_serial();
        let serial2 = counter.next_serial();
        assert!(serial1 < serial2);
    }

    #[test]
    fn non_consecutive_serials() {
        let skip_serials = 147;

        let counter = create_serial_counter(0);
        let serial1 = counter.next_serial();
        for _ in 0..skip_serials {
            let _ = counter.next_serial();
        }
        let serial2 = counter.next_serial();
        assert!(serial1 < serial2);
    }

    #[test]
    fn serial_wrap_around() {
        let counter = create_serial_counter(u32::MAX);
        let serial1 = counter.next_serial();
        let serial2 = counter.next_serial();

        assert!(serial1 == u32::MAX.into());
        assert!(serial2 == 1.into());

        assert!(serial1 < serial2);
    }
}
