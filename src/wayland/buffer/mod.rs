//! Buffer management utilities.
//!
//! This module provides the [`Buffer`] type to represent a [`WlBuffer`] managed by Smithay and data
//! associated with said buffer. This module has a dual purpose. This module provides a way for the compositor
//! to be told when a client has destroyed a buffer. The other purpose is to provide a way for specific types
//! of [`WlBuffer`] to abstractly associate some data with the protocol object without conflicting
//! [`Dispatch`](crate::reexports::wayland_server::Dispatch) implementations.
//!
//! Unlike most delegate types found in other modules of the wayland frontend, this module has no `delegate`
//! macro by design.

use std::{any::Any, sync::Arc};

use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, InvalidId, ObjectData, ObjectId},
    protocol::wl_buffer::WlBuffer,
    DataInit, DisplayHandle, New, Resource,
};

use crate::utils::UnmanagedResource;

/// Handler trait for associating data with a [`WlBuffer`].
///
/// This trait primarily allows compositors to be told when a buffer is destroyed.
///
/// # For buffer abstractions
///
/// Buffer abstractions (such as [`shm`](crate::wayland::shm)) should require this trait bound in dispatch
/// requirements to ensure access to the [`BufferManager`].
pub trait BufferHandler {
    /// Returns a reference to the buffer manager.
    fn buffer_manager(&self) -> &BufferManager;

    /// Called when the client has destroyed the buffer.
    ///
    /// At this point the buffer is no longer usable by Smithay.
    fn buffer_destroyed(&mut self, buffer: &Buffer);
}

/// The buffer manager.
///
/// This type allows a [`WlBuffer`] to have data associated and managed by Smithay.
#[derive(Debug)]
pub struct BufferManager(());

impl BufferManager {
    /// Initializes the buffer manager.
    #[allow(clippy::new_without_default)]
    pub fn new() -> BufferManager {
        BufferManager(())
    }

    /// Initializes a buffer, associating some user data with the buffer. This function causes the buffer to
    /// managed by Smithay.
    ///
    /// The data associated with the buffer may obtained using [`BufferManager::buffer_data`].
    pub fn init_buffer<D, T>(&self, init: &mut DataInit<'_, D>, buffer: New<WlBuffer>, data: T) -> Buffer
    where
        D: BufferHandler + 'static,
        T: Send + Sync + 'static,
    {
        let data = Arc::new(BufferData { data: Box::new(data) });
        let buffer = init.custom_init(buffer, data.clone());

        Buffer {
            buffer: buffer.id(),
            data,
        }
    }
}

/// A wrapper around a [`WlBuffer`] managed by Smithay.
#[derive(Debug, Clone)]
pub struct Buffer {
    buffer: ObjectId,
    data: Arc<BufferData>,
}

impl PartialEq for Buffer {
    fn eq(&self, other: &Self) -> bool {
        self.buffer == other.buffer
    }
}

impl Buffer {
    /// Creates a [`Buffer`] from a [`WlBuffer`].
    ///
    /// This function returns [`Err`] if the buffer is not managed by Smithay (such as an EGL buffer).
    pub fn from_buffer(buffer: &WlBuffer, dh: &mut DisplayHandle<'_>) -> Result<Buffer, UnmanagedResource> {
        let data = dh
            .get_object_data(buffer.id())
            .map_err(|_| UnmanagedResource)?
            .downcast::<BufferData>()
            .map_err(|_| UnmanagedResource)?;

        Ok(Buffer {
            buffer: buffer.id(),
            data,
        })
    }

    /// Returns a reference to the underlying [`WlBuffer`].
    pub fn buffer(&self, dh: &mut DisplayHandle<'_>) -> Result<WlBuffer, InvalidId> {
        WlBuffer::from_id(dh, self.buffer.clone())
    }

    /// Sends a `release` event, indicating the buffer is no longer in use by the compositor, meaning the
    /// client is free to reuse or destroy the buffer.
    pub fn release(&self, dh: &mut DisplayHandle<'_>) -> Result<(), InvalidId> {
        self.buffer(dh)?.release(dh);
        Ok(())
    }

    /// Returns the data associated with the buffer.
    ///
    /// This function is intended for abstractions to obtain the buffer data they store. Users should use the
    /// functions provided by the buffer abstractions to obtain the data. For example, data associated with an
    /// shm buffer should be obtained using [`with_buffer_contents`](crate::wayland::shm::with_buffer_contents)
    /// instead of specifying the type of the data in the `T` generic for this function.
    pub fn buffer_data<T>(&self) -> Option<&T>
    where
        T: Send + Sync + 'static,
    {
        <dyn Any>::downcast_ref::<T>(&*self.data.data)
    }
}

#[derive(Debug)]
struct BufferData {
    data: Box<(dyn Any + Send + Sync + 'static)>,
}

impl<D> ObjectData<D> for BufferData
where
    D: BufferHandler + 'static,
{
    fn request(
        self: Arc<Self>,
        _: &mut Handle<D>,
        data: &mut D,
        _: ClientId,
        msg: Message<ObjectId>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        // WlBuffer has a single request which is a destructor.
        assert_eq!(msg.opcode, 0);

        data.buffer_destroyed(&Buffer {
            buffer: msg.sender_id,
            data: self,
        });

        None
    }

    fn destroyed(&self, _: &mut D, _: ClientId, _: ObjectId) {}
}
