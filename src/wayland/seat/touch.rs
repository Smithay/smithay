use std::sync::{Arc, atomic::Ordering};

use atomic_float::AtomicF64;
use wayland_server::{
    Client, DisplayHandle, Resource,
    backend::ClientId,
    protocol::wl_touch::{self, WlTouch},
};

use super::SeatHandler;
use crate::input::touch::TouchHandle;
use crate::input::touch::TouchTarget;
use crate::wayland::Dispatch2;
use crate::wayland::compositor::CompositorHandler;
use crate::wayland::seat::wl_surface::WlSurface;
use crate::{
    input::{
        Seat,
        touch::{DownEvent, FrameMarker, MotionEvent, OrientationEvent, ShapeEvent, UpEvent},
    },
    utils::iter::new_locked_obj_iter_from_vec,
};

impl<D: SeatHandler> TouchHandle<D> {
    pub(crate) fn new_touch(&self, touch: WlTouch) {
        let mut guard = self.known_instances.lock().unwrap();
        guard.push(touch.downgrade());
    }
}

impl<D: SeatHandler + 'static> TouchHandle<D> {
    /// Attempt to retrieve a [`TouchHandle`] from an existing resource
    ///
    /// May return `None` for a valid `WlTouch` that was created without
    /// the touch capability.
    pub fn from_resource(seat: &WlTouch) -> Option<Self> {
        seat.data::<TouchUserData<D>>()?.handle.clone()
    }

    /// Return all raw [`WlTouch`] instances for a particular [`Client`]
    pub fn client_touch<'a>(&'a self, client: &Client) -> impl Iterator<Item = WlTouch> + 'a {
        let guard = self.known_instances.lock().unwrap();
        new_locked_obj_iter_from_vec(guard, client.id())
    }
}

fn for_each_focused_touch<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(WlTouch),
) {
    if let Some(touch) = seat.get_touch() {
        let mut inner = touch.known_instances.lock().unwrap();
        for ptr in &mut *inner {
            let Ok(ptr) = ptr.upgrade() else {
                continue;
            };

            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone());
            }
        }
    }
}

fn set_touch_frame_marker<D: CompositorHandler + 'static>(
    data: &D,
    surface: &WlSurface,
    marker: FrameMarker,
) {
    if let Some(client) = surface.client().as_ref() {
        data.client_compositor_state(client).set_last_touch_frame(marker);
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D> TouchTarget<D> for WlSurface
where
    D: SeatHandler + 'static,
    D: CompositorHandler,
{
    fn down(&self, seat: &Seat<D>, _data: &mut D, event: &DownEvent) {
        let serial = event.serial;
        let slot = event.slot;

        for_each_focused_touch(seat, self, |touch| {
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

    fn up(&self, seat: &Seat<D>, _data: &mut D, event: &UpEvent) {
        let serial = event.serial;
        let slot = event.slot;
        for_each_focused_touch(seat, self, |touch| {
            touch.up(serial.into(), event.time, slot.into());
        })
    }

    fn motion(&self, seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        let slot = event.slot;
        for_each_focused_touch(seat, self, |touch| {
            let client_scale = touch
                .data::<TouchUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);
            let location = event.location.to_client(client_scale);
            touch.motion(event.time, slot.into(), location.x, location.y);
        })
    }

    fn frame(&self, seat: &Seat<D>, data: &mut D, marker: FrameMarker) {
        set_touch_frame_marker(data, self, marker);

        for_each_focused_touch(seat, self, |touch| {
            touch.frame();
        })
    }

    fn cancel(&self, seat: &Seat<D>, data: &mut D, marker: FrameMarker) {
        set_touch_frame_marker(data, self, marker);

        for_each_focused_touch(seat, self, |touch| {
            touch.cancel();
        })
    }

    fn shape(&self, seat: &Seat<D>, _data: &mut D, event: &ShapeEvent) {
        let slot = event.slot;
        for_each_focused_touch(seat, self, |touch| {
            touch.shape(slot.into(), event.major, event.minor);
        })
    }

    fn orientation(&self, seat: &Seat<D>, _data: &mut D, event: &OrientationEvent) {
        let slot = event.slot;
        for_each_focused_touch(seat, self, |touch| {
            touch.orientation(slot.into(), event.orientation);
        })
    }

    fn last_frame(&self, _seat: &Seat<D>, data: &mut D) -> Option<FrameMarker> {
        self.client()
            .and_then(|c| data.client_compositor_state(&c).last_touch_frame())
    }
}

/// User data for touch
#[derive(Debug)]
pub struct TouchUserData<D: SeatHandler> {
    pub(crate) handle: Option<TouchHandle<D>>,
    pub(crate) client_scale: Arc<AtomicF64>,
}

impl<D> Dispatch2<WlTouch, D> for TouchUserData<D>
where
    D: SeatHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlTouch,
        _request: wl_touch::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(&self, _state: &mut D, _client_id: ClientId, touch: &WlTouch) {
        if let Some(ref handle) = self.handle {
            handle
                .known_instances
                .lock()
                .unwrap()
                .retain(|p| p.id() != touch.id());
        }
    }
}
