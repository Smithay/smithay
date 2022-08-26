use wayland_backend::server::GlobalId;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::{
    zwp_virtual_keyboard_manager_v1::{self, ZwpVirtualKeyboardManagerV1},
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::input::{Seat, SeatHandler};

use self::virtual_keyboard_handle::VirtualKeyboardHandle;

const MANAGER_VERSION: u32 = 1;

mod virtual_keyboard_handle;

pub use virtual_keyboard_handle::VirtualKeyboardUserData;

/// State of wp misc virtual keyboard protocol
#[derive(Debug)]
pub struct VirtualKeyboardManagerState {
    global: GlobalId,
}

/// Data associated with a VirtualKeyboardManager global.
#[allow(missing_debug_implementations)]
pub struct VirtualKeyboardManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

fn create_global_with_filter<D, F>(display: &DisplayHandle, filter: F) -> GlobalId
where
    D: GlobalDispatch<ZwpVirtualKeyboardManagerV1, VirtualKeyboardManagerGlobalData> + 'static,
    F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
{
    let data = VirtualKeyboardManagerGlobalData {
        filter: Box::new(filter),
    };

    display.create_global::<D, ZwpVirtualKeyboardManagerV1, _>(MANAGER_VERSION, data)
}

impl VirtualKeyboardManagerState {
    /// Initialize a virtual keyboard manager global.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwpVirtualKeyboardManagerV1, VirtualKeyboardManagerGlobalData>,
        D: Dispatch<ZwpVirtualKeyboardManagerV1, ()>,
        D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData<D>>,
        D: SeatHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = create_global_with_filter::<D, F>(display, filter);

        Self { global }
    }

    /// Get the id of ZwpVirtualKeyboardManagerV1 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpVirtualKeyboardManagerV1, VirtualKeyboardManagerGlobalData, D>
    for VirtualKeyboardManagerState
where
    D: GlobalDispatch<ZwpVirtualKeyboardManagerV1, VirtualKeyboardManagerGlobalData>,
    D: Dispatch<ZwpVirtualKeyboardManagerV1, ()>,
    D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpVirtualKeyboardManagerV1>,
        _: &VirtualKeyboardManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &VirtualKeyboardManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZwpVirtualKeyboardManagerV1, (), D> for VirtualKeyboardManagerState
where
    D: Dispatch<ZwpVirtualKeyboardManagerV1, ()>,
    D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ZwpVirtualKeyboardManagerV1,
        request: zwp_virtual_keyboard_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_manager_v1::Request::CreateVirtualKeyboard { seat, id } => {
                let seat = Seat::<D>::from_resource(&seat).unwrap();
                let user_data = seat.user_data();
                user_data.insert_if_missing(VirtualKeyboardHandle::default);
                let virtual_keyboard_handle = user_data.get::<VirtualKeyboardHandle>().unwrap();
                let instance = data_init.init(
                    id,
                    VirtualKeyboardUserData {
                        handle: virtual_keyboard_handle.clone(),
                        seat: seat.clone(),
                    },
                );

                virtual_keyboard_handle.add_instance::<D>(&instance);
            }
            _ => unreachable!(),
        }
    }
}

#[allow(missing_docs)] //TODO
#[macro_export]
macro_rules! delegate_virtual_keyboard_manager {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1: $crate::wayland::virtual_keyboard::VirtualKeyboardManagerGlobalData
        ] => $crate::wayland::virtual_keyboard::VirtualKeyboardManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1: ()
        ] => $crate::wayland::virtual_keyboard::VirtualKeyboardManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1: $crate::wayland::virtual_keyboard::VirtualKeyboardUserData<Self>
        ] => $crate::wayland::virtual_keyboard::VirtualKeyboardManagerState);
    };
}
