//! Utilities for wlr-virtual-pointer support
//!
//! This module provides utilities to handle virtual pointer instances, allowing
//! clients to emulate a physical pointer device.
//!
//! ```
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! use smithay::wayland::virtual_pointer::{VirtualPointerManagerState, VirtualPointerHandler};
//! use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
//! # use smithay::reexports::wayland_server::Client;
//! use smithay::utils::{Logical, Point};
//!
//! # struct State { seat_state: SeatState<Self> };
//!
//! smithay::delegate_dispatch2!(State);
//!
//! # let mut display = Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let seat_state = SeatState::<State>::new();
//!
//! impl SeatHandler for State {
//!     type KeyboardFocus = WlSurface;
//!     type PointerFocus = WlSurface;
//!     type TouchFocus = WlSurface;
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! }
//!
//! impl VirtualPointerHandler for State {
//!     fn virtual_pointer_motion(
//!         &mut self,
//!         seat: Option<&Seat<Self>>,
//!         time: u32,
//!         delta: Point<f64, Logical>,
//!     ) {
//!         // Move the pointer by delta, update focus
//!     }
//!
//!     fn virtual_pointer_motion_absolute(
//!         &mut self,
//!         seat: Option<&Seat<Self>>,
//!         time: u32,
//!         x: u32,
//!         y: u32,
//!         x_extent: u32,
//!         y_extent: u32,
//!     ) {
//!         // Map (x/x_extent, y/y_extent) to compositor coordinates, update focus
//!     }
//! }
//!
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &WlSurface) {}
//! # }
//!
//! // Create the global, gating access behind a client filter
//! VirtualPointerManagerState::new::<State, _>(&display_handle, |_client| true);
//! ```

use std::{fmt, sync::Mutex};

use tracing::{debug, trace, warn};
use wayland_protocols_wlr::virtual_pointer::v1::server::{
    zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
    zwlr_virtual_pointer_v1::{self, Error as VirtualPointerError, ZwlrVirtualPointerV1},
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
    backend::GlobalId,
    protocol::wl_pointer,
};

use crate::{
    backend::input::{Axis, AxisSource, ButtonState},
    input::{
        Seat, SeatHandler,
        pointer::{AxisFrame, ButtonEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{Dispatch2, GlobalData, GlobalDispatch2},
};

const MANAGER_VERSION: u32 = 2;

/// Handler trait for wlr-virtual-pointer protocol events.
///
/// Compositors implement this trait to respond to virtual pointer motion events.
/// Button, axis, and frame events are dispatched directly to the seat's pointer
/// handle by Smithay.
pub trait VirtualPointerHandler: SeatHandler + 'static {
    /// Return the seat to use when a client creates a virtual pointer without
    /// specifying one (`seat = null` in the protocol request).
    ///
    /// The wlr-virtual-pointer spec says: "If the seat is null, the compositor
    /// SHOULD use a default seat." Implementing this method lets Smithay dispatch
    /// button and axis events for seat-less virtual pointers.
    ///
    /// The default implementation returns `None`, which causes those events to be
    /// dropped with a warning.
    fn virtual_pointer_get_default_seat(&self) -> Option<Seat<Self>> {
        None
    }

    /// Handle a relative motion event from a virtual pointer.
    ///
    /// `delta` is the displacement in compositor-space logical coordinates.
    /// The compositor should add this to the current pointer position and update
    /// the pointer focus accordingly.
    fn virtual_pointer_motion(
        &mut self,
        seat: Option<&Seat<Self>>,
        time: u32,
        delta: Point<f64, Logical>,
    );

    /// Handle an absolute motion event from a virtual pointer.
    ///
    /// The `(x, y)` coordinates lie in the range `[0, x_extent)` x `[0, y_extent)`.
    /// The compositor should map these to compositor-space coordinates and update
    /// the pointer focus accordingly.
    fn virtual_pointer_motion_absolute(
        &mut self,
        seat: Option<&Seat<Self>>,
        time: u32,
        x: u32,
        y: u32,
        x_extent: u32,
        y_extent: u32,
    );
}

/// User data of a [`ZwlrVirtualPointerV1`] object.
pub struct VirtualPointerUserData<D: SeatHandler> {
    seat: Option<Seat<D>>,
    /// Axis events accumulate here until `frame` is received.
    pending_axis: Mutex<Option<AxisFrame>>,
}

impl<D: SeatHandler> fmt::Debug for VirtualPointerUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualPointerUserData")
            .field("pending_axis", &self.pending_axis)
            .finish()
    }
}

/// State of the wlr-virtual-pointer manager global.
#[derive(Debug)]
pub struct VirtualPointerManagerState {
    global: GlobalId,
}

/// Data associated with a [`VirtualPointerManagerState`] global.
#[allow(missing_debug_implementations)]
pub struct VirtualPointerManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl VirtualPointerManagerState {
    /// Initialize a new wlr-virtual-pointer manager global.
    ///
    /// The `filter` closure controls which clients are allowed to bind the global.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwlrVirtualPointerManagerV1, VirtualPointerManagerGlobalData>,
        D: Dispatch<ZwlrVirtualPointerManagerV1, GlobalData>,
        D: Dispatch<ZwlrVirtualPointerV1, VirtualPointerUserData<D>>,
        D: VirtualPointerHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = VirtualPointerManagerGlobalData {
            filter: Box::new(filter),
        };
        let global =
            display.create_global::<D, ZwlrVirtualPointerManagerV1, _>(MANAGER_VERSION, data);
        Self { global }
    }

    /// Get the [`GlobalId`] of the [`ZwlrVirtualPointerManagerV1`] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch2<ZwlrVirtualPointerManagerV1, D> for VirtualPointerManagerGlobalData
where
    D: Dispatch<ZwlrVirtualPointerManagerV1, GlobalData>,
    D: Dispatch<ZwlrVirtualPointerV1, VirtualPointerUserData<D>>,
    D: VirtualPointerHandler,
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrVirtualPointerManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<ZwlrVirtualPointerManagerV1, D> for GlobalData
where
    D: Dispatch<ZwlrVirtualPointerV1, VirtualPointerUserData<D>>,
    D: VirtualPointerHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        _manager: &ZwlrVirtualPointerManagerV1,
        request: zwlr_virtual_pointer_manager_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let (wl_seat, id) = match request {
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointer { seat, id } => {
                (seat, id)
            }
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointerWithOutput {
                seat,
                output: _,
                id,
            } => (seat, id),
            zwlr_virtual_pointer_manager_v1::Request::Destroy => return,
            _ => unreachable!(),
        };

        let seat = wl_seat.and_then(|s| Seat::<D>::from_resource(&s));
        data_init.init(
            id,
            VirtualPointerUserData {
                seat,
                pending_axis: Mutex::new(None),
            },
        );
    }
}

impl<D> Dispatch2<ZwlrVirtualPointerV1, D> for VirtualPointerUserData<D>
where
    D: VirtualPointerHandler + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        resource: &ZwlrVirtualPointerV1,
        request: zwlr_virtual_pointer_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let seat = self.seat.as_ref();
        // Fall back to compositor-supplied default when no seat was specified at
        // virtual pointer creation time (protocol allows seat = null).
        let default_seat = if seat.is_none() {
            state.virtual_pointer_get_default_seat()
        } else {
            None
        };
        let effective_seat = seat.or(default_seat.as_ref());

        match request {
            zwlr_virtual_pointer_v1::Request::Motion { time, dx, dy } => {
                debug!("virtual_pointer: motion dx={dx} dy={dy} time={time}");
                state.virtual_pointer_motion(effective_seat, time, Point::from((dx, dy)));
            }

            zwlr_virtual_pointer_v1::Request::MotionAbsolute {
                time,
                x,
                y,
                x_extent,
                y_extent,
            } => {
                debug!("virtual_pointer: motion_absolute x={x} y={y} x_extent={x_extent} y_extent={y_extent} time={time}");
                state.virtual_pointer_motion_absolute(effective_seat, time, x, y, x_extent, y_extent);
            }

            zwlr_virtual_pointer_v1::Request::Button {
                time,
                button,
                state: button_state,
            } => {
                let btn_state = match button_state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => ButtonState::Pressed,
                    WEnum::Value(wl_pointer::ButtonState::Released) => ButtonState::Released,
                    WEnum::Value(_) | WEnum::Unknown(_) => return,
                };
                debug!("virtual_pointer: button button={button:#010x} state={btn_state:?} time={time}");
                if let Some(seat) = effective_seat {
                    if let Some(ptr) = seat.get_pointer() {
                        ptr.button(
                            state,
                            &ButtonEvent {
                                serial: SERIAL_COUNTER.next_serial(),
                                time,
                                button,
                                state: btn_state,
                            },
                        );
                    }
                } else {
                    warn!("virtual_pointer: button event dropped - no seat associated with virtual pointer and no default seat provided");
                }
            }

            zwlr_virtual_pointer_v1::Request::Axis { time, axis, value } => {
                let axis = match to_axis(axis) {
                    Some(a) => a,
                    None => {
                        resource.post_error(VirtualPointerError::InvalidAxis, "invalid axis value");
                        return;
                    }
                };
                let mut pending = self.pending_axis.lock().unwrap();
                let frame = pending.get_or_insert_with(|| AxisFrame::new(time));
                frame.time = time;
                let updated = (*frame).value(axis, value);
                *frame = updated;
            }

            zwlr_virtual_pointer_v1::Request::Frame => {
                trace!("virtual_pointer: frame");
                let frame = self.pending_axis.lock().unwrap().take();
                if let Some(seat) = effective_seat {
                    if let Some(ptr) = seat.get_pointer() {
                        if let Some(frame) = frame {
                            debug!("virtual_pointer: dispatching axis frame {frame:?}");
                            ptr.axis(state, frame);
                        }
                        ptr.frame(state);
                    }
                } else if frame.is_some() {
                    warn!("virtual_pointer: axis frame dropped - no seat associated with virtual pointer and no default seat provided");
                }
            }

            zwlr_virtual_pointer_v1::Request::AxisSource { axis_source } => {
                let source = match to_axis_source(axis_source) {
                    Some(s) => s,
                    None => {
                        resource.post_error(
                            VirtualPointerError::InvalidAxisSource,
                            "invalid axis source value",
                        );
                        return;
                    }
                };
                let mut pending = self.pending_axis.lock().unwrap();
                let frame = pending.get_or_insert_with(|| AxisFrame::new(0));
                let updated = (*frame).source(source);
                *frame = updated;
            }

            zwlr_virtual_pointer_v1::Request::AxisStop { time, axis } => {
                let axis = match to_axis(axis) {
                    Some(a) => a,
                    None => {
                        resource.post_error(VirtualPointerError::InvalidAxis, "invalid axis value");
                        return;
                    }
                };
                let mut pending = self.pending_axis.lock().unwrap();
                let frame = pending.get_or_insert_with(|| AxisFrame::new(time));
                frame.time = time;
                let updated = (*frame).stop(axis);
                *frame = updated;
            }

            zwlr_virtual_pointer_v1::Request::AxisDiscrete {
                time,
                axis,
                value,
                discrete,
            } => {
                let axis = match to_axis(axis) {
                    Some(a) => a,
                    None => {
                        resource.post_error(VirtualPointerError::InvalidAxis, "invalid axis value");
                        return;
                    }
                };
                let mut pending = self.pending_axis.lock().unwrap();
                let frame = pending.get_or_insert_with(|| AxisFrame::new(time));
                frame.time = time;
                // axis_discrete sends a legacy discrete step count (1 per detent).
                // v120 uses 120 units per detent, so multiply accordingly.
                let updated = (*frame).value(axis, value).v120(axis, discrete * 120);
                *frame = updated;
            }

            zwlr_virtual_pointer_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

fn to_axis(axis: WEnum<wl_pointer::Axis>) -> Option<Axis> {
    match axis {
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => Some(Axis::Vertical),
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Some(Axis::Horizontal),
        _ => None,
    }
}

fn to_axis_source(source: WEnum<wl_pointer::AxisSource>) -> Option<AxisSource> {
    match source {
        WEnum::Value(wl_pointer::AxisSource::Wheel) => Some(AxisSource::Wheel),
        WEnum::Value(wl_pointer::AxisSource::Finger) => Some(AxisSource::Finger),
        WEnum::Value(wl_pointer::AxisSource::Continuous) => Some(AxisSource::Continuous),
        WEnum::Value(wl_pointer::AxisSource::WheelTilt) => Some(AxisSource::WheelTilt),
        _ => None,
    }
}
