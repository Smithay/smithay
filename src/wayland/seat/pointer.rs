use std::sync::{atomic::Ordering, Arc, Mutex};

use atomic_float::AtomicF64;
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::{
        wl_pointer::{
            self, Axis as WlAxis, AxisSource as WlAxisSource, ButtonState as WlButtonState, Request,
            WlPointer,
        },
        wl_surface::WlSurface,
    },
    Client, Dispatch, DisplayHandle, Resource, Weak,
};

use crate::{
    backend::input::{Axis, AxisSource, ButtonState},
    input::{
        pointer::{
            AxisFrame, ButtonEvent, CursorImageAttributes, CursorImageStatus, GestureHoldBeginEvent,
            GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            PointerHandle, PointerTarget, RelativeMotionEvent,
        },
        Seat,
    },
    utils::{iter::new_locked_obj_iter_from_vec, Client as ClientCoords, Point, Serial},
    wayland::{compositor, pointer_constraints::with_pointer_constraint},
};

use super::{SeatHandler, SeatState, WaylandFocus};

// Use to accumulate discrete values for `wl_pointer` < 8
#[derive(Default)]
struct V120UserData {
    x: i32,
    y: i32,
}

/// WlSurface role of a cursor image icon
pub const CURSOR_IMAGE_ROLE: &str = "cursor_image";

impl<D: SeatHandler + 'static> PointerHandle<D> {
    /// Attempt to retrieve a [`PointerHandle`] from an existing resource
    ///
    /// May return `None` for a valid `WlPointer` that was created without
    /// the keyboard capability.
    pub fn from_resource(seat: &WlPointer) -> Option<Self> {
        seat.data::<PointerUserData<D>>()?.handle.clone()
    }

    /// Return all raw [`WlPointer`] instances for a particular [`Client`]
    pub fn client_pointers<'a>(&'a self, client: &Client) -> impl Iterator<Item = WlPointer> + 'a {
        let guard = self.wl_pointer.known_pointers.lock().unwrap();
        new_locked_obj_iter_from_vec(guard, client.id())
    }
}

#[derive(Debug, Default)]
pub(crate) struct WlPointerHandle {
    pub(crate) last_enter: Mutex<Option<Serial>>,
    known_pointers: Mutex<Vec<Weak<WlPointer>>>,
}

impl WlPointerHandle {
    pub(super) fn new_pointer(&self, pointer: WlPointer) {
        self.known_pointers.lock().unwrap().push(pointer.downgrade());
    }

    fn enter<D: SeatHandler + 'static>(&self, surface: &WlSurface, event: &MotionEvent) {
        *self.last_enter.lock().unwrap() = Some(event.serial);

        self.for_each_focused_pointer(surface, |ptr| {
            let client_scale = ptr
                .data::<PointerUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);
            let location = event.location.to_client(client_scale);
            ptr.enter(event.serial.into(), surface, location.x, location.y);
        })
    }

    fn leave(&self, surface: &WlSurface, serial: Serial, _time: u32) {
        self.for_each_focused_pointer(surface, |ptr| {
            ptr.leave(serial.into(), surface);
            if ptr.version() >= 5 {
                ptr.frame();
            }
        });

        *self.last_enter.lock().unwrap() = None;
    }

    fn motion<D: SeatHandler + 'static>(&self, surface: &WlSurface, event: &MotionEvent) {
        self.for_each_focused_pointer(surface, |ptr| {
            let client_scale = ptr
                .data::<PointerUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);
            let location = event.location.to_client(client_scale);
            ptr.motion(event.time, location.x, location.y);
        })
    }

    fn button(&self, surface: &WlSurface, event: &ButtonEvent) {
        self.for_each_focused_pointer(surface, |ptr| {
            ptr.button(event.serial.into(), event.time, event.button, event.state.into());
        })
    }

    fn axis<D: SeatHandler + 'static>(&self, surface: &WlSurface, details: AxisFrame) {
        self.for_each_focused_pointer(surface, |ptr| {
            if ptr.version() >= 5 {
                // axis source
                if let Some(source) = details.source {
                    // WheelTilt was not supported before version 6
                    // The best we can do is replace it with Wheel
                    let source = if ptr.version() < 6 {
                        match source {
                            AxisSource::WheelTilt => AxisSource::Wheel,
                            other => other,
                        }
                    } else {
                        source
                    }
                    .into();
                    ptr.axis_source(source);
                }
                // axis discrete
                if let Some((x, y)) = details.v120 {
                    if ptr.version() >= 8 {
                        if x != 0 {
                            ptr.axis_value120(WlAxis::HorizontalScroll, x);
                        }
                        if y != 0 {
                            ptr.axis_value120(WlAxis::VerticalScroll, y);
                        }
                    } else {
                        compositor::with_states(surface, |states| {
                            let mut data = states
                                .data_map
                                .get_or_insert_threadsafe(Mutex::<V120UserData>::default)
                                .lock()
                                .unwrap();

                            data.x += x;
                            if data.x.abs() >= 120 {
                                ptr.axis_discrete(WlAxis::HorizontalScroll, data.x / 120);
                                data.x %= 120;
                            }

                            data.y += y;
                            if data.y.abs() >= 120 {
                                ptr.axis_discrete(WlAxis::VerticalScroll, data.y / 120);
                                data.y %= 120;
                            }
                        });
                    }
                }
                // stop
                if details.stop.0 {
                    ptr.axis_stop(details.time, WlAxis::HorizontalScroll);

                    compositor::with_states(surface, |states| {
                        if let Some(data) = states.data_map.get::<Mutex<V120UserData>>() {
                            data.lock().unwrap().x = 0;
                        }
                    });
                }
                if details.stop.1 {
                    ptr.axis_stop(details.time, WlAxis::VerticalScroll);

                    compositor::with_states(surface, |states| {
                        if let Some(data) = states.data_map.get::<Mutex<V120UserData>>() {
                            data.lock().unwrap().y = 0;
                        }
                    });
                }
            }
            // axis
            let client_scale = ptr
                .data::<PointerUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);
            if details.axis.0 != 0.0 {
                if ptr.version() >= 9 {
                    ptr.axis_relative_direction(
                        WlAxis::HorizontalScroll,
                        details.relative_direction.0.into(),
                    );
                }
                ptr.axis(
                    details.time,
                    WlAxis::HorizontalScroll,
                    details.axis.0 * client_scale,
                );
            }
            if details.axis.1 != 0.0 {
                if ptr.version() >= 9 {
                    ptr.axis_relative_direction(WlAxis::VerticalScroll, details.relative_direction.1.into());
                }
                ptr.axis(
                    details.time,
                    WlAxis::VerticalScroll,
                    details.axis.1 * client_scale,
                );
            }
        })
    }

    fn frame(&self, surface: &WlSurface) {
        self.for_each_focused_pointer(surface, |ptr| {
            if ptr.version() >= 5 {
                ptr.frame();
            }
        });
    }

    fn for_each_focused_pointer(&self, surface: &WlSurface, mut f: impl FnMut(WlPointer)) {
        let inner = self.known_pointers.lock().unwrap();
        for ptr in &*inner {
            let Ok(ptr) = ptr.upgrade() else {
                continue;
            };

            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

impl<D> PointerTarget<D> for WlSurface
where
    D: SeatHandler + 'static,
{
    fn enter(&self, seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wl_pointer.enter::<D>(self, event);
        }
    }

    fn leave(&self, seat: &Seat<D>, _data: &mut D, serial: Serial, time: u32) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.leave::<D>(self, serial, time);
            pointer.wl_pointer.leave(self, serial, time);

            with_pointer_constraint(self, &pointer, |constraint| {
                if let Some(constraint) = constraint {
                    constraint.deactivate();
                }
            });
        }

        compositor::with_states(self, |states| {
            if let Some(data) = states.data_map.get::<Mutex<V120UserData>>() {
                *data.lock().unwrap() = Default::default();
            }
        });
    }

    fn motion(&self, seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wl_pointer.motion::<D>(self, event);
        }
    }

    fn relative_motion(&self, seat: &Seat<D>, _data: &mut D, event: &RelativeMotionEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_relative.relative_motion::<D>(self, event);
        }
    }

    fn button(&self, seat: &Seat<D>, _data: &mut D, event: &ButtonEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wl_pointer.button(self, event);
        }
    }

    fn axis(&self, seat: &Seat<D>, _data: &mut D, details: AxisFrame) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wl_pointer.axis::<D>(self, details);
        }
    }

    fn frame(&self, seat: &Seat<D>, _data: &mut D) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wl_pointer.frame(self);
        }
    }

    fn gesture_swipe_begin(&self, seat: &Seat<D>, _data: &mut D, event: &GestureSwipeBeginEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_swipe_begin::<D>(self, event);
        }
    }

    fn gesture_swipe_update(&self, seat: &Seat<D>, _data: &mut D, event: &GestureSwipeUpdateEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_swipe_update::<D>(self, event);
        }
    }

    fn gesture_swipe_end(&self, seat: &Seat<D>, _data: &mut D, event: &GestureSwipeEndEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_swipe_end::<D>(self, event);
        }
    }

    fn gesture_pinch_begin(&self, seat: &Seat<D>, _data: &mut D, event: &GesturePinchBeginEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_pinch_begin::<D>(self, event);
        }
    }

    fn gesture_pinch_update(&self, seat: &Seat<D>, _data: &mut D, event: &GesturePinchUpdateEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_pinch_update::<D>(self, event);
        }
    }

    fn gesture_pinch_end(&self, seat: &Seat<D>, _data: &mut D, event: &GesturePinchEndEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_pinch_end::<D>(self, event);
        }
    }

    fn gesture_hold_begin(&self, seat: &Seat<D>, _data: &mut D, event: &GestureHoldBeginEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_hold_begin::<D>(self, event);
        }
    }

    fn gesture_hold_end(&self, seat: &Seat<D>, _data: &mut D, event: &GestureHoldEndEvent) {
        if let Some(pointer) = seat.get_pointer() {
            pointer.wp_pointer_gestures.gesture_hold_end::<D>(self, event);
        }
    }
}

/// User data for pointer
#[derive(Debug)]
pub struct PointerUserData<D: SeatHandler> {
    pub(crate) handle: Option<PointerHandle<D>>,
    pub(crate) client_scale: Arc<AtomicF64>,
}

impl<D> Dispatch<WlPointer, PointerUserData<D>, D> for SeatState<D>
where
    D: Dispatch<WlPointer, PointerUserData<D>>,
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        pointer: &WlPointer,
        request: wl_pointer::Request,
        data: &PointerUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            Request::SetCursor {
                serial,
                surface,
                hotspot_x,
                hotspot_y,
            } => {
                let handle = match &data.handle {
                    Some(handle) => handle,
                    None => return,
                };

                if !allow_setting_cursor(handle, Serial(serial), &pointer.id()) {
                    return;
                }

                let cursor_image = match surface {
                    Some(surface) => {
                        // tolerate re-using the same surface
                        if compositor::give_role(&surface, CURSOR_IMAGE_ROLE).is_err()
                            && compositor::get_role(&surface) != Some(CURSOR_IMAGE_ROLE)
                        {
                            pointer.post_error(wl_pointer::Error::Role, "Given wl_surface has another role.");
                            return;
                        }

                        compositor::with_states(&surface, |states| {
                            states.data_map.insert_if_missing_threadsafe(|| {
                                Mutex::new(CursorImageAttributes {
                                    hotspot: (0, 0).into(),
                                })
                            });
                            let client_scale = pointer
                                .data::<PointerUserData<D>>()
                                .unwrap()
                                .client_scale
                                .load(Ordering::Acquire);
                            let hotspot = Point::<i32, ClientCoords>::from((hotspot_x, hotspot_y))
                                .to_f64()
                                .to_logical(client_scale)
                                .to_i32_round();
                            states
                                .data_map
                                .get::<Mutex<CursorImageAttributes>>()
                                .unwrap()
                                .lock()
                                .unwrap()
                                .hotspot = hotspot;
                        });

                        CursorImageStatus::Surface(surface)
                    }
                    None => CursorImageStatus::Hidden,
                };

                let seat = state
                    .seat_state()
                    .seats
                    .iter()
                    .find(|seat| seat.get_pointer().map(|h| h == *handle).unwrap_or(false))
                    .cloned();

                if let Some(seat) = seat {
                    state.cursor_image(&seat, cursor_image)
                }
            }
            Request::Release => {
                // Our destructors already handle it
            }
            _ => unreachable!(),
        };
    }

    fn destroyed(_state: &mut D, _: ClientId, pointer: &WlPointer, data: &PointerUserData<D>) {
        if let Some(ref handle) = data.handle {
            handle
                .wl_pointer
                .known_pointers
                .lock()
                .unwrap()
                .retain(|p| p.id() != pointer.id());
        }
    }
}

impl From<Axis> for WlAxis {
    #[inline]
    fn from(axis: Axis) -> WlAxis {
        match axis {
            Axis::Horizontal => WlAxis::HorizontalScroll,
            Axis::Vertical => WlAxis::VerticalScroll,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unknown Axis {0:?}")]
pub struct UnknownAxis(WlAxis);

impl TryFrom<WlAxis> for Axis {
    type Error = UnknownAxis;
    #[inline]
    fn try_from(value: WlAxis) -> Result<Self, Self::Error> {
        match value {
            WlAxis::HorizontalScroll => Ok(Axis::Horizontal),
            WlAxis::VerticalScroll => Ok(Axis::Vertical),
            x => Err(UnknownAxis(x)),
        }
    }
}

impl From<AxisSource> for WlAxisSource {
    #[inline]
    fn from(axis: AxisSource) -> WlAxisSource {
        match axis {
            AxisSource::Wheel => WlAxisSource::Wheel,
            AxisSource::Finger => WlAxisSource::Finger,
            AxisSource::Continuous => WlAxisSource::Continuous,
            AxisSource::WheelTilt => WlAxisSource::WheelTilt,
        }
    }
}

impl From<ButtonState> for WlButtonState {
    #[inline]
    fn from(state: ButtonState) -> WlButtonState {
        match state {
            ButtonState::Pressed => WlButtonState::Pressed,
            ButtonState::Released => WlButtonState::Released,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unknown ButtonState {0:?}")]
pub struct UnknownButtonState(WlButtonState);

impl TryFrom<WlButtonState> for ButtonState {
    type Error = UnknownButtonState;
    #[inline]
    fn try_from(value: WlButtonState) -> Result<Self, Self::Error> {
        match value {
            WlButtonState::Pressed => Ok(ButtonState::Pressed),
            WlButtonState::Released => Ok(ButtonState::Released),
            x => Err(UnknownButtonState(x)),
        }
    }
}

pub(crate) fn allow_setting_cursor<D>(handle: &PointerHandle<D>, serial: Serial, object_id: &ObjectId) -> bool
where
    D: SeatHandler + 'static,
    <D as SeatHandler>::PointerFocus: WaylandFocus,
{
    // Allow client if there is a pointer grab for that client. Like drag and drop.
    if handle
        .grab_start_data()
        .and_then(|data| data.focus)
        .is_some_and(|focus| focus.0.same_client_as(object_id))
    {
        return true;
    }

    if !handle
        .wl_pointer
        .last_enter
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|last_serial| *last_serial == serial)
    {
        return false; // Ignore mismatches in serial
    }

    // Only allow setting the cursor icon if the current pointer focus is of the same
    // client.
    handle
        .inner
        .lock()
        .unwrap()
        .focus
        .as_ref()
        .is_some_and(|(focus, _)| focus.same_client_as(object_id))
}
