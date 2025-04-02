//! Utilities to track object's life cycle

// All our AliveTracker usage is internal and only
// used, when the wayland_frontend feature is enabled.
// So we need to silence some warnings in other cases.
#![allow(dead_code)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Util to track wayland object's life time
#[derive(Debug, Clone)]
#[must_use]
pub struct AliveTracker {
    is_alive: Arc<AtomicBool>,
}

impl AliveTracker {
    /// Create an already notified alive tracker
    pub fn notified() -> Self {
        Self {
            is_alive: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Default for AliveTracker {
    fn default() -> Self {
        Self {
            is_alive: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl AliveTracker {
    /// Notify the tracker that object is dead
    pub fn destroy_notify(&self) {
        self.is_alive.store(false, Ordering::Release);
    }

    /// Check if object is alive
    #[inline]
    pub fn alive(&self) -> bool {
        self.is_alive.load(Ordering::Acquire)
    }
}

/// Trait that is implemented on wayland objects tracked by Smithay
pub trait IsAlive {
    /// Check if object is alive
    fn alive(&self) -> bool;
}

impl<T: IsAlive> IsAlive for &T {
    #[inline]
    fn alive(&self) -> bool {
        IsAlive::alive(*self)
    }
}
