use std::sync::{atomic::Ordering, Arc};

use atomic_float::AtomicF64;
use wayland_server::{
    backend::ClientId,
    protocol::wl_touch::{self, WlTouch},
    Dispatch, DisplayHandle, Resource,
};

use super::{SeatHandler, SeatState};
use crate::input::touch::TouchTarget;
use crate::input::{
    touch::{MotionEvent, OrientationEvent, ShapeEvent, UpEvent},
    Seat,
};
use crate::{input::touch::DownEvent, wayland::seat::wl_surface::WlSurface};
use crate::{input::touch::TouchHandle, utils::Serial};

impl<D: SeatHandler> TouchHandle<D> {
    pub(crate) fn new_touch(&self, touch: WlTouch) {
        let mut guard = self.known_instances.lock().unwrap();
        guard.push((touch.downgrade(), None));
    }
}

impl<D: SeatHandler + 'static> TouchHandle<D> {
    /// Attempt to retrieve a [`TouchHandle`] from an existing resource
    ///
    /// May return `None` for a valid `WlTouch` that was created without
    /// the keyboard capability.
    pub fn from_resource(seat: &WlTouch) -> Option<Self> {
        seat.data::<TouchUserData<D>>()?.handle.clone()
    }
}

fn for_each_focused_touch<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    seq: Serial,
    mut f: impl FnMut(WlTouch),
) {
    if let Some(touch) = seat.get_touch() {
        let mut inner = touch.known_instances.lock().unwrap();
        for (ptr, last_seq) in &mut *inner {
            let Ok(ptr) = ptr.upgrade() else {
                continue;
            };

            if ptr.id().same_client_as(&surface.id()) && last_seq.map(|last| last < seq).unwrap_or(true) {
                *last_seq = Some(seq);
                f(ptr.clone());
            }
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D> TouchTarget<D> for WlSurface
where
    D: SeatHandler + 'static,
{
    fn down(&self, seat: &Seat<D>, _data: &mut D, event: &DownEvent, seq: Serial) {
        let serial = event.serial;
        let slot = event.slot;
        for_each_focused_touch(seat, self, seq, |touch| {
            let client_scale = touch
                .data::<TouchUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);
            let location = event.location.to_client(client_scale);
            touch.down(
                serial.into(),
                event.time,
                self,
                slot.into(),
                location.x,
                location.y,
            );
        })
    }

    fn up(&self, seat: &Seat<D>, _data: &mut D, event: &UpEvent, seq: Serial) {
        let serial = event.serial;
        let slot = event.slot;
        for_each_focused_touch(seat, self, seq, |touch| {
            touch.up(serial.into(), event.time, slot.into());
        })
    }

    fn motion(&self, seat: &Seat<D>, _data: &mut D, event: &MotionEvent, seq: Serial) {
        let slot = event.slot;
        for_each_focused_touch(seat, self, seq, |touch| {
            let client_scale = touch
                .data::<TouchUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);
            let location = event.location.to_client(client_scale);
            touch.motion(event.time, slot.into(), location.x, location.y);
        })
    }

    fn frame(&self, seat: &Seat<D>, _data: &mut D, seq: Serial) {
        for_each_focused_touch(seat, self, seq, |touch| {
            touch.frame();
        })
    }

    fn cancel(&self, seat: &Seat<D>, _data: &mut D, seq: Serial) {
        for_each_focused_touch(seat, self, seq, |touch| {
            touch.cancel();
        })
    }

    fn shape(&self, seat: &Seat<D>, _data: &mut D, event: &ShapeEvent, seq: Serial) {
        let slot = event.slot;
        for_each_focused_touch(seat, self, seq, |touch| {
            touch.shape(slot.into(), event.major, event.minor);
        })
    }

    fn orientation(&self, seat: &Seat<D>, _data: &mut D, event: &OrientationEvent, seq: Serial) {
        let slot = event.slot;
        for_each_focused_touch(seat, self, seq, |touch| {
            touch.orientation(slot.into(), event.orientation);
        })
    }
}

/// User data for touch
#[derive(Debug)]
pub struct TouchUserData<D: SeatHandler> {
    pub(crate) handle: Option<TouchHandle<D>>,
    pub(crate) client_scale: Arc<AtomicF64>,
}

impl<D> Dispatch<WlTouch, TouchUserData<D>, D> for SeatState<D>
where
    D: Dispatch<WlTouch, TouchUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlTouch,
        _request: wl_touch::Request,
        _data: &TouchUserData<D>,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client_id: ClientId, touch: &WlTouch, data: &TouchUserData<D>) {
        if let Some(ref handle) = data.handle {
            handle
                .known_instances
                .lock()
                .unwrap()
                .retain(|(p, _)| p.id() != touch.id());
        }
    }
}
