//! Implementation of the `zwlr_virtual_pointer_v1` protocol.
//!
//! A virtual pointer is exposed as a regular input device. Requests are
//! converted into [`VirtualPointerBackend`] [`InputEvent`]s and passed to
//! [`VirtualPointerHandler::process_virtual_pointer_event`] so synthetic input
//! is handled the same way as a real
//! [`InputBackend`](crate::backend::input::InputBackend).
//!
//! The device exists for as long as the client's pointer object does.
//!
//! Motion and button requests are forwarded as they arrive. Axis requests are
//! buffered until `frame` (since a scroll needs the separately sent value,
//! steps, and source), and are emitted as a single [`InputEvent::PointerAxis`].
//!
//! ```
//! use smithay::backend::input::InputEvent;
//! use smithay::wayland::virtual_pointer::{
//!     VirtualPointerManagerState, VirtualPointerBackend, VirtualPointerHandler,
//! };
//! # use smithay::reexports::wayland_server::Display;
//!
//! # struct State;
//!
//! smithay::delegate_dispatch2!(State);
//!
//! # let mut display = Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! impl VirtualPointerHandler for State {
//!     fn process_virtual_pointer_event(&mut self, event: InputEvent<VirtualPointerBackend>) {
//!         // process_input_event(event);
//!     }
//! }
//!
//! // The function decides which clients may create pointers.
//! VirtualPointerManagerState::new::<State, _>(&display_handle, |_client| true);
//! ```

use std::sync::{Arc, Mutex};

use tracing::debug;
use wayland_protocols_wlr::virtual_pointer::v1::server::{
    zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
    zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1},
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum, backend::ClientId,
    backend::GlobalId, protocol::wl_pointer, protocol::wl_seat::WlSeat,
};

use crate::backend::input::{Axis, AxisSource, ButtonState, InputEvent, InputTime};
use crate::output::{Output, WeakOutput};
use crate::wayland::{Dispatch2, GlobalData, GlobalDispatch2};

const MANAGER_VERSION: u32 = 2;

mod input;

use input::{PendingFrame, axis_index};
pub use input::{
    VirtualPointerAxisEvent, VirtualPointerBackend, VirtualPointerButtonEvent, VirtualPointerDevice,
    VirtualPointerMotionAbsoluteEvent, VirtualPointerMotionEvent,
};

/// Handler for virtual pointer input events.
pub trait VirtualPointerHandler {
    /// Handle an event from a virtual pointer.
    ///
    /// Process it the same way as any other
    /// [`InputBackend`](crate::backend::input::InputBackend).
    fn process_virtual_pointer_event(&mut self, event: InputEvent<VirtualPointerBackend>);
}

/// State of wlr virtual pointer protocol
#[derive(Debug)]
pub struct VirtualPointerManagerState {
    global: GlobalId,
}

/// Data associated with a VirtualPointerManager global.
#[allow(missing_debug_implementations)]
pub struct VirtualPointerManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl VirtualPointerManagerState {
    /// Initialize a virtual pointer manager global.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwlrVirtualPointerManagerV1, VirtualPointerManagerGlobalData>,
        D: Dispatch<ZwlrVirtualPointerManagerV1, GlobalData>,
        D: Dispatch<ZwlrVirtualPointerV1, VirtualPointerUserData>,
        D: VirtualPointerHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = VirtualPointerManagerGlobalData {
            filter: Box::new(filter),
        };
        let global = display.create_global::<D, ZwlrVirtualPointerManagerV1, _>(MANAGER_VERSION, data);

        Self { global }
    }

    /// Get the id of ZwlrVirtualPointerManagerV1 global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch2<ZwlrVirtualPointerManagerV1, D> for VirtualPointerManagerGlobalData
where
    D: Dispatch<ZwlrVirtualPointerManagerV1, GlobalData>,
    D: Dispatch<ZwlrVirtualPointerV1, VirtualPointerUserData>,
    D: VirtualPointerHandler,
    D: 'static,
{
    fn bind(
        &self,
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
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
    D: Dispatch<ZwlrVirtualPointerV1, VirtualPointerUserData>,
    D: VirtualPointerHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _resource: &ZwlrVirtualPointerManagerV1,
        request: zwlr_virtual_pointer_manager_v1::Request,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let (id, seat, output) = match request {
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointer { seat, id } => (id, seat, None),
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointerWithOutput { seat, output, id } => {
                let output = output.as_ref().and_then(|output| {
                    let resolved = Output::from_resource(output).map(|output| output.downgrade());
                    if resolved.is_none() {
                        debug!("Virtual pointer requested an unknown output, using the whole layout");
                    }
                    resolved
                });
                (id, seat, output)
            }
            zwlr_virtual_pointer_manager_v1::Request::Destroy => return,
            _ => unreachable!(),
        };

        let data = Arc::new(VirtualPointerData {
            seat,
            output,
            frame: Mutex::new(PendingFrame::default()),
            pressed_buttons: Mutex::new(Vec::new()),
        });
        let pointer = data_init.init(id, VirtualPointerUserData { data: data.clone() });

        state.process_virtual_pointer_event(InputEvent::DeviceAdded {
            device: VirtualPointerDevice { pointer, data },
        });
    }
}

/// User data for ZwlrVirtualPointerV1
#[derive(Debug)]
pub struct VirtualPointerUserData {
    data: Arc<VirtualPointerData>,
}

#[derive(Debug)]
struct VirtualPointerData {
    seat: Option<WlSeat>,
    output: Option<WeakOutput>,
    frame: Mutex<PendingFrame>,
    pressed_buttons: Mutex<Vec<u32>>,
}

impl VirtualPointerUserData {
    fn device(&self, pointer: &ZwlrVirtualPointerV1) -> VirtualPointerDevice {
        VirtualPointerDevice {
            pointer: pointer.clone(),
            data: self.data.clone(),
        }
    }
}

fn parse_axis(virtual_pointer: &ZwlrVirtualPointerV1, axis: WEnum<wl_pointer::Axis>) -> Option<Axis> {
    match axis {
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Some(Axis::Horizontal),
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => Some(Axis::Vertical),
        _ => {
            virtual_pointer.post_error(zwlr_virtual_pointer_v1::Error::InvalidAxis, "invalid axis");
            None
        }
    }
}

impl<D> Dispatch2<ZwlrVirtualPointerV1, D> for VirtualPointerUserData
where
    D: VirtualPointerHandler + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        virtual_pointer: &ZwlrVirtualPointerV1,
        request: zwlr_virtual_pointer_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwlr_virtual_pointer_v1::Request::Motion { time, dx, dy } => {
                state.process_virtual_pointer_event(InputEvent::PointerMotion {
                    event: VirtualPointerMotionEvent {
                        device: self.device(virtual_pointer),
                        time,
                        dx,
                        dy,
                    },
                });
            }
            zwlr_virtual_pointer_v1::Request::MotionAbsolute {
                time,
                x,
                y,
                x_extent,
                y_extent,
            } => {
                if x_extent == 0 || y_extent == 0 {
                    debug!("Dropping virtual pointer absolute motion with zero extents");
                    return;
                }

                state.process_virtual_pointer_event(InputEvent::PointerMotionAbsolute {
                    event: VirtualPointerMotionAbsoluteEvent {
                        device: self.device(virtual_pointer),
                        time,
                        x,
                        y,
                        x_extent,
                        y_extent,
                    },
                });
            }
            zwlr_virtual_pointer_v1::Request::Button {
                time,
                button,
                state: button_state,
            } => {
                // The protocol defines this as an enum, but wlroots treats it
                // as a C boolean (zero or nonzero), so any invalid value counts
                // as pressed.
                let button_state = match button_state {
                    WEnum::Value(wl_pointer::ButtonState::Released) => ButtonState::Released,
                    _ => ButtonState::Pressed,
                };

                // Tracked so `destroyed` can release held buttons.
                {
                    let mut pressed = self.data.pressed_buttons.lock().unwrap();
                    match button_state {
                        ButtonState::Pressed => {
                            if !pressed.contains(&button) {
                                pressed.push(button);
                            }
                        }
                        ButtonState::Released => pressed.retain(|pressed| *pressed != button),
                    }
                }

                state.process_virtual_pointer_event(InputEvent::PointerButton {
                    event: VirtualPointerButtonEvent {
                        device: self.device(virtual_pointer),
                        time,
                        button,
                        state: button_state,
                    },
                });
            }
            zwlr_virtual_pointer_v1::Request::Axis { time, axis, value } => {
                let Some(axis) = parse_axis(virtual_pointer, axis) else {
                    return;
                };

                let mut frame = self.data.frame.lock().unwrap();
                frame.time = frame.time.max(time);
                frame.axes[axis_index(axis)]
                    .get_or_insert_with(Default::default)
                    .value = value;
            }
            zwlr_virtual_pointer_v1::Request::AxisSource { axis_source } => {
                let axis_source = match axis_source {
                    WEnum::Value(wl_pointer::AxisSource::Wheel) => AxisSource::Wheel,
                    WEnum::Value(wl_pointer::AxisSource::Finger) => AxisSource::Finger,
                    WEnum::Value(wl_pointer::AxisSource::Continuous) => AxisSource::Continuous,
                    WEnum::Value(wl_pointer::AxisSource::WheelTilt) => AxisSource::WheelTilt,
                    _ => {
                        virtual_pointer.post_error(
                            zwlr_virtual_pointer_v1::Error::InvalidAxisSource,
                            "invalid axis source",
                        );
                        return;
                    }
                };

                self.data.frame.lock().unwrap().source = Some(axis_source);
            }
            zwlr_virtual_pointer_v1::Request::AxisStop { time, axis } => {
                let Some(axis) = parse_axis(virtual_pointer, axis) else {
                    return;
                };

                let mut frame = self.data.frame.lock().unwrap();
                frame.time = frame.time.max(time);
                // A zero value without any discrete steps is a stop.
                frame.axes[axis_index(axis)] = Some(Default::default());
            }
            zwlr_virtual_pointer_v1::Request::AxisDiscrete {
                time,
                axis,
                value,
                discrete,
            } => {
                let Some(axis) = parse_axis(virtual_pointer, axis) else {
                    return;
                };

                let mut frame = self.data.frame.lock().unwrap();
                frame.time = frame.time.max(time);
                let pending = frame.axes[axis_index(axis)].get_or_insert_with(Default::default);
                pending.value = value;
                pending.v120 = Some(discrete * 120);
            }
            zwlr_virtual_pointer_v1::Request::Frame => {
                let frame = std::mem::take(&mut *self.data.frame.lock().unwrap());
                if frame.axes.iter().any(Option::is_some) {
                    state.process_virtual_pointer_event(InputEvent::PointerAxis {
                        event: VirtualPointerAxisEvent {
                            device: self.device(virtual_pointer),
                            frame,
                        },
                    });
                }
            }
            zwlr_virtual_pointer_v1::Request::Destroy => {
                // no-op; the device is removed in `destroyed`
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, virtual_pointer: &ZwlrVirtualPointerV1) {
        let device = self.device(virtual_pointer);

        // A client may destroy the mouse, or die, with buttons still down.
        // Release them here like how libinput does with an unplugged mouse.
        let pressed = std::mem::take(&mut *self.data.pressed_buttons.lock().unwrap());
        let time = InputTime::now().millis();
        for button in pressed.into_iter().rev() {
            state.process_virtual_pointer_event(InputEvent::PointerButton {
                event: VirtualPointerButtonEvent {
                    device: device.clone(),
                    time,
                    button,
                    state: ButtonState::Released,
                },
            });
        }

        state.process_virtual_pointer_event(InputEvent::DeviceRemoved { device });
    }
}
