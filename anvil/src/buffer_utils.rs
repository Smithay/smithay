#[cfg(feature = "egl")]
use std::{cell::RefCell, rc::Rc};

#[cfg(feature = "egl")]
use smithay::backend::egl::{
    display::EGLBufferReader, BufferAccessError as EGLBufferAccessError, EGLImages, Format,
};
use smithay::{
    reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    wayland::shm::{with_buffer_contents as shm_buffer_contents, BufferAccessError},
};

/// Utilities for working with `WlBuffer`s.
#[derive(Clone)]
pub struct BufferUtils {
    #[cfg(feature = "egl")]
    egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>,
    log: ::slog::Logger,
}

impl BufferUtils {
    /// Creates a new `BufferUtils`.
    #[cfg(feature = "egl")]
    pub fn new(egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>, log: ::slog::Logger) -> Self {
        Self {
            egl_buffer_reader,
            log,
        }
    }

    /// Creates a new `BufferUtils`.
    #[cfg(not(feature = "egl"))]
    pub fn new(log: ::slog::Logger) -> Self {
        Self { log }
    }

    /// Returns the dimensions of an image stored in the buffer.
    #[cfg(feature = "egl")]
    pub fn dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        // Try to retrieve the EGL dimensions of this buffer, and, if that fails, the shm dimensions.
        self.egl_buffer_reader
            .borrow()
            .as_ref()
            .and_then(|display| display.egl_buffer_dimensions(buffer))
            .or_else(|| self.shm_buffer_dimensions(buffer).ok())
    }

    /// Returns the dimensions of an image stored in the buffer.
    #[cfg(not(feature = "egl"))]
    pub fn dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        self.shm_buffer_dimensions(buffer).ok()
    }

    /// Returns the dimensions of an image stored in the shm buffer.
    fn shm_buffer_dimensions(&self, buffer: &WlBuffer) -> Result<(i32, i32), BufferAccessError> {
        shm_buffer_contents(buffer, |_, data| (data.width, data.height)).map_err(|err| {
            warn!(self.log, "Unable to load buffer contents"; "err" => format!("{:?}", err));
            err
        })
    }
}
