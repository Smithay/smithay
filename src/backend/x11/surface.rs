use std::{
    mem,
    sync::{mpsc::Receiver, Arc, Mutex, MutexGuard, Weak},
};

use drm_fourcc::DrmFourcc;
use gbm::BufferObject;
use x11rb::{connection::Connection, protocol::xproto::PixmapWrapper, rust_connection::RustConnection};

use crate::{
    backend::{
        allocator::{
            dmabuf::{AsDmabuf, Dmabuf},
            Slot, Swapchain,
        },
        drm::DrmNode,
        x11::{buffer::PixmapWrapperExt, window_inner::WindowInner, AllocateBuffersError, Window},
    },
    utils::{Logical, Size},
};

use super::{WindowTemporary, X11Error};

/// An error that may occur when presenting.
#[derive(Debug, thiserror::Error)]
pub enum PresentError {
    /// The dmabuf being presented has too many planes.
    #[error("The Dmabuf had too many planes")]
    TooManyPlanes,

    /// Duplicating the dmabuf handles failed.
    #[error("Duplicating the file descriptors for the dmabuf handles failed")]
    DupFailed(String),

    /// The format dmabuf presented does not match the format of the window.
    #[error("Buffer had incorrect format, expected: {0}")]
    IncorrectFormat(DrmFourcc),
}

/// An X11 surface which uses GBM to allocate and present buffers.
#[derive(Debug)]
pub struct X11Surface {
    pub(crate) connection: Weak<RustConnection>,
    pub(crate) window: Weak<WindowInner>,
    pub(crate) resize: Receiver<Size<u16, Logical>>,
    pub(crate) swapchain: Swapchain<Arc<Mutex<gbm::Device<DrmNode>>>, BufferObject<()>>,
    pub(crate) format: DrmFourcc,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) buffer: Option<Slot<BufferObject<()>>>,
}

impl X11Surface {
    /// Returns the window the surface presents to.
    ///
    /// This will return [`None`] if the window has been destroyed.
    pub fn window(&self) -> Option<impl AsRef<Window> + '_> {
        self.window.upgrade().map(Window).map(WindowTemporary)
    }

    /// Returns a handle to the GBM device used to allocate buffers.
    pub fn device(&self) -> MutexGuard<'_, gbm::Device<DrmNode>> {
        self.swapchain.allocator.lock().unwrap()
    }

    /// Returns the format of the buffers the surface accepts.
    pub fn format(&self) -> DrmFourcc {
        self.format
    }

    /// Returns the next buffer that will be presented to the Window and its age.
    ///
    /// You may bind this buffer to a renderer to render.
    /// This function will return the same buffer until [`submit`](Self::submit) is called
    /// or [`reset_buffers`](Self::reset_buffers) is used to reset the buffers.
    pub fn buffer(&mut self) -> Result<(Dmabuf, u8), AllocateBuffersError> {
        if let Some(new_size) = self.resize.try_iter().last() {
            self.resize(new_size);
        }

        if self.buffer.is_none() {
            self.buffer = Some(
                self.swapchain
                    .acquire()
                    .map_err(Into::<AllocateBuffersError>::into)?
                    .ok_or(AllocateBuffersError::NoFreeSlots)?,
            );
        }

        let slot = self.buffer.as_ref().unwrap();
        let age = slot.age();
        match slot.userdata().get::<Dmabuf>() {
            Some(dmabuf) => Ok((dmabuf.clone(), age)),
            None => {
                let dmabuf = slot.export().map_err(Into::<AllocateBuffersError>::into)?;
                slot.userdata().insert_if_missing(|| dmabuf.clone());
                Ok((dmabuf, age))
            }
        }
    }

    /// Consume and submit the buffer to the window.
    pub fn submit(&mut self) -> Result<(), X11Error> {
        if let Some(connection) = self.connection.upgrade() {
            // Get a new buffer
            let mut next = self
                .swapchain
                .acquire()
                .map_err(Into::<AllocateBuffersError>::into)?
                .ok_or(AllocateBuffersError::NoFreeSlots)?;

            // Swap the buffers
            if let Some(current) = self.buffer.as_mut() {
                mem::swap(&mut next, current);
            }

            let window = self.window().ok_or(AllocateBuffersError::WindowDestroyed)?;
            let pixmap = PixmapWrapper::with_dmabuf(
                &*connection,
                window.as_ref(),
                next.userdata().get::<Dmabuf>().unwrap(),
            )?;

            // Now present the current buffer
            let _ = pixmap.present(&*connection, window.as_ref())?;
            self.swapchain.submitted(next);

            // Flush the connection after presenting to the window to ensure we don't run out of buffer space in the X11 connection.
            let _ = connection.flush();
        }
        Ok(())
    }

    /// Resets the internal buffers, e.g. to reset age values
    pub fn reset_buffers(&mut self) {
        self.swapchain.reset_buffers();
        self.buffer = None;
    }

    fn resize(&mut self, size: Size<u16, Logical>) {
        self.swapchain.resize(size.w as u32, size.h as u32);
        self.buffer = None;

        self.width = size.w;
        self.height = size.h;
    }
}
