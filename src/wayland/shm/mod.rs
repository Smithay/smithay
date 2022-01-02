//! SHM handling helpers
//!
//! This module provides helpers to handle SHM-based buffers from wayland clients.
//!
//! SHM (Shared Memory) is the most basic way wayland clients can send content to
//! the compositor: by sending a file descriptor to some (likely RAM-backed) storage
//! containing the actual data. This helper handles for you most of the logic for
//! handling these file descriptor and accessing their contents as simple `&[u8]` slices.
//!
//! This module is heavily inspired from
//! [the similar helpers](https://cgit.freedesktop.org/wayland/wayland/tree/src/wayland-shm.c)
//! of the wayland C libraries.
//!
//! To use it, first add a `ShmGlobal` to your display, specifying the formats
//! you want to support (ARGB8888 and XRGB8888 are always considered as supported,
//! as specified by the wayland protocol).
//!
//! ```
//! extern crate wayland_server;
//! extern crate smithay;
//!
//! use smithay::wayland::shm::init_shm_global;
//! use wayland_server::protocol::wl_shm::Format;
//!
//! # let mut display = wayland_server::Display::new();
//! // Insert the ShmGlobal into your event loop
//! // Here, we specify that Yuyv and C8 format are supported
//! // additionally to the standard Argb8888 and Xrgb8888.
//! init_shm_global(
//!     &mut display,
//!     vec![Format::Yuyv, Format::C8],
//!     None // we don't provide a logger here
//! );
//! ```
//!
//! Then, when you have a [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer)
//! and need to retrieve its contents, use the
//! [`with_buffer_contents`] function to do it:
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! # use wayland_server::protocol::wl_buffer::WlBuffer;
//! # fn wrap(buffer: &WlBuffer) {
//! use smithay::wayland::shm::{with_buffer_contents, BufferData, BufferAccessError};
//!
//! let content = with_buffer_contents(&buffer,
//!     |slice: &[u8], buffer_metadata: BufferData| {
//!         // do something to extract the contents of the buffer
//!     }
//! );
//!
//! match content {
//!     Ok(something) =>  {
//!         /* `something` is the value you returned from the closure */
//!     },
//!     Err(BufferAccessError::NotManaged) => {
//!         /* This buffer is not managed by the SHM global, but by something else */
//!     },
//!     Err(BufferAccessError::BadMap) => {
//!         /* The client supplied invalid content specification for this buffer,
//!            and was killed.
//!          */
//!     }
//! }
//! # }
//! ```
//!
//! **Note**
//!
//! This handler makes itself safe regarding the client providing a wrong size for the memory pool
//! by using a SIGBUS handler.
//!
//! If you are already using an handler for this signal, you probably don't want to use this handler.

use wayland_server::{
    backend::GlobalId,
    protocol::{
        wl_buffer::{self},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
    },
    Dispatch, DisplayHandle, GlobalDispatch, Resource,
};

mod handlers;
mod pool;

pub use handlers::{ShmBufferUserData, ShmPoolUserData};

/// State of SHM module
#[derive(Debug)]
pub struct ShmState {
    formats: Vec<wl_shm::Format>,
    shm: GlobalId,
    log: ::slog::Logger,
}

impl ShmState {
    /// Create a new SHM global advertizing given supported formats.
    ///
    /// This global will always advertize `ARGB8888` and `XRGB8888` format
    /// as they are required by the protocol. Formats given as argument
    /// as additionally advertized.
    ///
    /// The global is directly created on the provided [`Display`](wayland_server::Display),
    /// and this function returns the global handle, in case you wish to remove this global in
    /// the future.
    pub fn new<L, D>(
        display: &mut DisplayHandle<'_, D>,
        mut formats: Vec<wl_shm::Format>,
        logger: L,
    ) -> ShmState
    where
        L: Into<Option<::slog::Logger>>,
        D: GlobalDispatch<WlShm, GlobalData = ()>
            + Dispatch<WlShm, UserData = ()>
            + Dispatch<WlShmPool, UserData = ShmPoolUserData>
            + 'static,
    {
        let log = crate::slog_or_fallback(logger);

        // always add the mandatory formats
        formats.push(wl_shm::Format::Argb8888);
        formats.push(wl_shm::Format::Xrgb8888);

        let shm = display.create_global::<WlShm>(1, ());

        let state = ShmState {
            formats,
            shm,
            log: log.new(slog::o!("smithay_module" => "shm_handler")),
        };

        state
    }

    /// Get id of WlShm globabl
    pub fn shm_global(&self) -> GlobalId {
        self.shm.clone()
    }
}

/// Dispatching type for shm module
#[derive(Debug)]
pub struct ShmDispatch<'a>(pub &'a mut ShmState);

/// Error that can occur when accessing an SHM buffer
#[derive(Debug, thiserror::Error)]
pub enum BufferAccessError {
    /// This buffer is not managed by the SHM handler
    #[error("non-SHM buffer")]
    NotManaged,
    /// An error occurred while accessing the memory map
    ///
    /// This can happen if the client advertized a wrong size
    /// for the memory map.
    ///
    /// If this error occurs, the client has been killed as a result.
    #[error("invalid client buffer")]
    BadMap,
}

/// Call given closure with the contents of the given buffer
///
/// If the buffer is managed by the provided `ShmGlobal`, its contents are
/// extracted and the closure is extracted with them:
///
/// - The first argument is a data slice of the contents of the pool
/// - The second argument is the specification of this buffer is this pool
///
/// If the buffer is not managed by the provided `ShmGlobal`, the closure is not called
/// and this method will return `Err(BufferAccessError::NotManaged)` (this will be the case for an
/// EGL buffer for example).
pub fn with_buffer_contents<F, T>(buffer: &wl_buffer::WlBuffer, f: F) -> Result<T, BufferAccessError>
where
    F: FnOnce(&[u8], BufferData) -> T,
{
    let data = match buffer.data::<ShmBufferUserData>() {
        Some(d) => d,
        None => return Err(BufferAccessError::NotManaged),
    };

    match data.pool.with_data_slice(|slice| f(slice, data.data)) {
        Ok(t) => Ok(t),
        Err(()) => {
            // SIGBUS error occurred
            // buffer.post_error(display_handle, wl_shm::Error::InvalidFd, "Bad pool size.");
            Err(BufferAccessError::BadMap)
        }
    }
}

impl ShmState {}

/// Details of the contents of a buffer relative to its pool
#[derive(Copy, Clone, Debug)]
pub struct BufferData {
    /// Offset of the start of the buffer relative to the beginning of the pool in bytes
    pub offset: i32,
    /// Width of the buffer in bytes
    pub width: i32,
    /// Height of the buffer in bytes
    pub height: i32,
    /// Stride of the buffer in bytes
    pub stride: i32,
    /// Format used by this buffer
    pub format: wl_shm::Format,
}
