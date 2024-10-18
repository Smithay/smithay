use wayland_protocols::wp::fifo::v1::server::{wp_fifo_manager_v1, wp_fifo_v1};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource, WEnum, Weak,
};

pub struct FifoState {}

impl FifoState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<wp_fifo_manager_v1::WpFifoManagerV1, FifoManagerGlobalData>,
        D: Dispatch<wp_fifo_manager_v1::WpFifoManagerV1, ()>,
        D: Dispatch<wp_fifo_v1::WpFifoV1, FifoData>,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = FifoManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, wp_fifo_manager_v1::WpFifoManagerV1, _>(1, data);

        Self {}
    }
}

#[allow(missing_debug_implementations)]
#[doc(hidden)]
pub struct FifoManagerGlobalData {
    /// Filter whether the clients can view global.
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl<D> GlobalDispatch<wp_fifo_manager_v1::WpFifoManagerV1, FifoManagerGlobalData, D> for FifoState
where
    D: GlobalDispatch<wp_fifo_manager_v1::WpFifoManagerV1, FifoManagerGlobalData>,
    D: Dispatch<wp_fifo_manager_v1::WpFifoManagerV1, ()>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<wp_fifo_manager_v1::WpFifoManagerV1>,
        _global_data: &FifoManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }

    fn can_view(client: Client, global_data: &FifoManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<wp_fifo_manager_v1::WpFifoManagerV1, (), D> for FifoState
where
    D: Dispatch<wp_fifo_manager_v1::WpFifoManagerV1, ()>,
    D: Dispatch<wp_fifo_v1::WpFifoV1, FifoData>,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _proxy: &wp_fifo_manager_v1::WpFifoManagerV1,
        request: wp_fifo_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_fifo_manager_v1::Request::GetFifo { id, surface } => {
                // TODO AlreadyExists
                data_init.init(
                    id,
                    FifoData {
                        surface: surface.downgrade(),
                    },
                );
            }
            wp_fifo_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

#[doc(hidden)]
pub struct FifoData {
    surface: Weak<wl_surface::WlSurface>,
}

impl<D> Dispatch<wp_fifo_v1::WpFifoV1, FifoData, D> for FifoState
where
    D: Dispatch<wp_fifo_v1::WpFifoV1, FifoData>,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _proxy: &wp_fifo_v1::WpFifoV1,
        request: wp_fifo_v1::Request,
        _data: &FifoData,
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_fifo_v1::Request::SetBarrier => {}
            wp_fifo_v1::Request::WaitBarrier => {}
            wp_fifo_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

#[allow(missing_docs)]
#[macro_export]
macro_rules! delegate_fifo {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fifo::v1::server::wp_fifo_manager_v1::WpFifoManagerV1: $crate::wayland::fifo::FifoManagerGlobalData
        ] => $crate::wayland::fifo::FifoState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fifo::v1::server::wp_fifo_manager_v1::WpFifoManagerV1: ()
        ] => $crate::wayland::fifo::FifoState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fifo::v1::server::wp_fifo_v1::WpFifoV1: $crate::wayland::fifo::FifoData
        ] => $crate::wayland::fifo::FifoState);
    };
}
