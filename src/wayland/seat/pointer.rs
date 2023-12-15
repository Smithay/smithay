use std::{fmt, sync::Mutex};

use wayland_protocols::wp::{
    pointer_gestures::zv1::server::{
        zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1,
        zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
        zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1,
    },
    relative_pointer::zv1::server::zwp_relative_pointer_v1::ZwpRelativePointerV1,
};
use wayland_server::{
    backend::ClientId,
    protocol::{
        wl_pointer::{
            self, Axis as WlAxis, AxisSource as WlAxisSource, ButtonState as WlButtonState, Request,
            WlPointer,
        },
        wl_surface::WlSurface,
    },
    Dispatch, DisplayHandle, Resource,
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
    utils::{Serial, SERIAL_COUNTER},
    wayland::{
        compositor, pointer_constraints::with_pointer_constraint, pointer_gestures::PointerGestureUserData,
    },
};

use super::{SeatHandler, SeatState, WaylandFocus};

// Use to accumulate discrete values for `wl_pointer` < 8
#[derive(Default)]
struct V120UserData {
    x: i32,
    y: i32,
}

impl<D: SeatHandler> PointerHandle<D> {
    pub(crate) fn new_pointer(&self, pointer: WlPointer) {
        let mut guard = self.known_pointers.lock().unwrap();
        guard.push(pointer);
    }

    pub(crate) fn new_relative_pointer(&self, pointer: ZwpRelativePointerV1) {
        let mut guard = self.known_relative_pointers.lock().unwrap();
        guard.push(pointer);
    }

    pub(crate) fn new_swipe_gesture(&self, gesture: ZwpPointerGestureSwipeV1) {
        let mut guard = self.known_swipe_gestures.lock().unwrap();
        guard.push(gesture);
    }

    pub(crate) fn new_pinch_gesture(&self, gesture: ZwpPointerGesturePinchV1) {
        let mut guard = self.known_pinch_gestures.lock().unwrap();
        guard.push(gesture);
    }

    pub(crate) fn new_hold_gesture(&self, gesture: ZwpPointerGestureHoldV1) {
        let mut guard = self.known_hold_gestures.lock().unwrap();
        guard.push(gesture);
    }
}

/// WlSurface role of a cursor image icon
pub const CURSOR_IMAGE_ROLE: &str = "cursor_image";

fn for_each_focused_pointers<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(WlPointer),
) {
    if let Some(pointer) = seat.get_pointer() {
        let inner = pointer.known_pointers.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

fn for_each_focused_relative_pointers<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(ZwpRelativePointerV1),
) {
    if let Some(pointer) = seat.get_pointer() {
        let inner = pointer.known_relative_pointers.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

fn for_each_focused_swipe_gestures<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(ZwpPointerGestureSwipeV1),
) {
    if let Some(pointer) = seat.get_pointer() {
        let inner = pointer.known_swipe_gestures.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

fn for_each_focused_pinch_gestures<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(ZwpPointerGesturePinchV1),
) {
    if let Some(pointer) = seat.get_pointer() {
        let inner = pointer.known_pinch_gestures.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

fn for_each_focused_hold_gestures<D: SeatHandler + 'static>(
    seat: &Seat<D>,
    surface: &WlSurface,
    mut f: impl FnMut(ZwpPointerGestureHoldV1),
) {
    if let Some(pointer) = seat.get_pointer() {
        let inner = pointer.known_hold_gestures.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D> PointerTarget<D> for WlSurface
where
    D: SeatHandler + 'static,
{
    fn enter(&self, seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        let serial = event.serial;
        if let Some(pointer) = seat.get_pointer() {
            *pointer.last_enter.lock().unwrap() = Some(serial);
        }
        for_each_focused_pointers(seat, self, |ptr| {
            ptr.enter(serial.into(), self, event.location.x, event.location.y);
        })
    }
    fn leave(&self, seat: &Seat<D>, _data: &mut D, serial: Serial, time: u32) {
        for_each_focused_swipe_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            if ongoing.is_some() {
                // Cancel the ongoing gesture.
                gesture.end(serial.into(), time, 1);
            }
        });
        for_each_focused_pinch_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            if ongoing.is_some() {
                // Cancel the ongoing gesture.
                gesture.end(serial.into(), time, 1);
            }
        });
        for_each_focused_hold_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            if ongoing.is_some() {
                // Cancel the ongoing gesture.
                gesture.end(serial.into(), time, 1);
            }
        });
        for_each_focused_pointers(seat, self, |ptr| {
            ptr.leave(serial.into(), self);
            if ptr.version() >= 5 {
                ptr.frame();
            }
        });
        if let Some(pointer) = seat.get_pointer() {
            *pointer.last_enter.lock().unwrap() = None;
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
        for_each_focused_pointers(seat, self, |ptr| {
            ptr.motion(event.time, event.location.x, event.location.y);
        })
    }
    fn relative_motion(&self, seat: &Seat<D>, _data: &mut D, event: &RelativeMotionEvent) {
        for_each_focused_relative_pointers(seat, self, |ptr| {
            let utime_hi = (event.utime >> 32) as u32;
            let utime_lo = (event.utime & 0xffffffff) as u32;
            ptr.relative_motion(
                utime_hi,
                utime_lo,
                event.delta.x,
                event.delta.y,
                event.delta_unaccel.x,
                event.delta_unaccel.y,
            );
        })
    }
    fn button(&self, seat: &Seat<D>, _data: &mut D, event: &ButtonEvent) {
        for_each_focused_pointers(seat, self, |ptr| {
            ptr.button(event.serial.into(), event.time, event.button, event.state.into());
        })
    }
    fn axis(&self, seat: &Seat<D>, _data: &mut D, details: AxisFrame) {
        for_each_focused_pointers(seat, self, |ptr| {
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
                        compositor::with_states(self, |states| {
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

                    compositor::with_states(self, |states| {
                        if let Some(data) = states.data_map.get::<Mutex<V120UserData>>() {
                            data.lock().unwrap().x = 0;
                        }
                    });
                }
                if details.stop.1 {
                    ptr.axis_stop(details.time, WlAxis::VerticalScroll);

                    compositor::with_states(self, |states| {
                        if let Some(data) = states.data_map.get::<Mutex<V120UserData>>() {
                            data.lock().unwrap().y = 0;
                        }
                    });
                }
            }
            // axis
            if details.axis.0 != 0.0 {
                if ptr.version() >= 9 {
                    ptr.axis_relative_direction(
                        WlAxis::HorizontalScroll,
                        details.relative_direction.0.into(),
                    );
                }
                ptr.axis(details.time, WlAxis::HorizontalScroll, details.axis.0);
            }
            if details.axis.1 != 0.0 {
                if ptr.version() >= 9 {
                    ptr.axis_relative_direction(WlAxis::VerticalScroll, details.relative_direction.1.into());
                }
                ptr.axis(details.time, WlAxis::VerticalScroll, details.axis.1);
            }
        })
    }

    fn frame(&self, seat: &Seat<D>, _data: &mut D) {
        for_each_focused_pointers(seat, self, |ptr| {
            if ptr.version() >= 5 {
                ptr.frame();
            }
        });
    }

    fn gesture_swipe_begin(&self, seat: &Seat<D>, _data: &mut D, event: &GestureSwipeBeginEvent) {
        for_each_focused_swipe_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().replace(self.clone());
            if ongoing.is_some() {
                // Cancel an ongoing gesture for a different surface.
                gesture.end(event.serial.into(), event.time, 1);
            }
            gesture.begin(event.serial.into(), event.time, self, event.fingers);
        })
    }

    fn gesture_swipe_update(&self, seat: &Seat<D>, _data: &mut D, event: &GestureSwipeUpdateEvent) {
        for_each_focused_swipe_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let mut ongoing = data.in_progress_on.lock().unwrap();
            // Check that the ongoing gesture is for this surface.
            if ongoing.as_ref() == Some(self) {
                gesture.update(event.time, event.delta.x, event.delta.y);
            } else if ongoing.take().is_some() {
                // If it was for a different surface, cancel it.
                gesture.end(SERIAL_COUNTER.next_serial().into(), event.time, 1);
            }
        })
    }

    fn gesture_swipe_end(&self, seat: &Seat<D>, _data: &mut D, event: &GestureSwipeEndEvent) {
        for_each_focused_swipe_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            // Check if the gesture was ongoing.
            if ongoing.is_some() {
                let cancelled = if ongoing.as_ref() == Some(self) {
                    event.cancelled
                } else {
                    // If the gesture was ongoing for any other surface then cancel it.
                    true
                };
                gesture.end(event.serial.into(), event.time, cancelled.into());
            }
        })
    }

    fn gesture_pinch_begin(&self, seat: &Seat<D>, _data: &mut D, event: &GesturePinchBeginEvent) {
        for_each_focused_pinch_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().replace(self.clone());
            if ongoing.is_some() {
                // Cancel an ongoing gesture for a different surface.
                gesture.end(event.serial.into(), event.time, 1);
            }
            gesture.begin(event.serial.into(), event.time, self, event.fingers);
        })
    }

    fn gesture_pinch_update(&self, seat: &Seat<D>, _data: &mut D, event: &GesturePinchUpdateEvent) {
        for_each_focused_pinch_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let mut ongoing = data.in_progress_on.lock().unwrap();
            // Check that the ongoing gesture is for this surface.
            if ongoing.as_ref() == Some(self) {
                gesture.update(
                    event.time,
                    event.delta.x,
                    event.delta.y,
                    event.scale,
                    event.rotation,
                );
            } else if ongoing.take().is_some() {
                // If it was for a different surface, cancel it.
                gesture.end(SERIAL_COUNTER.next_serial().into(), event.time, 1);
            }
        })
    }

    fn gesture_pinch_end(&self, seat: &Seat<D>, _data: &mut D, event: &GesturePinchEndEvent) {
        for_each_focused_pinch_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            // Check if the gesture was ongoing.
            if ongoing.is_some() {
                let cancelled = if ongoing.as_ref() == Some(self) {
                    event.cancelled
                } else {
                    // If the gesture was ongoing for any other surface then cancel it.
                    true
                };
                gesture.end(event.serial.into(), event.time, cancelled.into());
            }
        })
    }

    fn gesture_hold_begin(&self, seat: &Seat<D>, _data: &mut D, event: &GestureHoldBeginEvent) {
        for_each_focused_hold_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().replace(self.clone());
            if ongoing.is_some() {
                // Cancel an ongoing gesture for a different surface.
                gesture.end(event.serial.into(), event.time, 1);
            }
            gesture.begin(event.serial.into(), event.time, self, event.fingers);
        })
    }

    fn gesture_hold_end(&self, seat: &Seat<D>, _data: &mut D, event: &GestureHoldEndEvent) {
        for_each_focused_hold_gestures(seat, self, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            // Check if the gesture was ongoing.
            if ongoing.is_some() {
                let cancelled = if ongoing.as_ref() == Some(self) {
                    event.cancelled
                } else {
                    // If the gesture was ongoing for any other surface then cancel it.
                    true
                };
                gesture.end(event.serial.into(), event.time, cancelled.into());
            }
        })
    }
}

/// User data for pointer
pub struct PointerUserData<D: SeatHandler> {
    pub(crate) handle: Option<PointerHandle<D>>,
}

impl<D: SeatHandler> fmt::Debug for PointerUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerUserData")
            .field("handle", &self.handle)
            .finish()
    }
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

                if !handle
                    .last_enter
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|last_serial| last_serial.0 == serial)
                    .unwrap_or(false)
                {
                    return; // Ignore mismatches in serial
                }

                // Only allow setting the cursor icon if the current pointer focus is of the same
                // client.
                if !handle
                    .inner
                    .lock()
                    .unwrap()
                    .focus
                    .as_ref()
                    .map(|(focus, _)| focus.same_client_as(&pointer.id()))
                    .unwrap_or(false)
                {
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
                            states
                                .data_map
                                .get::<Mutex<CursorImageAttributes>>()
                                .unwrap()
                                .lock()
                                .unwrap()
                                .hotspot = (hotspot_x, hotspot_y).into();
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
                .known_pointers
                .lock()
                .unwrap()
                .retain(|p| p.id() != pointer.id());
        }
    }
}

impl From<Axis> for WlAxis {
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
    fn try_from(value: WlAxis) -> Result<Self, Self::Error> {
        match value {
            WlAxis::HorizontalScroll => Ok(Axis::Horizontal),
            WlAxis::VerticalScroll => Ok(Axis::Vertical),
            x => Err(UnknownAxis(x)),
        }
    }
}

impl From<AxisSource> for WlAxisSource {
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
    fn try_from(value: WlButtonState) -> Result<Self, Self::Error> {
        match value {
            WlButtonState::Pressed => Ok(ButtonState::Pressed),
            WlButtonState::Released => Ok(ButtonState::Released),
            x => Err(UnknownButtonState(x)),
        }
    }
}
