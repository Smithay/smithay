use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Deref;
use std::rc::Rc;

use wayland_server::protocol::wl_touch::WlTouch;
use wayland_server::{Filter, Main};

use crate::backend::input::TouchSlot;
use crate::utils::{Logical, Point};
use crate::wayland::seat::wl_surface::WlSurface;
use crate::wayland::Serial;

/// An handle to a touch handler.
///
/// It can be cloned and all clones manipulate the same internal state.
///
/// This handle gives you access to an interface to send touch events to your
/// clients.
#[derive(Debug, Clone)]
pub struct TouchHandle {
    inner: Rc<RefCell<TouchInternal>>,
}

impl TouchHandle {
    pub(crate) fn new() -> Self {
        Self {
            inner: Default::default(),
        }
    }

    /// Register a new touch handle to this handler
    ///
    /// This should be done first, before anything else is done with this touch handle.
    pub(crate) fn new_touch(&self, touch: WlTouch) {
        self.inner.borrow_mut().known_handles.push(touch);
    }

    /// Notify clients about new touch points.
    pub fn down(
        &mut self,
        serial: Serial,
        time: u32,
        surface: &WlSurface,
        surface_offset: Point<i32, Logical>,
        slot: TouchSlot,
        location: Point<f64, Logical>,
    ) {
        self.inner
            .borrow_mut()
            .down(serial, time, surface, surface_offset, slot, location);
    }

    /// Notify clients about touch point removal.
    pub fn up(&self, serial: Serial, time: u32, slot: TouchSlot) {
        self.inner.borrow_mut().up(serial, time, slot);
    }

    /// Notify clients about touch motion.
    pub fn motion(&self, time: u32, slot: TouchSlot, location: Point<f64, Logical>) {
        self.inner.borrow_mut().motion(time, slot, location);
    }

    /// Notify clients about touch shape changes.
    pub fn shape(&self, slot: TouchSlot, major: f64, minor: f64) {
        self.inner.borrow_mut().shape(slot, major, minor);
    }

    /// Notify clients about touch shape orientation.
    pub fn orientation(&self, slot: TouchSlot, orientation: f64) {
        self.inner.borrow_mut().orientation(slot, orientation);
    }

    /// Notify clients about touch cancellation.
    ///
    /// This should be sent by the compositor when the currently active touch
    /// slot was recognized as a gesture.
    pub fn cancel(&self) {
        self.inner.borrow_mut().cancel();
    }
}

/// Touch-slot focused Wayland client state.
#[derive(Default, Debug)]
struct TouchFocus {
    surface_offset: Point<f64, Logical>,
    handles: Vec<WlTouch>,
}

#[derive(Default, Debug)]
struct TouchInternal {
    known_handles: Vec<WlTouch>,
    focus: HashMap<TouchSlot, TouchFocus>,
}

impl TouchInternal {
    fn down(
        &mut self,
        serial: Serial,
        time: u32,
        surface: &WlSurface,
        surface_offset: Point<i32, Logical>,
        slot: TouchSlot,
        location: Point<f64, Logical>,
    ) {
        // Update focused client state.
        let focus = self.focus.entry(slot).or_default();
        focus.surface_offset = surface_offset.to_f64();
        focus.handles.clear();

        // Select all WlTouch instances associated to the active WlSurface.
        for handle in &self.known_handles {
            if handle.as_ref().same_client_as(surface.as_ref()) {
                focus.handles.push(handle.clone());
            }
        }

        let (x, y) = (location - focus.surface_offset).into();
        self.with_focused_handles(slot, |handle| {
            handle.down(serial.into(), time, surface, slot.into(), x, y)
        });
    }

    fn up(&self, serial: Serial, time: u32, slot: TouchSlot) {
        self.with_focused_handles(slot, |handle| handle.up(serial.into(), time, slot.into()));
    }

    fn motion(&self, time: u32, slot: TouchSlot, location: Point<f64, Logical>) {
        let focus = match self.focus.get(&slot) {
            Some(slot) => slot,
            None => return,
        };

        let (x, y) = (location - focus.surface_offset).into();
        self.with_focused_handles(slot, |handle| handle.motion(time, slot.into(), x, y));
    }

    fn shape(&self, slot: TouchSlot, major: f64, minor: f64) {
        self.with_focused_handles(slot, |handle| {
            if handle.as_ref().version() >= 6 {
                handle.shape(slot.into(), major, minor);
            }
        });
    }

    fn orientation(&self, slot: TouchSlot, orientation: f64) {
        self.with_focused_handles(slot, |handle| {
            if handle.as_ref().version() >= 6 {
                handle.orientation(slot.into(), orientation);
            }
        });
    }

    // TODO: In theory doesn't need to be sent for WlTouch that isn't in the focus hashmap?
    fn cancel(&self) {
        for handle in &self.known_handles {
            handle.cancel();
        }
    }

    // TODO: Document this also sends frame every time.
    #[inline]
    fn with_focused_handles<F>(&self, slot: TouchSlot, mut f: F)
    where
        F: FnMut(&WlTouch),
    {
        if let Some(focus) = self.focus.get(&slot) {
            for handle in &focus.handles {
                f(handle);
                handle.frame();
            }
        }
    }
}

pub(crate) fn implement_touch(touch: Main<WlTouch>, handle: Option<&TouchHandle>) -> WlTouch {
    // The sole `Release` request is already handled by our destructor.
    touch.quick_assign(|_touch, _request, _data| {});

    // Remove from touch handles on destroy.
    if let Some(handle) = handle {
        let inner = handle.inner.clone();
        touch.assign_destructor(Filter::new(move |touch: WlTouch, _, _| {
            inner
                .borrow_mut()
                .known_handles
                .retain(|t| !t.as_ref().equals(touch.as_ref()))
        }));
    }

    touch.deref().clone()
}
