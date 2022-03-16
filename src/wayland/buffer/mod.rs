//! Buffer management utilities.
//!
//! This module provides the [`Buffer`] type to represent a [`WlBuffer`] managed by Smithay and data
//! associated with said buffer. This module has a dual purpose. This module provides a way for the compositor
//! to be told when a client has destroyed a buffer. The other purpose is to provide a way for specific types
//! of [`WlBuffer`] to abstractly associate some data with the protocol object without conflicting
//! [`Dispatch`](crate::reexports::wayland_server::Dispatch) implementations.

use std::{any::Any, sync::Arc};

use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, InvalidId, ObjectData, ObjectId},
    protocol::wl_buffer::WlBuffer,
    Client, DataInit, DisplayHandle, New, Resource,
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
    fn buffer_destroyed(&mut self, buffer: &Buffer);
}

/// A wrapper around a [`WlBuffer`] managed by Smithay.
#[derive(Debug, Clone, PartialEq)]
pub struct Buffer(BufferInner);

impl Buffer {
    /// Initializes a buffer, associating some user data with the buffer. This function causes the buffer to
    /// managed by Smithay.
    ///
    /// The data associated with the buffer may obtained using [`Buffer::buffer_data`].
    pub fn init_buffer<D, T>(init: &mut DataInit<'_, D>, buffer: New<WlBuffer>, data: T) -> Buffer
    where
        D: BufferHandler + 'static,
        T: Send + Sync + 'static,
    {
        let data = Arc::new(BufferData { data: Box::new(data) });
        let buffer = init.custom_init(buffer, data.clone());

        Buffer(BufferInner::Managed {
            buffer: buffer.id(),
            data,
        })
    }

    /// Creates a [`WlBuffer`] protocol object and associates some data with the buffer. This buffer object
    /// is immediately managed by Smithay.
    ///
    /// The data associated with the buffer may obtained using [`Buffer::buffer_data`].
    ///
    /// # Usage
    ///
    /// You must send the created protocol object to the client immediately or else protocol errors will occur.
    #[must_use = "You must send the WlBuffer to the client or else protocol errors will occur"]
    pub fn create_buffer<D, T>(
        dh: &mut DisplayHandle<'_>,
        client: &Client,
        data: T,
    ) -> Result<(WlBuffer, Buffer), InvalidId>
    where
        D: BufferHandler + 'static,
        T: Send + Sync + 'static,
    {
        let backend = dh.backend_handle::<D>().unwrap();
        let data = Arc::new(BufferData { data: Box::new(data) });
        let buffer_id = backend.create_object(client.id(), WlBuffer::interface(), 1, data.clone())?;
        let wl_buffer = WlBuffer::from_id(dh, buffer_id.clone())?;

        Ok((
            wl_buffer,
            Buffer(BufferInner::Managed {
                buffer: buffer_id,
                data,
            }),
        ))
    }

    /// Creates a [`Buffer`] from a [`WlBuffer`].
    pub fn from_wl(buffer: &WlBuffer, dh: &mut DisplayHandle<'_>) -> Buffer {
        match dh.get_object_data(buffer.id()) {
            Ok(data) => {
                match data.downcast::<BufferData>() {
                    Ok(data) => Buffer(BufferInner::Managed {
                        buffer: buffer.id(),
                        data,
                    }),

                    // The buffer has some user data but is not managed by Smithay.
                    Err(_) => Buffer(BufferInner::Unmanaged(buffer.clone())),
                }
            }

            // A completely unmanaged buffer (generally EGL)
            Err(_) => Buffer(BufferInner::Unmanaged(buffer.clone())),
        }
    }

    /// Returns the object id of the underlying [`WlBuffer`].
    pub fn id(&self) -> ObjectId {
        match &self.0 {
            BufferInner::Managed { buffer, .. } => buffer.clone(),
            BufferInner::Unmanaged(buffer) => buffer.id(),
        }
    }

    /// Returns a reference to the underlying [`WlBuffer`].
    pub fn buffer(&self, dh: &mut DisplayHandle<'_>) -> Result<WlBuffer, InvalidId> {
        match &self.0 {
            BufferInner::Managed { buffer, .. } => WlBuffer::from_id(dh, buffer.clone()),
            BufferInner::Unmanaged(buffer) => Ok(buffer.clone()),
        }
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
    pub fn buffer_data<T>(&self) -> Result<Option<&T>, UnmanagedResource>
    where
        T: Send + Sync + 'static,
    {
        match self.0 {
            BufferInner::Managed { ref data, .. } => Ok(<dyn Any>::downcast_ref::<T>(&*data.data)),
            BufferInner::Unmanaged(_) => Err(UnmanagedResource),
        }
    }
}

#[derive(Debug, Clone)]
enum BufferInner {
    /// The buffer is managed by Smithay.
    Managed {
        /// Object id of the [`WlBuffer`].
        ///
        /// This cannot be a [`WlBuffer`] or else there will be an Arc cycle.
        buffer: ObjectId,

        /// Data associated with the managed buffer.
        data: Arc<BufferData>,
    },

    /// Buffer not managed by Smithay.
    ///
    /// There is no user data that may be taken from the buffer.
    Unmanaged(WlBuffer),
}

impl PartialEq for BufferInner {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Managed {
                    buffer: self_buffer, ..
                },
                Self::Managed {
                    buffer: other_buffer, ..
                },
            ) => self_buffer == other_buffer,

            (Self::Unmanaged(self_buffer), Self::Unmanaged(other_buffer)) => self_buffer == other_buffer,

            _ => false,
        }
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

        data.buffer_destroyed(&Buffer(BufferInner::Managed {
            buffer: msg.sender_id,
            data: self,
        }));

        None
    }

    fn destroyed(&self, _: &mut D, _: ClientId, _: ObjectId) {}
}
