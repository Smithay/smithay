use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::ClientId,
    protocol::wl_touch::{self, WlTouch},
    Dispatch, DisplayHandle, Resource,
};

use super::{SeatHandler, SeatState};
use crate::backend::input::TouchSlot;
use crate::utils::Serial;
use crate::utils::{Logical, Point};
use crate::wayland::seat::wl_surface::WlSurface;

/// An handle to a touch handler.
///
/// It can be cloned and all clones manipulate the same internal state.
///
/// This handle gives you access to an interface to send touch events to your
/// clients.
#[derive(Debug, Clone)]
pub struct TouchHandle {
    inner: Arc<Mutex<TouchInternal>>,
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
        self.inner.lock().unwrap().known_handles.push(touch);
    }

    /// Notify clients about new touch points.
    ///
    /// The `position` parameter is in surface-local coordinates.
    pub fn down(
        &self,
        serial: Serial,
        time: u32,
        surface: &WlSurface,
        position: Point<f64, Logical>,
        slot: TouchSlot,
    ) {
        self.inner
            .lock()
            .unwrap()
            .down(serial, time, surface, position, slot);
    }

    /// Notify clients about touch point removal.
    pub fn up(&self, serial: Serial, time: u32, slot: TouchSlot) {
        self.inner.lock().unwrap().up(serial, time, slot);
    }

    /// Notify clients about touch motion.
    pub fn motion(&self, time: u32, slot: TouchSlot, location: Point<f64, Logical>) {
        self.inner.lock().unwrap().motion(time, slot, location);
    }

    /// Notify clients about touch shape changes.
    pub fn shape(&self, slot: TouchSlot, major: f64, minor: f64) {
        self.inner.lock().unwrap().shape(slot, major, minor);
    }

    /// Notify clients about touch shape orientation.
    pub fn orientation(&self, slot: TouchSlot, orientation: f64) {
        self.inner.lock().unwrap().orientation(slot, orientation);
    }

    /// Notify clients about touch cancellation.
    ///
    /// This should be sent by the compositor when the touch stream is recognized as
    /// a global gesture. Cancellation applies to all currently active touch slots.
    pub fn cancel(&self) {
        self.inner.lock().unwrap().cancel();
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
        position: Point<f64, Logical>,
        slot: TouchSlot,
    ) {
        // Update focused client state.
        let focus = self.focus.entry(slot).or_default();
        focus.handles.clear();

        // Select all WlTouch instances associated to the active WlSurface.
        for handle in &self.known_handles {
            if handle.id().same_client_as(&surface.id()) {
                focus.handles.push(handle.clone());
            }
        }

        self.with_focused_handles(slot, |handle| {
            handle.down(serial.into(), time, surface, slot.into(), position.x, position.y)
        });
    }

    fn up(&mut self, serial: Serial, time: u32, slot: TouchSlot) {
        self.with_focused_handles(slot, |handle| handle.up(serial.into(), time, slot.into()));

        // Clear this slot's associated WlTouch handles.
        if let Some(focus) = self.focus.get_mut(&slot) {
            focus.handles.clear();
        }
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
            if handle.version() >= 6 {
                handle.shape(slot.into(), major, minor);
            }
        });
    }

    fn orientation(&self, slot: TouchSlot, orientation: f64) {
        self.with_focused_handles(slot, |handle| {
            if handle.version() >= 6 {
                handle.orientation(slot.into(), orientation);
            }
        });
    }

    fn cancel(&mut self) {
        for handle in &self.known_handles {
            handle.cancel();
        }

        self.focus.clear();
    }

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

/// User data for keyboard
#[derive(Debug)]
pub struct TouchUserData {
    pub(crate) handle: Option<TouchHandle>,
}

impl<D> Dispatch<WlTouch, TouchUserData, D> for SeatState<D>
where
    D: Dispatch<WlTouch, TouchUserData>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlTouch,
        _request: wl_touch::Request,
        _data: &TouchUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client_id: ClientId, touch: &WlTouch, data: &TouchUserData) {
        if let Some(ref handle) = data.handle {
            handle
                .inner
                .lock()
                .unwrap()
                .known_handles
                .retain(|k| k.id() != touch.id())
        }
    }
}
