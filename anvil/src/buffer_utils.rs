use std::{cell::RefCell, rc::Rc};

use slog::Logger;

#[cfg(feature = "egl")]
use smithay::backend::egl::display::WaylandEGLDisplay;
use smithay::{
    reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    wayland::shm::with_buffer_contents as shm_buffer_contents,
};

/// Utilities for working with `WlBuffer`s.
#[derive(Clone)]
pub struct BufferUtils {
    #[cfg(feature = "egl")]
    egl_display: Rc<RefCell<Option<WaylandEGLDisplay>>>,
    log: Logger,
}

impl BufferUtils {
    /// Creates a new `BufferUtils`.
    #[cfg(feature = "egl")]
    pub fn new(egl_display: Rc<RefCell<Option<WaylandEGLDisplay>>>, log: Logger) -> Self {
        Self { egl_display, log }
    }

    /// Creates a new `BufferUtils`.
    #[cfg(not(feature = "egl"))]
    pub fn new(log: Logger) -> Self {
        Self { log }
    }

    /// Returns the dimensions of an image stored in the buffer.
    #[cfg(feature = "egl")]
    pub fn dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        // Try to retrieve the EGL dimensions of this buffer, and, if that fails, the shm dimensions.
        self.egl_display
            .borrow()
            .as_ref()
            .and_then(|display| display.egl_buffer_dimensions(buffer))
            .or_else(|| self.shm_buffer_dimensions(buffer))
    }

    /// Returns the dimensions of an image stored in the buffer.
    #[cfg(not(feature = "egl"))]
    pub fn dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        self.shm_buffer_dimensions(buffer)
    }

    /// Returns the dimensions of an image stored in the shm buffer.
    fn shm_buffer_dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        shm_buffer_contents(buffer, |_, data| (data.width, data.height))
            .map_err(|err| {
                warn!(self.log, "Unable to load buffer contents"; "err" => format!("{:?}", err));
                err
            })
            .ok()
    }
}
