//! Automatic handling of the `wlr_data_control` protocol
//!
//! ## Initialization
//!
//! To initialize this implementation, create [`DataControlState`], store it in your `State`
//! struct, and implement the required trait, as shown in the example:
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! use smithay::wayland::selection::SelectionHandler;
//! use smithay::wayland::selection::wlr_data_control::{DataControlState, DataControlHandler};
//! # use smithay::input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus};
//! # use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
//!
//! # struct State { data_control_state: DataControlState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the data_control state
//! let data_control_state = DataControlState::new::<State, _>(
//!     &display.handle(), None, |_| true
//! );
//!
//! // insert the DataControlState into your state
//! // ..
//!
//! // implement the necessary traits
//! # impl SeatHandler for State {
//! #     type KeyboardFocus = WlSurface;
//! #     type PointerFocus = WlSurface;
//! #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
//! #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//! #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! # }
//! impl SelectionHandler for State {
//!     type SelectionUserData = ();
//! }
//! impl DataControlHandler for State {
//!     fn data_control_state(&self) -> &DataControlState { &self.data_control_state }
//!     // ... override default implementations here to customize handling ...
//! }
//! delegate_data_control!(State);
//!
//! // You're now ready to go!
//! ```
//!
//! Be aware that data control clients rely on other selection providers to be implemneted, like
//! wl_data_device or zwp_primary_selection.

use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_server::backend::GlobalId;
use wayland_server::{Client, DisplayHandle, GlobalDispatch};

mod device;
mod source;

pub use device::DataControlDeviceUserData;
pub use source::DataControlSourceUserData;

use super::primary_selection::PrimarySelectionState;
use super::SelectionHandler;

/// Access the data control state.
pub trait DataControlHandler: Sized + SelectionHandler {
    /// [`DataControlState`] getter.
    fn data_control_state(&self) -> &DataControlState;
}

/// State of the data control.
#[derive(Debug)]
pub struct DataControlState {
    manager_global: GlobalId,
}

impl DataControlState {
    /// Register new [ZwlrDataControlManagerV1] global.
    ///
    /// Passing `primary_selection` will enable support for primary selection as well.
    pub fn new<D, F>(
        display: &DisplayHandle,
        primary_selection: Option<&PrimarySelectionState>,
        filter: F,
    ) -> Self
    where
        D: GlobalDispatch<ZwlrDataControlManagerV1, DataControlManagerGlobalData> + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = DataControlManagerGlobalData {
            primary: primary_selection.is_some(),
            filter: Box::new(filter),
        };
        let manager_global = display.create_global::<D, ZwlrDataControlManagerV1, _>(2, data);
        Self { manager_global }
    }

    /// [ZwlrDataControlManagerV1]  GlobalId getter.
    pub fn global(&self) -> GlobalId {
        self.manager_global.clone()
    }
}

#[allow(missing_debug_implementations)]
#[doc(hidden)]
pub struct DataControlManagerGlobalData {
    /// Whether to allow primary selection.
    primary: bool,

    /// Filter whether the clients can view global.
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct DataControlManagerUserData {
    /// Whether to allow primary selection.
    primary: bool,
}

mod handlers {
    use std::cell::RefCell;

    use tracing::error;
    use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
    use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_manager_v1;
    use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
    use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_source_v1::ZwlrDataControlSourceV1;
    use wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch};

    use crate::input::Seat;
    use crate::wayland::selection::device::SelectionDevice;
    use crate::wayland::selection::seat_data::SeatData;
    use crate::wayland::selection::SelectionTarget;

    use super::DataControlDeviceUserData;
    use super::DataControlHandler;
    use super::DataControlManagerGlobalData;
    use super::DataControlManagerUserData;
    use super::DataControlSourceUserData;
    use super::DataControlState;

    impl<D> GlobalDispatch<ZwlrDataControlManagerV1, DataControlManagerGlobalData, D> for DataControlState
    where
        D: GlobalDispatch<ZwlrDataControlManagerV1, DataControlManagerGlobalData>,
        D: Dispatch<ZwlrDataControlManagerV1, DataControlManagerUserData>,
        D: Dispatch<ZwlrDataControlDeviceV1, DataControlDeviceUserData>,
        D: Dispatch<ZwlrDataControlSourceV1, DataControlSourceUserData>,
        D: DataControlHandler,
        D: 'static,
    {
        fn bind(
            _state: &mut D,
            _handle: &DisplayHandle,
            _client: &wayland_server::Client,
            resource: wayland_server::New<ZwlrDataControlManagerV1>,
            global_data: &DataControlManagerGlobalData,
            data_init: &mut wayland_server::DataInit<'_, D>,
        ) {
            data_init.init(
                resource,
                DataControlManagerUserData {
                    primary: global_data.primary,
                },
            );
        }

        fn can_view(client: Client, global_data: &DataControlManagerGlobalData) -> bool {
            (global_data.filter)(&client)
        }
    }

    impl<D> Dispatch<ZwlrDataControlManagerV1, DataControlManagerUserData, D> for DataControlState
    where
        D: Dispatch<ZwlrDataControlManagerV1, DataControlManagerUserData>,
        D: Dispatch<ZwlrDataControlDeviceV1, DataControlDeviceUserData>,
        D: Dispatch<ZwlrDataControlSourceV1, DataControlSourceUserData>,
        D: DataControlHandler,
        D: 'static,
    {
        fn request(
            _handler: &mut D,
            client: &wayland_server::Client,
            _resource: &ZwlrDataControlManagerV1,
            request: <ZwlrDataControlManagerV1 as wayland_server::Resource>::Request,
            data: &DataControlManagerUserData,
            dh: &DisplayHandle,
            data_init: &mut wayland_server::DataInit<'_, D>,
        ) {
            match request {
                zwlr_data_control_manager_v1::Request::CreateDataSource { id } => {
                    data_init.init(id, DataControlSourceUserData::new());
                }
                zwlr_data_control_manager_v1::Request::GetDataDevice { id, seat: wl_seat } => {
                    match Seat::<D>::from_resource(&wl_seat) {
                        Some(seat) => {
                            seat.user_data()
                                .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));

                            let device = SelectionDevice::DataControl(data_init.init(
                                id,
                                DataControlDeviceUserData {
                                    wl_seat,
                                    primary: data.primary,
                                },
                            ));

                            let mut seat_data = seat
                                .user_data()
                                .get::<RefCell<SeatData<D::SelectionUserData>>>()
                                .unwrap()
                                .borrow_mut();

                            seat_data.add_device(device.clone());

                            // NOTE: broadcast selection only to the newly created device.
                            let device = Some(&device);
                            seat_data.send_selection::<D>(dh, SelectionTarget::Clipboard, device, true);
                            if data.primary {
                                seat_data.send_selection::<D>(dh, SelectionTarget::Primary, device, true);
                            }
                        }
                        None => {
                            error!(
                                data_control_device = ?id,
                                client = ?client,
                                "Unmanaged seat given to a primary selection device."
                            );
                        }
                    }
                }
                zwlr_data_control_manager_v1::Request::Destroy => (),
                _ => unreachable!(),
            }
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_data_control {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1: $crate::wayland::selection::wlr_data_control::DataControlManagerGlobalData
        ] => $crate::wayland::selection::wlr_data_control::DataControlState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1: $crate::wayland::selection::wlr_data_control::DataControlManagerUserData
        ] => $crate::wayland::selection::wlr_data_control::DataControlState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1: $crate::wayland::selection::wlr_data_control::DataControlDeviceUserData
        ] => $crate::wayland::selection::wlr_data_control::DataControlState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_source_v1::ZwlrDataControlSourceV1: $crate::wayland::selection::wlr_data_control::DataControlSourceUserData
        ] => $crate::wayland::selection::wlr_data_control::DataControlState);
    };
}
