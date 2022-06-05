use crate::{grabs::resize_grab, Smallvil};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        DisplayHandle,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorHandler, CompositorState},
        shm::ShmState,
    },
};

impl CompositorHandler for Smallvil {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn commit(&mut self, dh: &DisplayHandle, surface: &WlSurface) {
        on_commit_buffer_handler(dh, surface);
        self.space.commit(surface);

        resize_grab::handle_commit(&mut self.space, surface);
    }
}

impl BufferHandler for Smallvil {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl AsRef<ShmState> for Smallvil {
    fn as_ref(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Smallvil);
delegate_shm!(Smallvil);
