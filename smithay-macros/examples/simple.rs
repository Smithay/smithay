use ::smithay::wayland::{buffer::BufferHandler, shm::ShmHandler};

trait MarkerTrait {}

struct State<T: MarkerTrait> {
    _d: T,
}

impl<T: MarkerTrait> BufferHandler for State<T> {
    fn buffer_destroyed(
        &mut self,
        _buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
        todo!()
    }
}

impl<T: MarkerTrait> ShmHandler for State<T> {
    fn shm_state(&self) -> &smithay::wayland::shm::ShmState {
        todo!()
    }
}

smithay_macros::delegate_bundle!(
    impl<T: MarkerTrait + 'static> State<T> {},
    Bundle {
        dispatch_to: smithay::wayland::shm::ShmState,
        globals: [Global {
            interface: smithay::reexports::wayland_server::protocol::wl_shm::WlShm,
            data: (),
        }],
        resources: [
            Resource {
                interface: smithay::reexports::wayland_server::protocol::wl_shm::WlShm,
                data: (),
            },
            Resource {
                interface: smithay::reexports::wayland_server::protocol::wl_shm_pool::WlShmPool,
                data: smithay::wayland::shm::ShmPoolUserData,
            },
            Resource {
                interface: smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
                data: smithay::wayland::shm::ShmBufferUserData,
            },
        ],
    },
);

fn main() {}
