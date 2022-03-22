//! Buffer management utilities.
//!
//! This module provides the [`ManagedBuffer`] type to represent a [`WlBuffer`] managed by Smithay and data
//! associated with said buffer. This module has a dual purpose. This module provides a way for the compositor
//! to be told when a client has destroyed a buffer. The other purpose is to provide a way for specific types
//! of [`WlBuffer`] to abstractly associate some data with the protocol object without conflicting
//! [`Dispatch`](crate::reexports::wayland_server::Dispatch) implementations.

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
/// Buffer abstractions (such as [`shm`](crate::wayland::shm)) should require this trait to allow notification
/// when a buffer is destroyed.
pub trait BufferHandler {
    /// Called when the client has destroyed the buffer.
    ///
    /// At this point the buffer is no longer usable by Smithay.
    fn buffer_destroyed(&mut self, buffer: &ManagedBuffer);
}

/// A wrapper around a [`WlBuffer`] managed by Smithay.
#[derive(Debug, Clone)]
pub struct ManagedBuffer {
    buffer: ObjectId,
    data: Arc<BufferData>,
}

impl PartialEq for ManagedBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.buffer == other.buffer
    }
}

impl ManagedBuffer {
    /// Initializes a buffer, associating some user data with the buffer. This function causes the buffer to
    /// managed by Smithay.
    ///
    /// The data associated with the buffer may obtained using [`BufferManager::buffer_data`].
    pub fn init_buffer<D, T>(init: &mut DataInit<'_, D>, buffer: New<WlBuffer>, data: T) -> ManagedBuffer
    where
        D: BufferHandler + 'static,
        T: Send + Sync + 'static,
    {
        let data = Arc::new(BufferData { data: Box::new(data) });
        let buffer = init.custom_init(buffer, data.clone());

        ManagedBuffer {
            buffer: buffer.id(),
            data,
        }
    }

    /// Creates a [`Buffer`] from a [`WlBuffer`].
    ///
    /// This function returns [`Err`] if the buffer is not managed by Smithay (such as an EGL buffer).
    pub fn from_buffer(
        buffer: &WlBuffer,
        dh: &mut DisplayHandle<'_>,
    ) -> Result<ManagedBuffer, UnmanagedResource> {
        let data = dh
            .get_object_data(buffer.id())
            .map_err(|_| UnmanagedResource)?
            .downcast::<BufferData>()
            .map_err(|_| UnmanagedResource)?;

        Ok(ManagedBuffer {
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
        debug_assert_eq!(msg.opcode, 0);

        data.buffer_destroyed(&ManagedBuffer {
            buffer: msg.sender_id,
            data: self,
        });

        None
    }

    fn destroyed(&self, _: &mut D, _: ClientId, _: ObjectId) {}
}
