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
//! as specified by the wayland protocol) and obtain its `ShmGlobalToken`.
//!
//! ```
//! extern crate wayland_server;
//! extern crate smithay;
//!
//! use smithay::shm::ShmGlobal;
//! use wayland_server::protocol::wl_shm;
//!
//! # fn main() {
//! # let (_, mut event_loop) = wayland_server::create_display();
//!
//! // Insert the ShmGlobal as a handler to your event loop
//! // Here, we specify that Yuyv and C8 format are supported
//! // additionnaly to the standart Argb8888 and Xrgb8888.
//! let handler_id = event_loop.add_handler_with_init(ShmGlobal::new(
//!     vec![wl_shm::Format::Yuyv, wl_shm::Format::C8],
//!     None // we don't provide a logger here
//! ));
//! // Register this handler to advertise a wl_shm global of version 1
//! let shm_global = event_loop.register_global::<wl_shm::WlShm,ShmGlobal>(handler_id, 1);
//! // Retrieve the shm token for later use to access the buffers
//! let shm_token = {
//!     let state = event_loop.state();
//!     state.get_handler::<ShmGlobal>(handler_id).get_token()
//! };
//!
//! # }
//! ```
//!
//! Then, when you have a `WlBuffer` and need to retrieve its contents, use the token method to
//! do it:
//!
//! ```ignore
//! shm_token.with_buffer_contents(&buffer,
//!     |slice: &[u8], buffer_metadata: &BufferData| {
//!         // do something to draw it on the screen
//!     }
//! );
//! ```
//!
//! **Note**
//!
//! This handler makes itself safe regading the client providing a wrong size for the memory pool
//! by using a SIGBUS handler.
//!
//! If you are already using an handler for this signal, you probably don't want to use this handler.


use self::pool::{Pool, ResizeError};

use slog_or_stdlog;
use std::os::unix::io::RawFd;
use std::sync::Arc;

use wayland_server::{Client, Destroy, EventLoopHandle, GlobalHandler, Init, Resource, resource_is_registered};
use wayland_server::protocol::{wl_buffer, wl_shm, wl_shm_pool};

mod pool;

/// A global for handling SHM pool and buffers
///
/// You must register it to an event loop using `register_with_init`, or it will
/// quickly panic.
pub struct ShmGlobal {
    formats: Vec<wl_shm::Format>,
    handler_id: Option<usize>,
    log: ::slog::Logger,
}

impl ShmGlobal {
    /// Create a new SHM global advertizing given supported formats.
    ///
    /// This global will always advertize `ARGB8888` and `XRGB8888` format
    /// as they are required by the protocol. Formats given as argument
    /// as additionnaly advertized.
    pub fn new<L>(mut formats: Vec<wl_shm::Format>, logger: L) -> ShmGlobal
        where L: Into<Option<::slog::Logger>>
    {
        let log = slog_or_stdlog(logger);

        // always add the mandatory formats
        formats.push(wl_shm::Format::Argb8888);
        formats.push(wl_shm::Format::Xrgb8888);
        ShmGlobal {
            formats: formats,
            handler_id: None,
            log: log.new(o!("smithay_module" => "shm_handler")),
        }
    }

    /// Retreive a token from the SHM global.
    ///
    /// This can only be called once the `ShmGlobal` has been added to and event loop
    /// and has been initialized. If it is not the case, this method will panic.
    ///
    /// This is needed to retrieve the contents of the shm pools and buffers.
    pub fn get_token(&self) -> ShmGlobalToken {
        ShmGlobalToken { hid: self.handler_id.expect("ShmGlobal was not initialized.") }
    }
}

/// An SHM global token
///
/// It is needed to access the contents of the buffers & pools managed by the
/// associated `ShmGlobal`.
pub struct ShmGlobalToken {
    hid: usize,
}

/// Error that can occur when accessing an SHM buffer
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

impl ShmGlobalToken {
    /// Call given closure with the contents of the given buffer
    ///
    /// If the buffer is managed by the associated ShmGlobal, its contents are
    /// extracted and the closure is extracted with them:
    ///
    /// - The first argument is a data slice of the contents of the pool
    /// - The second argument is the specification of this buffer is this pool
    ///
    /// If the buffer is not managed by the associated ShmGlobal, the closure is not called
    /// and this method will return `Err(())` (this will be the case for an EGL buffer for example).
    pub fn with_buffer_contents<F>(&self, buffer: &wl_buffer::WlBuffer, f: F) -> Result<(), BufferAccessError>
        where F: FnOnce(&[u8], BufferData)
    {
        if !resource_is_registered::<_, ShmHandler>(buffer, self.hid) {
            return Err(BufferAccessError::NotManaged);
        }
        let data = unsafe { &*(buffer.get_user_data() as *mut InternalBufferData) };

        if data.pool
               .with_data_slice(|slice| f(slice, data.data))
               .is_err() {
            // SIGBUS error occured
            buffer.post_error(wl_shm::Error::InvalidFd as u32, "Bad pool size.".into());
            return Err(BufferAccessError::BadMap);
        }
        Ok(())
    }
}

impl Init for ShmGlobal {
    fn init(&mut self, evqh: &mut EventLoopHandle, _index: usize) {
        let id = evqh.add_handler_with_init(ShmHandler {
                                                my_id: ::std::usize::MAX,
                                                valid_formats: self.formats.clone(),
                                                log: self.log.clone(),
                                            });
        self.handler_id = Some(id);
    }
}

impl GlobalHandler<wl_shm::WlShm> for ShmGlobal {
    fn bind(&mut self, evqh: &mut EventLoopHandle, _: &Client, global: wl_shm::WlShm) {
        let hid = self.handler_id.expect("ShmGlobal was not initialized.");
        // register an handler for this shm
        evqh.register::<_, ShmHandler>(&global, hid);
        // and then the custom formats
        for f in &self.formats {
            global.format(*f);
        }
    }
}

struct ShmHandler {
    my_id: usize,
    valid_formats: Vec<wl_shm::Format>,
    log: ::slog::Logger,
}

impl Init for ShmHandler {
    fn init(&mut self, _evqh: &mut EventLoopHandle, index: usize) {
        self.my_id = index;
        debug!(self.log, "Init finished")
    }
}

impl wl_shm::Handler for ShmHandler {
    fn create_pool(&mut self, evqh: &mut EventLoopHandle, _client: &Client, shm: &wl_shm::WlShm,
                   pool: wl_shm_pool::WlShmPool, fd: RawFd, size: i32) {
        if size <= 0 {
            shm.post_error(wl_shm::Error::InvalidFd as u32,
                           "Invalid size for a new wl_shm_pool.".into());
            return;
        }
        let mmap_pool = match Pool::new(fd, size as usize, self.log.clone()) {
            Ok(p) => p,
            Err(()) => {
                shm.post_error(wl_shm::Error::InvalidFd as u32, format!("Failed mmap of fd {}.", fd));
                return
            }
        };
        let arc_pool = Box::new(Arc::new(mmap_pool));
        evqh.register_with_destructor::<_, ShmHandler, ShmHandler>(&pool, self.my_id);
        pool.set_user_data(Box::into_raw(arc_pool) as *mut ());
    }
}

impl Destroy<wl_shm_pool::WlShmPool> for ShmHandler {
    fn destroy(pool: &wl_shm_pool::WlShmPool) {
        let arc_pool = unsafe { Box::from_raw(pool.get_user_data() as *mut Arc<Pool>) };
        drop(arc_pool)
    }
}

declare_handler!(ShmHandler, wl_shm::Handler, wl_shm::WlShm);

/// Details of the contents of a buffer relative to its pool
#[derive(Copy,Clone,Debug)]
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

impl wl_shm_pool::Handler for ShmHandler {
    fn create_buffer(&mut self, evqh: &mut EventLoopHandle, _client: &Client,
                     pool: &wl_shm_pool::WlShmPool, buffer: wl_buffer::WlBuffer, offset: i32, width: i32,
                     height: i32, stride: i32, format: wl_shm::Format) {
        if !self.valid_formats.contains(&format) {
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
        evqh.register_with_destructor::<_, ShmHandler, ShmHandler>(&buffer, self.my_id);
        buffer.set_user_data(data as *mut ());
    }

    fn resize(&mut self, _evqh: &mut EventLoopHandle, _client: &Client, pool: &wl_shm_pool::WlShmPool,
              size: i32) {
        let arc_pool = unsafe { &*(pool.get_user_data() as *mut Arc<Pool>) };
        match arc_pool.resize(size) {
            Ok(()) => {}
            Err(ResizeError::InvalidSize) => {
                pool.post_error(wl_shm::Error::InvalidFd as u32,
                                "Invalid new size for a wl_shm_pool.".into());
            }
            Err(ResizeError::MremapFailed) => {
                pool.post_error(wl_shm::Error::InvalidFd as u32, "mremap failed.".into());
            }
        }
    }
}

impl Destroy<wl_buffer::WlBuffer> for ShmHandler {
    fn destroy(buffer: &wl_buffer::WlBuffer) {
        let buffer_data = unsafe { Box::from_raw(buffer.get_user_data() as *mut InternalBufferData) };
        drop(buffer_data)
    }
}

declare_handler!(ShmHandler, wl_shm_pool::Handler, wl_shm_pool::WlShmPool);

impl wl_buffer::Handler for ShmHandler {}

declare_handler!(ShmHandler, wl_buffer::Handler, wl_buffer::WlBuffer);
