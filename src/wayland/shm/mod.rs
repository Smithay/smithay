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
//! use smithay::wayland::buffer::BufferHandler;
//! use smithay::wayland::shm::{ShmState, ShmHandler};
//! use smithay::delegate_shm;
//! use wayland_server::protocol::wl_shm::Format;
//!
//! # struct State { shm_state: ShmState };
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the ShmState.
//! // Here, we specify that Yuyv and C8 format are supported
//! // additionally to the standard Argb8888 and Xrgb8888.
//! let state = ShmState::new::<State, _>(
//!     &display.handle(),
//!     vec![Format::Yuyv, Format::C8],
//!     None // we don't provide a logger here
//! );
//!
//! // insert the shmstate into your compositor state.
//! // ..
//!
//! // provide the necessary trait implementations
//! impl BufferHandler for State {
//!     fn buffer_destroyed(&mut self, buffer: &wayland_server::protocol::wl_buffer::WlBuffer) {
//!         // All renderers can handle buffer destruction at this point.
//!         // Some parts of window management may also use this function.
//!     }
//! }
//! impl ShmHandler for State {
//!     fn shm_state(&self) -> &ShmState {
//!         &self.shm_state
//!     }
//! }
//! delegate_shm!(State);
//! ```
//!
//! Then, when you have a [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer)
//! and need to retrieve its contents, use the
//! [`with_buffer_contents`] function to do it:
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! # fn wrap(buffer: &wayland_server::protocol::wl_buffer::WlBuffer) {
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

use std::sync::Arc;

use wayland_server::{
    backend::GlobalId,
    protocol::{
        wl_buffer,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
    },
    Dispatch, DisplayHandle, GlobalDispatch, Resource,
};

mod handlers;
mod pool;

use crate::utils::UnmanagedResource;

use self::pool::Pool;

use super::buffer::BufferHandler;

/// State of SHM module
#[derive(Debug)]
pub struct ShmState {
    formats: Vec<wl_shm::Format>,
    shm: GlobalId,
    log: ::slog::Logger,
}

impl ShmState {
    /// Create a new SHM global advertizing the given formats.
    ///
    /// This global will always advertize `ARGB8888` and `XRGB8888` since these formats are required by the
    /// protocol. Formats given as an argument are also advertized.
    ///
    /// The global is directly created on the provided [`Display`](wayland_server::Display),
    /// and this function returns the a delegate type. The id provided by [`ShmState::global`] may be used to
    /// remove this global in the future.
    pub fn new<D, L>(display: &DisplayHandle, mut formats: Vec<wl_shm::Format>, logger: L) -> ShmState
    where
        D: GlobalDispatch<WlShm, ()>
            + Dispatch<WlShm, ()>
            + Dispatch<WlShmPool, ShmPoolUserData>
            + BufferHandler
            + ShmHandler
            + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger);

        // Mandatory formats
        formats.push(wl_shm::Format::Argb8888);
        formats.push(wl_shm::Format::Xrgb8888);

        let shm = display.create_global::<D, WlShm, _>(1, ());

        ShmState {
            formats,
            shm,
            log: log.new(slog::o!("smithay_module" => "shm_handler")),
        }
    }

    /// Returns the id of the [`WlShm`] global.
    pub fn global(&self) -> GlobalId {
        self.shm.clone()
    }
}

/// Shm global handler
pub trait ShmHandler {
    /// Return the Shm global state
    fn shm_state(&self) -> &ShmState;
}

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

impl From<UnmanagedResource> for BufferAccessError {
    fn from(_: UnmanagedResource) -> Self {
        Self::NotManaged
    }
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
    let data = buffer
        .data::<ShmBufferUserData>()
        .ok_or(BufferAccessError::NotManaged)?;

    match data.pool.with_data_slice(|slice| f(slice, data.data)) {
        Ok(t) => Ok(t),
        Err(()) => {
            // SIGBUS error occurred
            buffer.post_error(wl_shm::Error::InvalidFd, "Bad pool size.");
            Err(BufferAccessError::BadMap)
        }
    }
}

/// Call given closure with the contents of the given buffer for mutable access
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
pub fn with_buffer_contents_mut<F, T>(buffer: &wl_buffer::WlBuffer, f: F) -> Result<T, BufferAccessError>
where
    F: FnOnce(&mut [u8], BufferData) -> T,
{
    let data = buffer
        .data::<ShmBufferUserData>()
        .ok_or(BufferAccessError::NotManaged)?;

    match data.pool.with_data_slice_mut(|slice| f(slice, data.data)) {
        Ok(t) => Ok(t),
        Err(()) => {
            // SIGBUS error occurred
            buffer.post_error(wl_shm::Error::InvalidFd, "Bad pool size.");
            Err(BufferAccessError::BadMap)
        }
    }
}

/// Returns if the buffer has an alpha channel
///
/// Note: This is a best-effort, but it will never return
/// `false` for formats having an alpha channel
pub fn has_alpha(format: wl_shm::Format) -> bool {
    !matches!(
        format,
        wl_shm::Format::Xrgb8888
            | wl_shm::Format::C8
            | wl_shm::Format::Rgb332
            | wl_shm::Format::Bgr233
            | wl_shm::Format::Xrgb4444
            | wl_shm::Format::Xbgr4444
            | wl_shm::Format::Rgbx4444
            | wl_shm::Format::Bgrx4444
            | wl_shm::Format::Xrgb1555
            | wl_shm::Format::Xbgr1555
            | wl_shm::Format::Rgbx5551
            | wl_shm::Format::Bgrx5551
            | wl_shm::Format::Rgb565
            | wl_shm::Format::Bgr565
            | wl_shm::Format::Rgb888
            | wl_shm::Format::Bgr888
            | wl_shm::Format::Xbgr8888
            | wl_shm::Format::Rgbx8888
            | wl_shm::Format::Bgrx8888
            | wl_shm::Format::Xrgb2101010
            | wl_shm::Format::Xbgr2101010
            | wl_shm::Format::Rgbx1010102
            | wl_shm::Format::Bgrx1010102
            | wl_shm::Format::Yuyv
            | wl_shm::Format::Yvyu
            | wl_shm::Format::Uyvy
            | wl_shm::Format::Vyuy
            | wl_shm::Format::Ayuv
            | wl_shm::Format::Nv12
            | wl_shm::Format::Nv21
            | wl_shm::Format::Nv16
            | wl_shm::Format::Nv61
            | wl_shm::Format::Yuv410
            | wl_shm::Format::Yvu410
            | wl_shm::Format::Yuv411
            | wl_shm::Format::Yvu411
            | wl_shm::Format::Yuv420
            | wl_shm::Format::Yvu420
            | wl_shm::Format::Yuv422
            | wl_shm::Format::Yvu422
            | wl_shm::Format::Yuv444
            | wl_shm::Format::Yvu444
            | wl_shm::Format::R8
            | wl_shm::Format::R16
            | wl_shm::Format::Rg88
            | wl_shm::Format::Gr88
            | wl_shm::Format::Rg1616
            | wl_shm::Format::Gr1616
            | wl_shm::Format::Xrgb16161616f
            | wl_shm::Format::Xbgr16161616f
            | wl_shm::Format::Xyuv8888
            | wl_shm::Format::Vuy888
            | wl_shm::Format::Vuy101010
            | wl_shm::Format::Y210
            | wl_shm::Format::Y212
            | wl_shm::Format::Y216
            | wl_shm::Format::Y410
            | wl_shm::Format::Y412
            | wl_shm::Format::Y416
            | wl_shm::Format::Xvyu2101010
            | wl_shm::Format::Xvyu1216161616
            | wl_shm::Format::Xvyu16161616
            | wl_shm::Format::Y0l0
            | wl_shm::Format::X0l0
            | wl_shm::Format::Y0l2
            | wl_shm::Format::X0l2
            | wl_shm::Format::Yuv4208bit
            | wl_shm::Format::Yuv42010bit
            | wl_shm::Format::Xrgb8888A8
            | wl_shm::Format::Xbgr8888A8
            | wl_shm::Format::Rgbx8888A8
            | wl_shm::Format::Bgrx8888A8
            | wl_shm::Format::Rgb888A8
            | wl_shm::Format::Bgr888A8
            | wl_shm::Format::Rgb565A8
            | wl_shm::Format::Bgr565A8
            | wl_shm::Format::Nv24
            | wl_shm::Format::Nv42
            | wl_shm::Format::P210
            | wl_shm::Format::P010
            | wl_shm::Format::P012
            | wl_shm::Format::P016
            | wl_shm::Format::Nv15
            | wl_shm::Format::Q410
            | wl_shm::Format::Q401
            | wl_shm::Format::Xrgb16161616
            | wl_shm::Format::Xbgr16161616
    )
}

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

/// User data of WlShmPool
#[derive(Debug)]
pub struct ShmPoolUserData {
    inner: Arc<Pool>,
}

/// User data of shm WlBuffer
#[derive(Debug)]
pub struct ShmBufferUserData {
    pub(crate) pool: Arc<Pool>,
    pub(crate) data: BufferData,
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_shm {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_shm::WlShm: ()
        ] => $crate::wayland::shm::ShmState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_shm::WlShm: ()
        ] => $crate::wayland::shm::ShmState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_shm_pool::WlShmPool: $crate::wayland::shm::ShmPoolUserData
        ] => $crate::wayland::shm::ShmState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_buffer::WlBuffer: $crate::wayland::shm::ShmBufferUserData
        ] => $crate::wayland::shm::ShmState);
    };
}
