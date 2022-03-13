use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::wl_touch::{self, WlTouch},
    DelegateDispatch, DelegateDispatchBase, Dispatch, DisplayHandle, Resource,
};

use super::{SeatHandler, SeatState};
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
    pub fn down(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        serial: Serial,
        time: u32,
        surface: &WlSurface,
        surface_offset: Point<i32, Logical>,
        slot: TouchSlot,
        location: Point<f64, Logical>,
    ) {
        self.inner
            .lock()
            .unwrap()
            .down(dh, serial, time, surface, surface_offset, slot, location);
    }

    /// Notify clients about touch point removal.
    pub fn up(&self, dh: &mut DisplayHandle<'_>, serial: Serial, time: u32, slot: TouchSlot) {
        self.inner.lock().unwrap().up(dh, serial, time, slot);
    }

    /// Notify clients about touch motion.
    pub fn motion(
        &self,
        dh: &mut DisplayHandle<'_>,
        time: u32,
        slot: TouchSlot,
        location: Point<f64, Logical>,
    ) {
        self.inner.lock().unwrap().motion(dh, time, slot, location);
    }

    /// Notify clients about touch shape changes.
    pub fn shape(&self, dh: &mut DisplayHandle<'_>, slot: TouchSlot, major: f64, minor: f64) {
        self.inner.lock().unwrap().shape(dh, slot, major, minor);
    }

    /// Notify clients about touch shape orientation.
    pub fn orientation(&self, dh: &mut DisplayHandle<'_>, slot: TouchSlot, orientation: f64) {
        self.inner.lock().unwrap().orientation(dh, slot, orientation);
    }

    /// Notify clients about touch cancellation.
    ///
    /// This should be sent by the compositor when the currently active touch
    /// slot was recognized as a gesture.
    pub fn cancel(&self, dh: &mut DisplayHandle<'_>) {
        self.inner.lock().unwrap().cancel(dh);
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
        dh: &mut DisplayHandle<'_>,
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
            if handle.id().same_client_as(&surface.id()) {
                focus.handles.push(handle.clone());
            }
        }

        let (x, y) = (location - focus.surface_offset).into();
        self.with_focused_handles(dh, slot, |dh, handle| {
            handle.down(dh, serial.into(), time, surface, slot.into(), x, y)
        });
    }

    fn up(&self, dh: &mut DisplayHandle<'_>, serial: Serial, time: u32, slot: TouchSlot) {
        self.with_focused_handles(dh, slot, |dh, handle| {
            handle.up(dh, serial.into(), time, slot.into())
        });
    }

    fn motion(&self, dh: &mut DisplayHandle<'_>, time: u32, slot: TouchSlot, location: Point<f64, Logical>) {
        let focus = match self.focus.get(&slot) {
            Some(slot) => slot,
            None => return,
        };

        let (x, y) = (location - focus.surface_offset).into();
        self.with_focused_handles(dh, slot, |dh, handle| handle.motion(dh, time, slot.into(), x, y));
    }

    fn shape(&self, dh: &mut DisplayHandle<'_>, slot: TouchSlot, major: f64, minor: f64) {
        self.with_focused_handles(dh, slot, |dh, handle| {
            if handle.version() >= 6 {
                handle.shape(dh, slot.into(), major, minor);
            }
        });
    }

    fn orientation(&self, dh: &mut DisplayHandle<'_>, slot: TouchSlot, orientation: f64) {
        self.with_focused_handles(dh, slot, |dh, handle| {
            if handle.version() >= 6 {
                handle.orientation(dh, slot.into(), orientation);
            }
        });
    }

    // TODO: In theory doesn't need to be sent for WlTouch that isn't in the focus hashmap?
    fn cancel(&self, dh: &mut DisplayHandle<'_>) {
        for handle in &self.known_handles {
            handle.cancel(dh);
        }
    }

    // TODO: Document this also sends frame every time.
    #[inline]
    fn with_focused_handles<F>(&self, dh: &mut DisplayHandle<'_>, slot: TouchSlot, mut f: F)
    where
        F: FnMut(&mut DisplayHandle<'_>, &WlTouch),
    {
        if let Some(focus) = self.focus.get(&slot) {
            for handle in &focus.handles {
                f(dh, handle);
                handle.frame(dh);
            }
        }
    }
}

/// User data for keyboard
#[derive(Debug)]
pub struct TouchUserData {
    pub(crate) handle: Option<TouchHandle>,
}

impl<T> DelegateDispatchBase<WlTouch> for SeatState<T> {
    type UserData = TouchUserData;
}

impl<T, D> DelegateDispatch<WlTouch, D> for SeatState<T>
where
    D: 'static + Dispatch<WlTouch, UserData = TouchUserData>,
    D: SeatHandler<T>,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlTouch,
        _request: wl_touch::Request,
        _data: &Self::UserData,
        _dhandle: &mut DisplayHandle<'_>,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client_id: ClientId, object_id: ObjectId, data: &Self::UserData) {
        if let Some(ref handle) = data.handle {
            handle
                .inner
                .lock()
                .unwrap()
                .known_handles
                .retain(|k| k.id() != object_id)
        }
    }
}
