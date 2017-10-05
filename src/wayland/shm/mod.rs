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
//! To use it, first add a `ShmGlobal` to your event loop, specifying the formats
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
//! # fn main() {
//! # let (display, mut event_loop) = wayland_server::create_display();
//! // Insert the ShmGlobal into your event loop
//! // Here, we specify that Yuyv and C8 format are supported
//! // additionnaly to the standart Argb8888 and Xrgb8888.
//! let shm_global = init_shm_global(
//!     &mut event_loop,
//!     vec![Format::Yuyv, Format::C8],
//!     None // we don't provide a logger here
//! );
//! # }
//! ```
//!
//! Then, when you have a `WlBuffer` and need to retrieve its contents, use the token method to
//! do it:
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! # use wayland_server::protocol::wl_buffer::WlBuffer;
//! # fn wrap(buffer: &WlBuffer) {
//! use smithay::wayland::shm::{with_buffer_contents, BufferData};
//!
//! with_buffer_contents(&buffer,
//!     |slice: &[u8], buffer_metadata: BufferData| {
//!         // do something to draw it on the screen
//!     }
//! );
//! # }
//! # fn main() {}
//! ```
//!
//! **Note**
//!
//! This handler makes itself safe regading the client providing a wrong size for the memory pool
//! by using a SIGBUS handler.
//!
//! If you are already using an handler for this signal, you probably don't want to use this handler.


use self::pool::{Pool, ResizeError};
use std::rc::Rc;
use std::sync::Arc;
use wayland_server::{resource_is_registered, Client, EventLoop, EventLoopHandle, Global, Resource};
use wayland_server::protocol::{wl_buffer, wl_shm, wl_shm_pool};

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
/// as additionnaly advertized.
///
/// The global is directly registered into the eventloop, and this function
/// returns the global handle, in case you whish to remove this global in
/// the future.
pub fn init_shm_global<L>(evl: &mut EventLoop, mut formats: Vec<wl_shm::Format>, logger: L)
                          -> Global<wl_shm::WlShm, ShmGlobalData>
where
    L: Into<Option<::slog::Logger>>,
{
    let log = ::slog_or_stdlog(logger);

    // always add the mandatory formats
    formats.push(wl_shm::Format::Argb8888);
    formats.push(wl_shm::Format::Xrgb8888);
    let data = ShmGlobalData {
        formats: Rc::new(formats),
        log: log.new(o!("smithay_module" => "shm_handler")),
    };

    evl.register_global::<wl_shm::WlShm, _>(1, shm_global_bind, data)
}

/// Error that can occur when accessing an SHM buffer
#[derive(Debug)]
pub enum BufferAccessError {
    /// This buffer is not managed by the SHM handler
    NotManaged,
    /// An error occured while accessing the memory map
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
pub fn with_buffer_contents<F>(buffer: &wl_buffer::WlBuffer, f: F) -> Result<(), BufferAccessError>
where
    F: FnOnce(&[u8], BufferData),
{
    if !resource_is_registered(buffer, &buffer_implementation()) {
        return Err(BufferAccessError::NotManaged);
    }
    let data = unsafe { &*(buffer.get_user_data() as *mut InternalBufferData) };

    if data.pool
        .with_data_slice(|slice| f(slice, data.data))
        .is_err()
    {
        // SIGBUS error occured
        buffer.post_error(wl_shm::Error::InvalidFd as u32, "Bad pool size.".into());
        return Err(BufferAccessError::BadMap);
    }
    Ok(())
}

fn shm_global_bind(evlh: &mut EventLoopHandle, data: &mut ShmGlobalData, _: &Client, global: wl_shm::WlShm) {
    // register an handler for this shm
    evlh.register(&global, shm_implementation(), data.clone(), None);
    // and then the custom formats
    for f in &data.formats[..] {
        global.format(*f);
    }
}

fn shm_implementation() -> wl_shm::Implementation<ShmGlobalData> {
    wl_shm::Implementation {
        create_pool: |evlh, data, _, shm, pool, fd, size| {
            if size <= 0 {
                shm.post_error(
                    wl_shm::Error::InvalidFd as u32,
                    "Invalid size for a new wl_shm_pool.".into(),
                );
                return;
            }
            let mmap_pool = match Pool::new(fd, size as usize, data.log.clone()) {
                Ok(p) => p,
                Err(()) => {
                    shm.post_error(
                        wl_shm::Error::InvalidFd as u32,
                        format!("Failed mmap of fd {}.", fd),
                    );
                    return;
                }
            };
            let arc_pool = Box::new(Arc::new(mmap_pool));
            evlh.register(
                &pool,
                shm_pool_implementation(),
                data.clone(),
                Some(destroy_shm_pool),
            );
            pool.set_user_data(Box::into_raw(arc_pool) as *mut ());
        },
    }
}

fn destroy_shm_pool(pool: &wl_shm_pool::WlShmPool) {
    let arc_pool = unsafe { Box::from_raw(pool.get_user_data() as *mut Arc<Pool>) };
    drop(arc_pool)
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

fn shm_pool_implementation() -> wl_shm_pool::Implementation<ShmGlobalData> {
    wl_shm_pool::Implementation {
        create_buffer: |evlh, data, _, pool, buffer, offset, width, height, stride, format| {
            if !data.formats.contains(&format) {
                buffer.post_error(wl_shm::Error::InvalidFormat as u32, String::new());
                return;
            }
            let arc_pool = unsafe { &*(pool.get_user_data() as *mut Arc<Pool>) };
            let data = Box::into_raw(Box::new(InternalBufferData {
                pool: arc_pool.clone(),
                data: BufferData {
                    offset: offset,
                    width: width,
                    height: height,
                    stride: stride,
                    format: format,
                },
            }));
            evlh.register(&buffer, buffer_implementation(), (), Some(destroy_buffer));
            buffer.set_user_data(data as *mut ());
        },
        resize: |_, _, _, pool, size| {
            let arc_pool = unsafe { &*(pool.get_user_data() as *mut Arc<Pool>) };
            match arc_pool.resize(size) {
                Ok(()) => {}
                Err(ResizeError::InvalidSize) => {
                    pool.post_error(
                        wl_shm::Error::InvalidFd as u32,
                        "Invalid new size for a wl_shm_pool.".into(),
                    );
                }
                Err(ResizeError::MremapFailed) => {
                    pool.post_error(wl_shm::Error::InvalidFd as u32, "mremap failed.".into());
                }
            }
        },
        destroy: |_, _, _, _| {},
    }
}

fn destroy_buffer(buffer: &wl_buffer::WlBuffer) {
    let buffer_data = unsafe { Box::from_raw(buffer.get_user_data() as *mut InternalBufferData) };
    drop(buffer_data)
}

fn buffer_implementation() -> wl_buffer::Implementation<()> {
    wl_buffer::Implementation {
        destroy: |_, _, _, _| {},
    }
}
