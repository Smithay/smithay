//! Utilities to track object's life cycle

use std::sync::atomic::{AtomicBool, Ordering};

/// Util to track wayland object's life time
#[derive(Debug)]
pub struct AliveTracker {
    is_alive: AtomicBool,
}

impl Default for AliveTracker {
    fn default() -> Self {
        Self {
            is_alive: AtomicBool::new(true),
        }
    }
}

impl AliveTracker {
    /// Create new alive tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Notify the tracker that object is dead
    pub fn destroy_notify(&self) {
        self.is_alive.store(false, Ordering::Release);
    }

    /// Check if object is alive
    pub fn alive(&self) -> bool {
        self.is_alive.load(Ordering::Acquire)
    }
}

/// Trait that is implemented on wayland objects tracked by Smithay
pub trait IsAlive {
    /// Check if object is alive
    fn alive(&self) -> bool;
}
