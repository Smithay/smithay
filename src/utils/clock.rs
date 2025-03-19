use rustix::time::{ClockId, Timespec};
use std::{cmp::Ordering, marker::PhantomData, ops::Add, time::Duration};

/// Marker for clock source that never returns a negative [`Time`]
pub trait NonNegativeClockSource: ClockSource {}

/// Monotonic clock
#[derive(Debug)]
pub struct Monotonic;

impl ClockSource for Monotonic {
    const ID: ClockId = ClockId::Monotonic;
}

impl NonNegativeClockSource for Monotonic {}

/// Realtime clock
#[derive(Debug)]
pub struct Realtime;

impl ClockSource for Realtime {
    const ID: ClockId = ClockId::Realtime;
}

/// Id for a clock according to unix clockid_t
pub trait ClockSource {
    /// Gets the id of the clock source
    const ID: ClockId;
}

/// Defines a clock with a specific kind
#[derive(Debug)]
pub struct Clock<Kind: ClockSource> {
    _kind: PhantomData<Kind>,
}

impl<Kind: ClockSource> Clock<Kind> {
    /// Initialize a new clock
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Clock { _kind: PhantomData }
    }

    /// Returns the current time
    pub fn now(&self) -> Time<Kind> {
        rustix::time::clock_gettime(Kind::ID).into()
    }

    /// Gets the id of the clock
    pub fn id(&self) -> ClockId {
        Kind::ID
    }
}

/// A point in time for a clock with a specific kind
pub struct Time<Kind> {
    tp: Timespec,
    _kind: PhantomData<Kind>,
}

impl<Kind> Time<Kind> {
    /// Gets the duration from self until a later time
    pub fn elapsed(elapsed: &Time<Kind>, later: Time<Kind>) -> Duration {
        saturating_sub_timespec(later.tp, elapsed.tp).unwrap_or(Duration::ZERO)
    }
}

impl Time<Monotonic> {
    /// Returns the time in milliseconds
    ///
    /// This should match timestamps from libinput:
    /// <https://wayland.freedesktop.org/libinput/doc/latest/timestamps.html>
    pub fn as_millis(&self) -> u32 {
        debug_assert!(self.tp.tv_sec >= 0);
        debug_assert!(self.tp.tv_nsec >= 0);

        // The monotonic clock does not fit as milliseconds in 32-bit after ~50 days of uptime. We
        // do a modulo conversion which should match what happens in libinput.
        (self.as_micros() / 1000) as u32
    }

    /// Returns the time in microseconds
    ///
    /// This should match timestamps from libinput:
    /// <https://wayland.freedesktop.org/libinput/doc/latest/timestamps.html>
    pub fn as_micros(&self) -> u64 {
        // Assume monotonic clock (but not realitime) fits as microseconds in 64-bit
        debug_assert!(self.tp.tv_sec >= 0);
        debug_assert!(self.tp.tv_nsec >= 0);
        self.tp.tv_sec as u64 * 1000000 + self.tp.tv_nsec as u64 / 1000
    }
}

impl<Kind> Clone for Time<Kind> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<Kind> Copy for Time<Kind> {}

impl<Kind: NonNegativeClockSource> From<Time<Kind>> for Duration {
    #[inline]
    fn from(time: Time<Kind>) -> Self {
        debug_assert!(time.tp.tv_sec >= 0);
        debug_assert!(time.tp.tv_nsec >= 0);
        Duration::new(time.tp.tv_sec as u64, time.tp.tv_nsec as u32)
    }
}

impl<Kind> std::fmt::Debug for Time<Kind> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Time")
            .field("tp", &self.tp)
            .field("kind", &self._kind)
            .finish()
    }
}

impl<Kind> PartialEq for Time<Kind> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.tp == other.tp && self._kind == other._kind
    }
}

impl<Kind> Eq for Time<Kind> {}

impl<Kind> PartialOrd for Time<Kind> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<Kind> Ord for Time<Kind> {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let tv_sec = self.tp.tv_sec.cmp(&other.tp.tv_sec);

        if tv_sec == Ordering::Equal {
            self.tp.tv_nsec.cmp(&other.tp.tv_nsec)
        } else {
            tv_sec
        }
    }
}

impl<Kind: NonNegativeClockSource> From<Duration> for Time<Kind> {
    #[inline]
    fn from(tp: Duration) -> Self {
        let tp = Timespec {
            tv_sec: tp.as_secs() as rustix::time::Secs,
            tv_nsec: tp.subsec_nanos() as rustix::time::Nsecs,
        };
        Time {
            tp,
            _kind: PhantomData,
        }
    }
}

impl<Kind> From<Timespec> for Time<Kind> {
    #[inline]
    fn from(tp: Timespec) -> Self {
        Time {
            tp,
            _kind: PhantomData,
        }
    }
}

impl<Kind> From<Time<Kind>> for Timespec {
    fn from(value: Time<Kind>) -> Self {
        value.tp
    }
}

const NANOS_PER_SEC: rustix::time::Nsecs = 1_000_000_000;

fn saturating_sub_timespec(lhs: Timespec, rhs: Timespec) -> Option<Duration> {
    debug_assert!(!lhs.tv_sec.is_negative());
    debug_assert!(!lhs.tv_nsec.is_negative());
    debug_assert!(!rhs.tv_sec.is_negative());
    debug_assert!(!rhs.tv_nsec.is_negative());
    debug_assert!(lhs.tv_nsec < NANOS_PER_SEC);
    debug_assert!(rhs.tv_nsec < NANOS_PER_SEC);

    let lhs_tv_sec = lhs.tv_sec as u64;
    let lhs_tv_nsec = lhs.tv_nsec as u64;
    let rhs_tv_sec = rhs.tv_sec as u64;
    let rhs_tv_nsec = rhs.tv_nsec as u64;

    if let Some(mut secs) = lhs_tv_sec.checked_sub(rhs_tv_sec) {
        let nanos = if lhs_tv_nsec >= rhs_tv_nsec {
            lhs_tv_nsec - rhs_tv_nsec
        } else if let Some(sub_secs) = secs.checked_sub(1) {
            secs = sub_secs;
            lhs_tv_nsec + (NANOS_PER_SEC as u64) - rhs_tv_nsec
        } else {
            return None;
        };
        debug_assert!(nanos < (NANOS_PER_SEC as u64));
        Some(Duration::new(secs, nanos as u32))
    } else {
        None
    }
}

impl<T, Kind> Add<T> for Time<Kind>
where
    T: Into<Time<Kind>>,
{
    type Output = Time<Kind>;

    fn add(self, rhs: T) -> Self::Output {
        let rhs = rhs.into();
        let tv_nsec = (self.tp.tv_nsec + rhs.tp.tv_nsec) % NANOS_PER_SEC;
        let tv_sec = (self.tp.tv_sec + rhs.tp.tv_sec) + ((self.tp.tv_nsec + rhs.tp.tv_nsec) / NANOS_PER_SEC);
        Self::from(Timespec { tv_sec, tv_nsec })
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use crate::utils::{Clock, Monotonic, Time};

    #[test]
    fn monotonic() {
        let clock_source: Clock<Monotonic> = Clock::new();
        let now = clock_source.now();
        let zero = Time::<Monotonic>::from(Duration::ZERO);
        assert_eq!(Time::<Monotonic>::elapsed(&zero, now), now.into());
    }
}
