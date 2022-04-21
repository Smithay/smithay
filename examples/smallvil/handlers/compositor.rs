use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorHandler, CompositorState},
        shm::ShmState,
    },
};
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};

use crate::Smallvil;

impl CompositorHandler for Smallvil {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn commit(&mut self, cx: &mut DisplayHandle, surface: &WlSurface) {
        on_commit_buffer_handler(cx, surface);
        self.space.commit(surface);
    }
}

impl BufferHandler for Smallvil {
    fn buffer_destroyed(&mut self, _buffer: &smithay::wayland::buffer::Buffer) {}
}

impl AsRef<ShmState> for Smallvil {
    fn as_ref(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Smallvil);
delegate_shm!(Smallvil);
