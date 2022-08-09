use std::sync::Mutex;

use wayland_server::{
    backend::{ClientId, ObjectId},
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
            AxisFrame, ButtonEvent, CursorImageAttributes, CursorImageStatus, MotionEvent, PointerHandle,
            PointerHandler, PointerInternal,
        },
        Seat,
    },
    utils::{IsAlive, Serial},
    wayland::compositor,
};

use super::{SeatHandler, SeatState};

impl<D> PointerHandle<D> {
    pub(crate) fn new_pointer(&self, pointer: WlPointer) {
        let mut guard = self.known_pointers.lock().unwrap();
        guard.push(pointer);
    }
}

/// WlSurface role of a cursor image icon
pub const CURSOR_IMAGE_ROLE: &str = "cursor_image";

fn with_focused_pointers<D: SeatHandler + 'static>(
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

#[cfg(feature = "wayland_frontend")]
impl<D> PointerHandler<D> for WlSurface
where
    D: SeatHandler + 'static,
{
    fn enter(&self, seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        with_focused_pointers(seat, self, |ptr| {
            ptr.enter(event.serial.into(), self, event.location.x, event.location.y);
            if ptr.version() >= 5 {
                ptr.frame();
            }
        })
    }
    fn leave(&self, seat: &Seat<D>, _data: &mut D, serial: Serial, _time: u32) {
        with_focused_pointers(seat, self, |ptr| {
            ptr.leave(serial.into(), self);
            if ptr.version() >= 5 {
                ptr.frame();
            }
        })
    }
    fn motion(&self, seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        with_focused_pointers(seat, self, |ptr| {
            ptr.motion(event.time, event.location.x, event.location.y);
            if ptr.version() >= 5 {
                ptr.frame();
            }
        })
    }
    fn button(&self, seat: &Seat<D>, _data: &mut D, event: &ButtonEvent) {
        with_focused_pointers(seat, self, |ptr| {
            ptr.button(event.serial.into(), event.time, event.button, event.state.into());
            if ptr.version() >= 5 {
                ptr.frame();
            }
        })
    }
    fn axis(&self, seat: &Seat<D>, _data: &mut D, details: AxisFrame) {
        with_focused_pointers(seat, self, |ptr| {
            // axis
            if details.axis.0 != 0.0 {
                ptr.axis(details.time, WlAxis::HorizontalScroll, details.axis.0);
            }
            if details.axis.1 != 0.0 {
                ptr.axis(details.time, WlAxis::VerticalScroll, details.axis.1);
            }
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
                if details.discrete.0 != 0 {
                    ptr.axis_discrete(WlAxis::HorizontalScroll, details.discrete.0);
                }
                if details.discrete.1 != 0 {
                    ptr.axis_discrete(WlAxis::VerticalScroll, details.discrete.1);
                }
                // stop
                if details.stop.0 {
                    ptr.axis_stop(details.time, WlAxis::HorizontalScroll);
                }
                if details.stop.1 {
                    ptr.axis_stop(details.time, WlAxis::VerticalScroll);
                }
                // frame
                ptr.frame();
            }
        })
    }

    fn is_alive(&self) -> bool {
        IsAlive::alive(self)
    }
    fn same_handler_as(&self, other: &dyn PointerHandler<D>) -> bool {
        if let Some(other_surface) = other.as_any().downcast_ref::<WlSurface>() {
            self.id() == other_surface.id()
        } else {
            false
        }
    }
    fn clone_handler(&self) -> Box<dyn PointerHandler<D> + 'static> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// User data for pointer
#[derive(Debug)]
pub struct PointerUserData<D> {
    pub(crate) handle: Option<PointerHandle<D>>,
}

impl<D> Dispatch<WlPointer, PointerUserData<D>, D> for SeatState<D>
where
    D: Dispatch<WlPointer, PointerUserData<D>>,
    D: SeatHandler,
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
                surface,
                hotspot_x,
                hotspot_y,
                ..
            } => {
                if let Some(ref handle) = data.handle {
                    let seat = {
                        let seat_state = state.seat_state();
                        seat_state
                            .seats
                            .iter()
                            .find(|seat| seat.get_pointer().map(|h| &h == handle).unwrap_or(false))
                            .cloned()
                    };

                    let guard = handle.inner.lock().unwrap();
                    // only allow setting the cursor icon if the current pointer focus
                    // is of the same client
                    let PointerInternal { ref focus, .. } = *guard;
                    if let Some((ref focus, _)) = *focus {
                        if let Some(focused_surface) = focus.as_any().downcast_ref::<WlSurface>() {
                            if focused_surface.id().same_client_as(&pointer.id()) {
                                match surface {
                                    Some(surface) => {
                                        // tolerate re-using the same surface
                                        if compositor::give_role(&surface, CURSOR_IMAGE_ROLE).is_err()
                                            && compositor::get_role(&surface) != Some(CURSOR_IMAGE_ROLE)
                                        {
                                            pointer.post_error(
                                                wl_pointer::Error::Role,
                                                "Given wl_surface has another role.",
                                            );
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

                                        if let Some(seat) = seat {
                                            state.cursor_image(&seat, CursorImageStatus::Surface(surface));
                                        }
                                    }
                                    None => {
                                        if let Some(seat) = seat {
                                            state.cursor_image(&seat, CursorImageStatus::Hidden);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Request::Release => {
                // Our destructors already handle it
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _: ClientId, object_id: ObjectId, data: &PointerUserData<D>) {
        if let Some(ref handle) = data.handle {
            handle
                .known_pointers
                .lock()
                .unwrap()
                .retain(|p| p.id() != object_id);
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
