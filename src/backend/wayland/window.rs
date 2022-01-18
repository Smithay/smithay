use std::{
    mem,
    os::unix::prelude::RawFd,
    sync::{atomic::Ordering, Arc, Mutex},
};

use gbm::BufferObject;
use sctk::{
    reexports::client::{protocol::wl_buffer, Connection, QueueHandle},
    shell::xdg::window::Window as SctkWindow,
};

use crate::{
    backend::allocator::{
        dmabuf::{AsDmabuf, Dmabuf},
        Slot, Swapchain,
    },
    utils::{Logical, Size},
};

use super::{data::WaylandBackendData, AllocateBuffersError, WaylandError};

/// A wayland window.
///
/// Dropping an instance of the window will destroy it.
#[derive(Debug)]
pub struct Window(pub(crate) Arc<Inner>);

impl Window {
    pub fn id(&self) -> WindowId {
        self.0.id
    }

    // Buffer functions

    /// Returns the next buffer that will be presented to the Window and its age.
    ///
    /// You may bind this buffer to a renderer to render.
    /// This function will return the same buffer until [`submit`](Self::submit) is called
    /// or [`reset_buffers`](Self::reset_buffers) is used to reset the buffers.
    pub fn buffer(&self) -> Result<(Dmabuf, u8), WaylandError> {
        let inner = &self.0;
        let mut backend_data = inner.backend_data.lock().unwrap();
        let mut data = inner.data.lock().unwrap();

        if let Some(new_size) = data.new_size.take() {
            // This manual dereference is needed otherwise rustc thinks we double borrow `data`.
            let data = &mut *data;

            data.swapchain.resize(new_size.w, new_size.h);
            data.buffer = None;
            data.pending_destruction.append(&mut data.current_buffers);
        }

        // Try to free any buffers pending destruction if the compositor has freed said buffers.
        data.pending_destruction.retain(|buffer| {
            if let Some(entry) = backend_data.protocols.dmabuf_state.get_entry(buffer) {
                let free = entry.free.load(Ordering::SeqCst);

                if free {
                    buffer.destroy();
                }

                !free
            } else {
                false
            }
        });

        if data.buffer.is_none() {
            data.buffer = Some(
                data.swapchain
                    .acquire()
                    .map_err(Into::<AllocateBuffersError>::into)?
                    .ok_or(AllocateBuffersError::NoFreeSlots)?,
            );
        }

        let slot = data.buffer.as_ref().unwrap();
        let age = slot.age();

        match slot.userdata().get::<Dmabuf>() {
            Some(dmabuf) => Ok((dmabuf.clone(), age)),
            None => {
                let dmabuf = slot.export().map_err(Into::<AllocateBuffersError>::into)?;
                slot.userdata().insert_if_missing(|| dmabuf.clone());

                let wl_buffer = backend_data.protocols.dmabuf_state.create_buffer(
                    &dmabuf,
                    &self.0.conn,
                    &inner.queue_handle,
                )?;
                slot.userdata().insert_if_missing(|| wl_buffer.clone());
                drop(backend_data);

                // Record the buffer as in use so it is not freed.
                data.current_buffers.push(wl_buffer);

                Ok((dmabuf, age))
            }
        }
    }

    /// Consume and submit the buffer to the window.
    pub fn submit(&self) -> Result<(), WaylandError> {
        let inner = &self.0;
        let backend_data = inner.backend_data.lock().unwrap();
        let mut data = inner.data.lock().unwrap();

        // Get a new buffer
        let mut next = data
            .swapchain
            .acquire()
            .map_err(Into::<AllocateBuffersError>::into)?
            .ok_or(AllocateBuffersError::NoFreeSlots)?;

        if let Some(current) = data.buffer.as_mut() {
            mem::swap(&mut next, current);
        }

        let wl_surface = inner.sctk.wl_surface();

        let buffer = next.userdata().get::<wl_buffer::WlBuffer>().unwrap();

        // Mark the buffer as used.
        backend_data
            .protocols
            .dmabuf_state
            .get_entry(buffer)
            .unwrap()
            .free
            .store(false, Ordering::SeqCst);

        let width = next.width().map_err(Into::<AllocateBuffersError>::into)?;
        let height = next.height().map_err(Into::<AllocateBuffersError>::into)?;

        // Request the next frame.
        let _frame = wl_surface.frame(&inner.queue_handle, wl_surface.clone());
        wl_surface.attach(Some(buffer), 0, 0);
        // TODO: Damage the entire buffer until there is a way to pass damage before submitting.
        wl_surface.damage_buffer(0, 0, width as i32, height as i32);
        wl_surface.commit();

        data.swapchain.submitted(&next);

        Ok(())
    }

    /// Resets the internal buffers, e.g. to reset age values
    pub fn reset_buffers(&self) {
        let mut guard = self.0.data.lock().unwrap();

        // This manual dereference is needed otherwise rustc thinks we double borrow `data`.
        let data = &mut *guard;

        data.swapchain.reset_buffers();
        data.buffer = None;
        data.pending_destruction.append(&mut data.current_buffers);
    }
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub(crate) usize);

#[derive(Debug)]
pub(crate) struct Inner {
    // FIXME: Since sctk performs cleanup at it's own convince, we need to ensure we don't have a race when
    // destroying the wl_surface.
    pub(crate) sctk: SctkWindow,
    pub(crate) id: WindowId,
    pub(crate) conn: Connection,
    pub(crate) queue_handle: QueueHandle<WaylandBackendData>,
    pub(crate) backend_data: Arc<Mutex<WaylandBackendData>>,
    pub(crate) data: Mutex<Data>,
}

#[derive(Debug)]
pub(crate) struct Data {
    pub(crate) current_size: Size<u32, Logical>,
    pub(crate) new_size: Option<Size<u32, Logical>>,
    pub(crate) swapchain: Swapchain<Arc<Mutex<gbm::Device<RawFd>>>, BufferObject<()>>,
    pub(crate) buffer: Option<Slot<BufferObject<()>>>,
    /// Buffers associated with the current swapchain.
    pub(crate) current_buffers: Vec<wl_buffer::WlBuffer>,
    /// Buffers that need to be freed when possible.
    pub(crate) pending_destruction: Vec<wl_buffer::WlBuffer>,
}
