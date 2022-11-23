use std::{cmp::Ordering, marker::PhantomData, mem::MaybeUninit, time::Duration};

/// Marker for clock source that never returns a negative [`Time`]
pub trait NonNegativeClockSource: ClockSource {}

/// Monotonic clock
#[derive(Debug)]
pub struct Monotonic;

impl ClockSource for Monotonic {
    fn id() -> libc::clockid_t {
        libc::CLOCK_MONOTONIC
    }
}

impl NonNegativeClockSource for Monotonic {}

/// Clock based on boottime
#[derive(Debug)]
pub struct Boottime;

impl ClockSource for Boottime {
    fn id() -> libc::clockid_t {
        libc::CLOCK_BOOTTIME
    }
}

impl NonNegativeClockSource for Boottime {}

/// Realtime clock
#[derive(Debug)]
pub struct Realtime;

impl ClockSource for Realtime {
    fn id() -> libc::clockid_t {
        libc::CLOCK_REALTIME
    }
}

/// Id for a clock according to unix clockid_t
pub trait ClockSource {
    /// Gets the id of the clock source
    fn id() -> libc::clockid_t;
}

/// Defines a clock with a specific kind
#[derive(Debug)]
pub struct Clock<Kind> {
    clk_id: libc::clockid_t,
    _kind: PhantomData<Kind>,
}

impl<Kind: ClockSource> Clock<Kind> {
    /// Initialize a new clock
    pub fn new() -> std::io::Result<Self> {
        let clk_id = Kind::id();
        clock_get_time(clk_id)?;
        Ok(Clock {
            clk_id,
            _kind: PhantomData,
        })
    }

    /// Returns the current time
    pub fn now(&self) -> Time<Kind> {
        clock_get_time(self.clk_id)
            .expect("failed to get clock time")
            .into()
    }

    /// Gets the id of the clock
    pub fn id(&self) -> libc::clockid_t {
        Kind::id()
    }
}

/// A point in time for a clock with a specific kind
pub struct Time<Kind> {
    tp: libc::timespec,
    _kind: PhantomData<Kind>,
}

impl<Kind> Time<Kind> {
    /// Gets the duration between self and a later time
    pub fn duration_since(&self, later: Time<Kind>) -> Duration {
        saturating_sub_timespec(later.tp, self.tp).unwrap_or(Duration::ZERO)
    }
}

impl<Kind> Clone for Time<Kind> {
    fn clone(&self) -> Self {
        Self {
            tp: self.tp,
            _kind: self._kind,
        }
    }
}

impl<Kind> Copy for Time<Kind> {}

impl<Kind: NonNegativeClockSource> From<Time<Kind>> for Duration {
    fn from(time: Time<Kind>) -> Self {
        debug_assert!(time.tp.tv_sec > 0);
        debug_assert!(time.tp.tv_nsec > 0);
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
    fn eq(&self, other: &Self) -> bool {
        self.tp == other.tp && self._kind == other._kind
    }
}

impl<Kind> Eq for Time<Kind> {}

impl<Kind> PartialOrd for Time<Kind> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<Kind> Ord for Time<Kind> {
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
    fn from(tp: Duration) -> Self {
        let tp = libc::timespec {
            tv_sec: tp.as_secs() as libc::time_t,
            #[cfg(all(target_arch = "x86_64", target_pointer_width = "32"))]
            tv_nsec: tp.subsec_nanos() as i64,
            #[cfg(not(all(target_arch = "x86_64", target_pointer_width = "32")))]
            tv_nsec: tp.subsec_nanos() as std::os::raw::c_long,
        };
        Time {
            tp,
            _kind: PhantomData,
        }
    }
}

impl<Kind> From<libc::timespec> for Time<Kind> {
    fn from(tp: libc::timespec) -> Self {
        Time {
            tp,
            _kind: PhantomData,
        }
    }
}

#[cfg(all(target_arch = "x86_64", target_pointer_width = "32"))]
const NANOS_PER_SEC: i64 = 1_000_000_000;

#[cfg(not(all(target_arch = "x86_64", target_pointer_width = "32")))]
const NANOS_PER_SEC: std::os::raw::c_long = 1_000_000_000;

fn saturating_sub_timespec(lhs: libc::timespec, rhs: libc::timespec) -> Option<Duration> {
    if let Some(mut secs) = lhs.tv_sec.checked_sub(rhs.tv_sec) {
        let nanos = if lhs.tv_nsec >= rhs.tv_nsec {
            lhs.tv_nsec - rhs.tv_nsec
        } else if let Some(sub_secs) = secs.checked_sub(1) {
            secs = sub_secs;
            lhs.tv_nsec + NANOS_PER_SEC - rhs.tv_nsec
        } else {
            return None;
        };
        debug_assert!(nanos < NANOS_PER_SEC);
        Some(Duration::new(secs as u64, nanos as u32))
    } else {
        None
    }
}

fn clock_get_time(clk_id: libc::clockid_t) -> Result<libc::timespec, std::io::Error> {
    let mut tp = MaybeUninit::zeroed();
    unsafe {
        let res = libc::clock_gettime(clk_id, tp.as_mut_ptr());

        if res < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(tp.assume_init())
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use crate::utils::{Boottime, Clock, Monotonic, Time};

    #[test]
    fn monotonic() {
        let clock_source: Clock<Monotonic> = Clock::new().unwrap();
        let now = clock_source.now();
        let zero = Time::<Monotonic>::from(Duration::ZERO);
        assert_eq!(zero.duration_since(now), now.into());
    }

    #[test]
    fn boottime() {
        let clock_source: Clock<Boottime> = Clock::new().unwrap();
        let now = clock_source.now();
        let zero = Time::<Boottime>::from(Duration::ZERO);
        assert_eq!(zero.duration_since(now), now.into());
    }
}
