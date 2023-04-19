use ::smithay::wayland::{buffer::BufferHandler, shm::ShmHandler};

trait MarkerTrait {}

#[derive(smithay_derive::DelegateModule)]
#[delegate(Output, Shm)]
struct State;

impl BufferHandler for State {
    fn buffer_destroyed(
        &mut self,
        _buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
        todo!()
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &smithay::wayland::shm::ShmState {
        todo!()
    }
}

fn main() {}
