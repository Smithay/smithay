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
//! as specified by the wayland protocol) and obtain its `ShmToken`.
//!
//! ```
//! extern crate wayland_server;
//! extern crate smithay;
//!
//! use smithay::wayland::shm::init_shm_global;
//! use wayland_server::protocol::wl_shm::Format;
//!
//! # let mut event_loop = calloop::EventLoop::<()>::new().unwrap();
//! # let mut display = wayland_server::Display::new(event_loop.handle());
//! // Insert the ShmGlobal into your event loop
//! // Here, we specify that Yuyv and C8 format are supported
//! // additionally to the standard Argb8888 and Xrgb8888.
//! let shm_global = init_shm_global(
//!     &mut display,
//!     vec![Format::Yuyv, Format::C8],
//!     None // we don't provide a logger here
//! );
//! ```
//!
//! Then, when you have a [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer)
//! and need to retrieve its contents, use the
//! [`with_buffer_contents`](::wayland::shm::with_buffer_contents) function to do it:
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
//!         /* `something` is the content you returned from the closure */
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

use self::pool::{Pool, ResizeError};
use std::{rc::Rc, sync::Arc};
use wayland_server::{
    protocol::{wl_buffer, wl_shm, wl_shm_pool},
    Display, Global, NewResource,
};

mod pool;

#[derive(Clone)]
/// Internal data storage of `ShmGlobal`
///
/// This type is only visible as type parameter of
/// the `Global` handle you are provided.
pub struct ShmGlobalData {
    formats: Rc<Vec<wl_shm::Format>>,
    log: ::slog::Logger,
}

/// Create a new SHM global advertizing given supported formats.
///
/// This global will always advertize `ARGB8888` and `XRGB8888` format
/// as they are required by the protocol. Formats given as argument
/// as additionally advertized.
///
/// The global is directly created on the provided [`Display`](wayland_server::Display),
/// and this function returns the global handle, in case you wish to remove this global in
/// the future.
pub fn init_shm_global<L>(
    display: &mut Display,
    mut formats: Vec<wl_shm::Format>,
    logger: L,
) -> Global<wl_shm::WlShm>
where
    L: Into<Option<::slog::Logger>>,
{
    let log = crate::slog_or_stdlog(logger);

    // always add the mandatory formats
    formats.push(wl_shm::Format::Argb8888);
    formats.push(wl_shm::Format::Xrgb8888);
    let data = ShmGlobalData {
        formats: Rc::new(formats),
        log: log.new(o!("smithay_module" => "shm_handler")),
    };

    display.create_global::<wl_shm::WlShm, _>(1, move |shm_new: NewResource<_>, _version| {
        let shm = shm_new.implement_closure(
            {
                let mut data = data.clone();
                move |req, shm| data.receive_shm_message(req, shm)
            },
            None::<fn(_)>,
            (),
        );
        // send the formats
        for &f in &data.formats[..] {
            shm.format(f);
        }
    })
}

/// Error that can occur when accessing an SHM buffer
#[derive(Debug)]
pub enum BufferAccessError {
    /// This buffer is not managed by the SHM handler
    NotManaged,
    /// An error occurred while accessing the memory map
    ///
    /// This can happen if the client advertized a wrong size
    /// for the memory map.
    ///
    /// If this error occurs, the client has been killed as a result.
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
/// and this method will return `Err(())` (this will be the case for an EGL buffer for example).
pub fn with_buffer_contents<F, T>(buffer: &wl_buffer::WlBuffer, f: F) -> Result<T, BufferAccessError>
where
    F: FnOnce(&[u8], BufferData) -> T,
{
    let data = match buffer.as_ref().user_data().get::<InternalBufferData>() {
        Some(d) => d,
        None => return Err(BufferAccessError::NotManaged),
    };

    match data.pool.with_data_slice(|slice| f(slice, data.data)) {
        Ok(t) => Ok(t),
        Err(()) => {
            // SIGBUS error occurred
            buffer
                .as_ref()
                .post_error(wl_shm::Error::InvalidFd as u32, "Bad pool size.".into());
            Err(BufferAccessError::BadMap)
        }
    }
}

impl ShmGlobalData {
    fn receive_shm_message(&mut self, request: wl_shm::Request, shm: wl_shm::WlShm) {
        use self::wl_shm::{Error, Request};

        let (pool, fd, size) = match request {
            Request::CreatePool { id: pool, fd, size } => (pool, fd, size),
            _ => unreachable!(),
        };
        if size <= 0 {
            shm.as_ref().post_error(
                Error::InvalidFd as u32,
                "Invalid size for a new wl_shm_pool.".into(),
            );
            return;
        }
        let mmap_pool = match Pool::new(fd, size as usize, self.log.clone()) {
            Ok(p) => p,
            Err(()) => {
                shm.as_ref().post_error(
                    wl_shm::Error::InvalidFd as u32,
                    format!("Failed mmap of fd {}.", fd),
                );
                return;
            }
        };
        let arc_pool = Arc::new(mmap_pool);
        pool.implement_closure(
            {
                let mut data = self.clone();
                move |req, pool| data.receive_pool_message(req, pool)
            },
            None::<fn(_)>,
            arc_pool,
        );
    }
}

/// Details of the contents of a buffer relative to its pool
#[derive(Copy, Clone, Debug)]
pub struct BufferData {
    /// Offset of the start of the buffer relative to the beginning of the pool in bytes
    pub offset: i32,
    /// Wwidth of the buffer in bytes
    pub width: i32,
    /// Height of the buffer in bytes
    pub height: i32,
    /// Stride of the buffer in bytes
    pub stride: i32,
    /// Format used by this buffer
    pub format: wl_shm::Format,
}

struct InternalBufferData {
    pool: Arc<Pool>,
    data: BufferData,
}

impl ShmGlobalData {
    fn receive_pool_message(&mut self, request: wl_shm_pool::Request, pool: wl_shm_pool::WlShmPool) {
        use self::wl_shm_pool::Request;

        let arc_pool = pool.as_ref().user_data().get::<Arc<Pool>>().unwrap();

        match request {
            Request::CreateBuffer {
                id: buffer,
                offset,
                width,
                height,
                stride,
                format,
            } => {
                if !self.formats.contains(&format) {
                    pool.as_ref().post_error(
                        wl_shm::Error::InvalidFormat as u32,
                        format!("SHM format {:?} is not supported.", format),
                    );
                    return;
                }
                let data = InternalBufferData {
                    pool: arc_pool.clone(),
                    data: BufferData {
                        offset,
                        width,
                        height,
                        stride,
                        format,
                    },
                };
                buffer.implement_closure(|_, _| {}, None::<fn(_)>, data);
            }
            Request::Resize { size } => match arc_pool.resize(size) {
                Ok(()) => {}
                Err(ResizeError::InvalidSize) => {
                    pool.as_ref().post_error(
                        wl_shm::Error::InvalidFd as u32,
                        "Invalid new size for a wl_shm_pool.".into(),
                    );
                }
                Err(ResizeError::MremapFailed) => {
                    pool.as_ref()
                        .post_error(wl_shm::Error::InvalidFd as u32, "mremap failed.".into());
                }
            },
            Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}
