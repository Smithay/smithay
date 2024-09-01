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
//! let state = ShmState::new::<State>(
//!     &display.handle(),
//!     vec![Format::Yuyv, Format::C8],
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
//!     |ptr: *const u8, len: usize, buffer_metadata: BufferData| {
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
//!     },
//!     Err(BufferAccessError::NotReadable) => {
//!         /* The client has not allowed reads to this buffer */
//!     },
//!     Err(BufferAccessError::NotWritable) => unreachable!("cannot be triggered by with_buffer_contents"),
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

use std::{
    any::Any,
    collections::HashSet,
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::GlobalId,
    protocol::{
        wl_buffer,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
    },
    Dispatch, DisplayHandle, GlobalDispatch, Resource, WEnum,
};

mod handlers;
mod pool;

use crate::{
    backend::allocator::format::get_bpp,
    utils::{hook::Hook, HookId, UnmanagedResource},
};

use self::pool::Pool;

use super::buffer::BufferHandler;

/// State of SHM module
#[derive(Debug)]
pub struct ShmState {
    formats: HashSet<wl_shm::Format>,
    shm: GlobalId,
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
    pub fn new<D>(display: &DisplayHandle, formats: impl IntoIterator<Item = wl_shm::Format>) -> ShmState
    where
        D: GlobalDispatch<WlShm, ()>
            + Dispatch<WlShm, ()>
            + Dispatch<WlShmPool, ShmPoolUserData>
            + BufferHandler
            + ShmHandler
            + 'static,
    {
        let mut formats = formats.into_iter().collect::<HashSet<_>>();

        // Mandatory formats
        formats.insert(wl_shm::Format::Argb8888);
        formats.insert(wl_shm::Format::Xrgb8888);

        let shm = display.create_global::<D, WlShm, _>(1, ());

        ShmState { formats, shm }
    }

    /// Returns the id of the [`WlShm`] global.
    pub fn global(&self) -> GlobalId {
        self.shm.clone()
    }

    /// Updates the list of formats advertised by the global.
    ///
    /// This will only affect new binds to the wl_shm global.
    ///
    /// Removing formats will cause old clients trying to create
    /// a buffer of a now unsupported format to be killed.
    ///
    /// This function will never remove the mandatory formats `ARGB8888` and `XRGB8888`.
    pub fn update_formats(&mut self, formats: impl IntoIterator<Item = wl_shm::Format>) {
        self.formats = formats.into_iter().collect::<HashSet<_>>();
        // Mandatory formats
        self.formats.insert(wl_shm::Format::Argb8888);
        self.formats.insert(wl_shm::Format::Xrgb8888);
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

    /// This buffer cannot be read by the compositor
    #[error("Client has not indicated read permission for the buffer")]
    NotReadable,

    /// This buffer cannot be written to by the compositor
    #[error("Client has not indicated write permission for the buffer")]
    NotWritable,
}

impl From<UnmanagedResource> for BufferAccessError {
    #[inline]
    fn from(_: UnmanagedResource) -> Self {
        Self::NotManaged
    }
}

/// Call given closure with the contents of the given buffer
///
/// If the buffer is managed by the provided `ShmGlobal`, its contents are
/// extracted and the closure is extracted with them:
///
/// - The first argument is a pointer to the contents of the pool
/// - The second argument is the length of the contents of the pool
/// - The third argument is the specification of this buffer is this pool
///
/// The pool cannot be provided as a slice since it is shared memory that could be mutated
/// by the client.
///
/// If the buffer is not managed by the provided `ShmGlobal`, the closure is not called
/// and this method will return `Err(BufferAccessError::NotManaged)` (this will be the case for an
/// EGL buffer for example).
///
/// # Safety
/// The pointer passed to the callback is only valid while the callback is running. The shared
/// memory may be mutated by the client at any time. Creating a reference or slice into this memory
/// is undefined behavior if it is mutated by the client while the reference exists.
pub fn with_buffer_contents<F, T>(buffer: &wl_buffer::WlBuffer, f: F) -> Result<T, BufferAccessError>
where
    F: FnOnce(*const u8, usize, BufferData) -> T,
{
    let data = buffer
        .data::<ShmBufferUserData>()
        .ok_or(BufferAccessError::NotManaged)?;

    match data.pool.with_data(|ptr, len| f(ptr, len, data.data)) {
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
/// - The first argument is a pointer to the contents of the pool
/// - The second argument is the length of the contents of the pool
/// - The third argument is the specification of this buffer is this pool
///
/// The pool cannot be provided as a slice since it is shared memory that could be mutated
/// by the client.
///
/// If the buffer is not managed by the provided `ShmGlobal`, the closure is not called
/// and this method will return `Err(BufferAccessError::NotManaged)` (this will be the case for an
/// EGL buffer for example).
///
/// # Safety
/// The pointer passed to the callback is only valid while the callback is running. The shared
/// memory may be mutated by the client at any time. Creating a reference or slice into this memory
/// is undefined behavior if it is mutated by the client while the reference exists.
pub fn with_buffer_contents_mut<F, T>(buffer: &wl_buffer::WlBuffer, f: F) -> Result<T, BufferAccessError>
where
    F: FnOnce(*mut u8, usize, BufferData) -> T,
{
    let data = buffer
        .data::<ShmBufferUserData>()
        .ok_or(BufferAccessError::NotManaged)?;

    match data.pool.with_data_mut(|ptr, len| f(ptr, len, data.data)) {
        Ok(t) => Ok(t),
        Err(()) => {
            // SIGBUS error occurred
            buffer.post_error(wl_shm::Error::InvalidFd, "Bad pool size.");
            Err(BufferAccessError::BadMap)
        }
    }
}

/// Returns the bpp of the format
///
/// Note: This will return 0 for formats that don't have a specified width.
pub fn wl_bytes_per_pixel(format: WEnum<wl_shm::Format>) -> i32 {
    match format {
        WEnum::Value(f) => {
            shm_format_to_fourcc(f).map_or(0, |fourcc| get_bpp(fourcc).map_or(0, |bpp| bpp / 8))
        }
        WEnum::Unknown(_) => 0,
    }
    .try_into()
    .unwrap()
}

macro_rules! shm_format_table {
    (
        $(
            $fourcc: ident => $shm: ident
        ),* $(,)?
    ) => {
        /// Convert from a [`Fourcc`](crate::backend::allocator::Fourcc) format to a wl_shm format
        pub const fn fourcc_to_shm_format(value: $crate::backend::allocator::Fourcc) -> Option<$crate::reexports::wayland_server::protocol::wl_shm::Format> {
            match value {
                $(
                    $crate::backend::allocator::Fourcc::$fourcc => Some($crate::reexports::wayland_server::protocol::wl_shm::Format::$shm),
                )*
                _ => None,
            }
        }

        /// Convert from a wl_shm format to a [`Fourcc`](crate::backend::allocator::Fourcc) format
        pub const fn shm_format_to_fourcc(value: $crate::reexports::wayland_server::protocol::wl_shm::Format) -> Option<$crate::backend::allocator::Fourcc> {
            match value {
                $(
                    $crate::reexports::wayland_server::protocol::wl_shm::Format::$shm => Some($crate::backend::allocator::Fourcc::$fourcc),
                )*
                _ => None,
            }
        }
    }
}

shm_format_table! {
    Argb8888 => Argb8888,
    Xrgb8888 => Xrgb8888,
    C8 => C8,
    Rgb332 => Rgb332,
    Bgr233 => Bgr233,
    Xrgb4444 => Xrgb4444,
    Xbgr4444 => Xbgr4444,
    Rgbx4444 => Rgbx4444,
    Bgrx4444 => Bgrx4444,
    Argb4444 => Argb4444,
    Abgr4444 => Abgr4444,
    Rgba4444 => Rgba4444,
    Bgra4444 => Bgra4444,
    Xrgb1555 => Xrgb1555,
    Xbgr1555 => Xbgr1555,
    Rgbx5551 => Rgbx5551,
    Bgrx5551 => Bgrx5551,
    Argb1555 => Argb1555,
    Abgr1555 => Abgr1555,
    Rgba5551 => Rgba5551,
    Bgra5551 => Bgra5551,
    Rgb565 => Rgb565,
    Bgr565 => Bgr565,
    Rgb888 => Rgb888,
    Bgr888 => Bgr888,
    Xbgr8888 => Xbgr8888,
    Rgbx8888 => Rgbx8888,
    Bgrx8888 => Bgrx8888,
    Abgr8888 => Abgr8888,
    Rgba8888 => Rgba8888,
    Bgra8888 => Bgra8888,
    Xrgb2101010 => Xrgb2101010,
    Xbgr2101010 => Xbgr2101010,
    Rgbx1010102 => Rgbx1010102,
    Bgrx1010102 => Bgrx1010102,
    Argb2101010 => Argb2101010,
    Abgr2101010 => Abgr2101010,
    Rgba1010102 => Rgba1010102,
    Bgra1010102 => Bgra1010102,
    Yuyv => Yuyv,
    Yvyu => Yvyu,
    Uyvy => Uyvy,
    Vyuy => Vyuy,
    Ayuv => Ayuv,
    Nv12 => Nv12,
    Nv21 => Nv21,
    Nv16 => Nv16,
    Nv61 => Nv61,
    Yuv410 => Yuv410,
    Yvu410 => Yvu410,
    Yuv411 => Yuv411,
    Yvu411 => Yvu411,
    Yuv420 => Yuv420,
    Yvu420 => Yvu420,
    Yuv422 => Yuv422,
    Yvu422 => Yvu422,
    Yuv444 => Yuv444,
    Yvu444 => Yvu444,
    R8 => R8,
    R16 => R16,
    Rg88 => Rg88,
    Gr88 => Gr88,
    Rg1616 => Rg1616,
    Gr1616 => Gr1616,
    Xrgb16161616f => Xrgb16161616f,
    Xbgr16161616f => Xbgr16161616f,
    Argb16161616f => Argb16161616f,
    Abgr16161616f => Abgr16161616f,
    Xyuv8888 => Xyuv8888,
    Vuy888 => Vuy888,
    Vuy101010 => Vuy101010,
    Y210 => Y210,
    Y212 => Y212,
    Y216 => Y216,
    Y410 => Y410,
    Y412 => Y412,
    Y416 => Y416,
    Xvyu2101010 => Xvyu2101010,
    Xvyu12_16161616 => Xvyu1216161616,
    Xvyu16161616 => Xvyu16161616,
    Y0l0 => Y0l0,
    X0l0 => X0l0,
    Y0l2 => Y0l2,
    X0l2 => X0l2,
    Yuv420_8bit => Yuv4208bit,
    Yuv420_10bit => Yuv42010bit,
    Xrgb8888_a8 => Xrgb8888A8,
    Xbgr8888_a8 => Xbgr8888A8,
    Rgbx8888_a8 => Rgbx8888A8,
    Bgrx8888_a8 => Bgrx8888A8,
    Rgb888_a8 => Rgb888A8,
    Bgr888_a8 => Bgr888A8,
    Rgb565_a8 => Rgb565A8,
    Bgr565_a8 => Bgr565A8,
    Nv24 => Nv24,
    Nv42 => Nv42,
    P210 => P210,
    P010 => P010,
    P012 => P012,
    P016 => P016,
    Axbxgxrx106106106106 => Axbxgxrx106106106106,
    Nv15 => Nv15,
    Q410 => Q410,
    Q401 => Q401,

    // these currently have no drm variant
    //Xrgb16161616 => Xrgb16161616,
    //Xbgr16161616 => Xbgr16161616,
    //Argb16161616 => Argb16161616,
    //Abgr16161616 => Abgr16161616,
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

type DestructionHook = dyn Fn(&mut dyn Any, &wl_buffer::WlBuffer) + Send + Sync;

/// User data of shm WlBuffer
#[derive(Debug)]
pub struct ShmBufferUserData {
    pub(crate) pool: Arc<Pool>,
    pub(crate) data: BufferData,
    destruction_hooks: Mutex<Vec<Hook<DestructionHook>>>,
}

impl ShmBufferUserData {
    pub(crate) fn add_destruction_hook(
        &self,
        hook: impl Fn(&mut dyn Any, &wl_buffer::WlBuffer) + Send + Sync + 'static,
    ) -> HookId {
        let hook: Hook<DestructionHook> = Hook::new(Arc::new(hook));
        let id = hook.id;
        self.destruction_hooks.lock().unwrap().push(hook);
        id
    }

    pub(crate) fn remove_destruction_hook(&self, hook_id: HookId) {
        let mut guard = self.destruction_hooks.lock().unwrap();
        if let Some(id) = guard.iter().position(|hook| hook.id != hook_id) {
            guard.remove(id);
        }
    }
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
