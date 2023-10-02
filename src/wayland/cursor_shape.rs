//! Utilities for compositor side cursor theming support
//!
//! This protocol allows clients to request compositor to draw a cursor for them, thus resulting
//! in more consistent look and feel of the cursor across the applications.
//!
//! ## Initialization
//!
//! ```
//! extern crate smithay;
//! extern crate wayland_server;
//!
//! use smithay::wayland::cursor_shape::CursorShapeManagerState;
//! use smithay::delegate_cursor_shape;
//!
//! # use smithay::backend::input::KeyState;
//! # use smithay::input::{
//! #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent, RelativeMotionEvent,
//! #             GestureSwipeBeginEvent, GestureSwipeUpdateEvent, GestureSwipeEndEvent,
//! #             GesturePinchBeginEvent, GesturePinchUpdateEvent, GesturePinchEndEvent,
//! #             GestureHoldBeginEvent, GestureHoldEndEvent},
//! #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
//! #   Seat, SeatHandler, SeatState,
//! # };
//! # use smithay::utils::{IsAlive, Serial};
//! # use smithay::wayland::seat::WaylandFocus;
//! # use wayland_server::protocol::wl_surface;
//!
//! # #[derive(Debug, Clone, PartialEq)]
//! # struct Target;
//! # impl IsAlive for Target {
//! #   fn alive(&self) -> bool { true }
//! # }
//! # impl WaylandFocus for Target {
//! #   fn wl_surface(&self) -> Option<wl_surface::WlSurface> {
//! #       None
//! #   }
//! # }
//!
//! # impl PointerTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {}
//! #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
//! #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
//! #   fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {}
//! #   fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {}
//! #   fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {}
//! #   fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {}
//! #   fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {}
//! #   fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {}
//! #   fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {}
//! #   fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {}
//! # }
//! # impl KeyboardTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {}
//! #   fn key(
//! #       &self,
//! #       seat: &Seat<State>,
//! #       data: &mut State,
//! #       key: KeysymHandle<'_>,
//! #       state: KeyState,
//! #       serial: Serial,
//! #       time: u32,
//! #   ) {}
//! #   fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {}
//! # }
//! # struct State {
//! #     seat_state: SeatState<Self>,
//! # };
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # impl SeatHandler for State {
//! #     type KeyboardFocus = Target;
//! #     type PointerFocus = Target;
//! #
//! #     fn seat_state(&mut self) -> &mut SeatState<Self> {
//! #         &mut self.seat_state
//! #     }
//! # }
//!
//! let state = CursorShapeManagerState::new::<State>(&display.handle());
//!
//! delegate_cursor_shape!(State);
//! ```

use wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_device_v1::Request as ShapeRequest;
use wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_device_v1::Shape;
use wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1 as CursorShapeDevice;
use wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_manager_v1::Request as ManagerRequest;
use wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1 as CursorShapeManager;
use wayland_protocols::wp::tablet::zv2::server::zwp_tablet_tool_v2::ZwpTabletToolV2;
use wayland_server::protocol::wl_pointer::WlPointer;
use wayland_server::GlobalDispatch;
use wayland_server::Resource;
use wayland_server::WEnum;
use wayland_server::{backend::GlobalId, Dispatch, DisplayHandle};

use crate::input::pointer::{CursorIcon, CursorImageStatus};
use crate::input::SeatHandler;
use crate::wayland::seat::WaylandFocus;

use super::seat::PointerUserData;
use super::tablet_manager::TabletToolUserData;

/// State of the cursor shape manager.
#[derive(Debug)]
pub struct CursorShapeManagerState {
    global: GlobalId,
}

impl CursorShapeManagerState {
    /// Register new [CursorShapeManager] global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<CursorShapeManager, ()>,
        D: Dispatch<CursorShapeManager, ()>,
        D: SeatHandler,
        D: 'static,
    {
        let global = display.create_global::<D, CursorShapeManager, _>(1, ());
        Self { global }
    }

    /// [CursorShapeManager] GlobalId getter.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<CursorShapeManager, (), D> for CursorShapeManagerState
where
    D: GlobalDispatch<CursorShapeManager, ()>,
    D: Dispatch<CursorShapeManager, ()>,
    D: SeatHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<CursorShapeManager>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<CursorShapeManager, (), D> for CursorShapeManagerState
where
    D: Dispatch<CursorShapeManager, ()>,
    D: Dispatch<CursorShapeDevice, CursorShapeDeviceUserData>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &CursorShapeManager,
        request: <CursorShapeManager as wayland_server::Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ManagerRequest::GetPointer {
                cursor_shape_device,
                pointer,
            } => {
                data_init.init(
                    cursor_shape_device,
                    CursorShapeDeviceUserData(CursorShapeDeviceUserDataInner::Pointer(pointer)),
                );
            }
            ManagerRequest::GetTabletToolV2 {
                cursor_shape_device,
                tablet_tool,
            } => {
                data_init.init(
                    cursor_shape_device,
                    CursorShapeDeviceUserData(CursorShapeDeviceUserDataInner::Tablet(tablet_tool)),
                );
            }
            ManagerRequest::Destroy => {}
            _ => unreachable!(),
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct CursorShapeDeviceUserData(CursorShapeDeviceUserDataInner);

#[derive(Debug, Clone)]
pub(crate) enum CursorShapeDeviceUserDataInner {
    /// The device was created for the pointer.
    Pointer(WlPointer),
    /// The device was created for the tablet tool.
    Tablet(ZwpTabletToolV2),
}

impl<D> Dispatch<CursorShapeDevice, CursorShapeDeviceUserData, D> for CursorShapeManagerState
where
    D: Dispatch<CursorShapeManager, ()>,
    D: Dispatch<CursorShapeDevice, CursorShapeDeviceUserData>,
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &CursorShapeDevice,
        request: <CursorShapeDevice as wayland_server::Resource>::Request,
        data: &CursorShapeDeviceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ShapeRequest::SetShape {
                serial,
                shape: WEnum::Value(shape),
            } => {
                match &data.0 {
                    CursorShapeDeviceUserDataInner::Pointer(pointer) => {
                        let pointer_data = pointer.data::<PointerUserData<D>>();
                        let handle = match pointer_data.and_then(|data| data.handle.as_ref()) {
                            Some(handle) => handle,
                            None => return,
                        };

                        // Ignore mismatches in serial.
                        if !handle
                            .last_enter
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|last_serial| last_serial.0 == serial)
                            .unwrap_or(false)
                        {
                            return;
                        }

                        // Check that pointer focus matches.
                        if !handle
                            .current_focus()
                            .map(|focus| focus.same_client_as(&pointer.id()))
                            .unwrap_or(false)
                        {
                            return;
                        }

                        let seat = state
                            .seat_state()
                            .seats
                            .iter()
                            .find(|seat| seat.get_pointer().map(|h| &h == handle).unwrap_or(false))
                            .cloned();

                        if let Some(seat) = seat {
                            let cursor_icon = shape_to_cursor_icon(shape);
                            state.cursor_image(&seat, CursorImageStatus::Named(cursor_icon));
                        }
                    }
                    CursorShapeDeviceUserDataInner::Tablet(tablet) => {
                        let tablet_data = match tablet.data::<TabletToolUserData>() {
                            Some(data) => data,
                            None => return,
                        };

                        // Check that tablet focus matches.
                        if !tablet_data
                            .handle
                            .inner
                            .lock()
                            .unwrap()
                            .focus
                            .as_ref()
                            .map(|focus| focus.same_client_as(&tablet.id()))
                            .unwrap_or(false)
                        {
                            return;
                        }

                        let cursor_icon = shape_to_cursor_icon(shape);
                        (tablet_data.cb.lock().unwrap())(
                            &tablet_data.desc,
                            CursorImageStatus::Named(cursor_icon),
                        );
                    }
                }
            }
            ShapeRequest::SetShape { .. } => {
                // Ignore unknown shapes.
            }
            ShapeRequest::Destroy => {}
            _ => unreachable!(),
        }
    }
}

fn shape_to_cursor_icon(shape: Shape) -> CursorIcon {
    match shape {
        Shape::Default => CursorIcon::Default,
        Shape::ContextMenu => CursorIcon::ContextMenu,
        Shape::Help => CursorIcon::Help,
        Shape::Pointer => CursorIcon::Pointer,
        Shape::Progress => CursorIcon::Progress,
        Shape::Wait => CursorIcon::Wait,
        Shape::Cell => CursorIcon::Cell,
        Shape::Crosshair => CursorIcon::Crosshair,
        Shape::Text => CursorIcon::Text,
        Shape::VerticalText => CursorIcon::VerticalText,
        Shape::Alias => CursorIcon::Alias,
        Shape::Copy => CursorIcon::Copy,
        Shape::Move => CursorIcon::Move,
        Shape::NoDrop => CursorIcon::NoDrop,
        Shape::NotAllowed => CursorIcon::NotAllowed,
        Shape::Grab => CursorIcon::Grab,
        Shape::Grabbing => CursorIcon::Grabbing,
        Shape::EResize => CursorIcon::EResize,
        Shape::NResize => CursorIcon::NResize,
        Shape::NeResize => CursorIcon::NeResize,
        Shape::NwResize => CursorIcon::NwResize,
        Shape::SResize => CursorIcon::SResize,
        Shape::SeResize => CursorIcon::SeResize,
        Shape::SwResize => CursorIcon::SwResize,
        Shape::WResize => CursorIcon::WResize,
        Shape::EwResize => CursorIcon::EwResize,
        Shape::NsResize => CursorIcon::NsResize,
        Shape::NeswResize => CursorIcon::NeswResize,
        Shape::NwseResize => CursorIcon::NwseResize,
        Shape::ColResize => CursorIcon::ColResize,
        Shape::RowResize => CursorIcon::RowResize,
        Shape::AllScroll => CursorIcon::AllScroll,
        Shape::ZoomIn => CursorIcon::ZoomIn,
        Shape::ZoomOut => CursorIcon::ZoomOut,
        _ => CursorIcon::Default,
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_cursor_shape {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1: ()
        ] => $crate::wayland::cursor_shape::CursorShapeManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1: ()
        ] => $crate::wayland::cursor_shape::CursorShapeManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1: $crate::wayland::cursor_shape::CursorShapeDeviceUserData
        ] => $crate::wayland::cursor_shape::CursorShapeManagerState);
    };
}
