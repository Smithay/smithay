//! Linux DMABUF protocol
//!
//! This module provides helper to handle the linux-dmabuf protocol, which allows clients to submit their
//! contents as dmabuf file descriptors. These handlers automate the aggregation of the metadata associated
//! with a dma buffer, and do some basic checking of the sanity of what the client sends.
//!
//! ## How to use
//!
//! To setup the dmabuf global, you will need to provide 2 things:
//!
//! - a list of the dmabuf formats you wish to support
//! - a closure to test if a dmabuf buffer can be imported by your renderer
//!
//! The list of supported formats is a `Vec<Format>`, where you will enter all the (code, modifier) pairs you
//! support. You can typically receive a list of supported formats for one renderer by calling
//! [`ImportDma::dmabuf_formats`](crate::backend::renderer::ImportDma::dmabuf_formats).
//!
//! Accessing a [`Dmabuf`] associated with a [`Buffer`] may be achieved using [`get_dmabuf`].
//!
//! ```no_run
//! # extern crate wayland_server;
//! use smithay::{
//!     delegate_dmabuf,
//!     backend::allocator::dmabuf::Dmabuf,
//!     reexports::{wayland_server::protocol::wl_buffer::WlBuffer},
//!     wayland::{
//!         buffer::{Buffer, BufferHandler},
//!         dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportError}
//!     },
//! };
//!
//! pub struct State {
//!     dmabuf_state: DmabufState,
//!     dmabuf_global: DmabufGlobal,
//! }
//!
//! // Smithay's "DmabufHandler" also requires the buffer management utilities, you need to implement
//! // "BufferHandler".
//! impl BufferHandler for State {
//!     fn buffer_destroyed(&mut self, buffer: &Buffer) {
//!         // All renderers can handle buffer destruction at this point. Some parts of window management may
//!         // also use this function.
//!         //
//!         // If you need to mark a dmabuf elsewhere in your state as destroyed, you use the "get_dmabuf"
//!         // function defined in this module to access the dmabuf associated the "Buffer".
//!     }
//! }
//!
//! impl DmabufHandler for State {
//!     fn dmabuf_state(&mut self) -> &mut DmabufState {
//!         &mut self.dmabuf_state
//!     }
//!
//!     fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf) -> Result<(), ImportError> {
//!         // Here you should import the dmabuf into your renderer.
//!         //
//!         // The return value indicates whether import was successful. If the return value is Err, then
//!         // the client is told dmabuf import has failed.
//!         Ok(())
//!     }
//! }
//!
//! // Delegate dmabuf handling for State to DmabufState.
//! delegate_dmabuf!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! // First a DmabufState must be created. This type is used to create some "DmabufGlobal"s
//! let mut dmabuf_state = DmabufState::new();
//!
//! // define your supported formats
//! let formats = vec![
//!     /* ... */
//! ];
//!
//! // And create the dmabuf global.
//! let dmabuf_global = dmabuf_state.create_global::<State, _>(
//!     &display_handle,
//!     formats,
//!     None // we don't provide a logger in this example
//! );
//!
//! let state = State {
//!     dmabuf_state,
//!     dmabuf_global,
//! };
//!
//! // Rest of the compositor goes here...
//! ```

mod dispatch;

use std::{
    collections::HashMap,
    convert::TryFrom,
    os::unix::io::IntoRawFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use nix::unistd;
use wayland_protocols::wp::linux_dmabuf::zv1::server::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1};
use wayland_server::{backend::GlobalId, Client, DisplayHandle, GlobalDispatch, Resource, WEnum};

use crate::{
    backend::allocator::{
        dmabuf::{Dmabuf, DmabufFlags, Plane},
        Format, Fourcc, Modifier,
    },
    utils::{ids::id_gen, UnmanagedResource},
};

use super::buffer::{Buffer, BufferHandler};

/// Delegate type for all dmabuf globals.
///
/// Dmabuf globals are created using this type and events will be forwarded to an instance of the dmabuf global.
#[derive(Debug)]
pub struct DmabufState {
    /// Globals managed by the dmabuf handler.
    globals: HashMap<usize, GlobalId>,
}

impl DmabufState {
    /// Creates a new [`DmabufState`] delegate type.
    #[allow(clippy::new_without_default)]
    pub fn new() -> DmabufState {
        DmabufState {
            globals: HashMap::new(),
        }
    }

    /// Creates a dmabuf global with the specified supported formats.
    pub fn create_global<D, L>(
        &mut self,
        display: &DisplayHandle,
        formats: Vec<Format>,
        logger: L,
    ) -> DmabufGlobal
    where
        D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
            + BufferHandler
            + DmabufHandler
            + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        self.create_global_with_filter::<D, _, L>(display, formats, |_| true, logger)
    }

    /// Creates a dmabuf global with the specified supported formats.
    ///
    /// This function unlike [`DmabufState::create_global`] also allows you to specify a filter function to
    /// determine which clients may see this global. This functionality may be used on multi-gpu systems in
    /// order to make a client choose the correct gpu.
    pub fn create_global_with_filter<D, F, L>(
        &mut self,
        display: &DisplayHandle,
        formats: Vec<Format>,
        filter: F,
        logger: L,
    ) -> DmabufGlobal
    where
        D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
            + BufferHandler
            + DmabufHandler
            + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let id = next_global_id();
        let logger = crate::slog_or_fallback(logger)
            .new(slog::o!("smithay_module" => "wayland_dmabuf", "global" => id));
        let formats = Arc::new(formats);
        let data = DmabufGlobalData {
            filter: Box::new(filter),
            formats,
            id,
            logger,
        };

        let global =
            display.create_global::<D, zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _>(GLOBAL_VERSION, data);
        self.globals.insert(id, global);

        DmabufGlobal { id }
    }

    /// Disables a dmabuf global.
    ///
    /// This operation is permanent and there is no way to re-enable a global.
    pub fn disable_global<D: 'static>(&mut self, display: &DisplayHandle, global: &DmabufGlobal) {
        display.disable_global(self.globals.get(&global.id).unwrap().clone())
    }

    /// Destroys a dmabuf global.
    ///
    /// It is highly recommended you disable the global before destroying it and ensure all child objects have
    /// been destroyed.
    pub fn destroy_global<D: 'static>(&mut self, display: &DisplayHandle, global: DmabufGlobal) {
        display.remove_global(self.globals.remove(&global.id).unwrap());
        DMABUF_GLOBAL_IDS.lock().unwrap().remove(&global.id);
    }
}

/// Data associated with a dmabuf global.
#[allow(missing_debug_implementations)]
pub struct DmabufGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
    formats: Arc<Vec<Format>>,
    id: usize,
    logger: slog::Logger,
}

/// Data associated with a dmabuf global protocol object.
#[derive(Debug)]
pub struct DmabufData {
    formats: Arc<Vec<Format>>,
    id: usize,
    logger: slog::Logger,
}

/// Data associated with a pending [`Dmabuf`] import.
#[derive(Debug)]
pub struct DmabufParamsData {
    /// Id of the dmabuf global these params were created from.
    id: usize,

    /// Whether the params protocol object has been used before to create a wl_buffer.
    used: AtomicBool,

    formats: Arc<Vec<Format>>,

    /// Pending planes for the params.
    planes: Mutex<Vec<Plane>>,

    logger: slog::Logger,
}

/// A handle to a registered dmabuf global.
///
/// This type may be used in equitability checks to determine which global a dmabuf is being imported to.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DmabufGlobal {
    id: usize,
    // note this type should never be `Clone` or `Copy`
}

/// Handler trait for [`Dmabuf`] import from the compositor.
pub trait DmabufHandler: BufferHandler {
    /// Returns a mutable reference to the [`DmabufState`] delegate type.
    fn dmabuf_state(&mut self) -> &mut DmabufState;

    /// This function is called when a client has imported a [`Dmabuf`].
    ///
    /// The `global` indicates which [`DmabufGlobal`] the buffer was imported to. You should import the dmabuf
    /// into your renderer to ensure the dmabuf may be used later when rendering.
    ///
    /// The return value of this function indicates whether dmabuf import is successful. The renderer is
    /// responsible for determining whether the format and plane combinations are valid and should return
    /// [`ImportError::InvalidFormat`] if the format and planes are not correct.
    ///
    /// If the import fails due to an implementation specific reason, then [`ImportError::Failed`] should be
    /// returned.
    fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf) -> Result<(), ImportError>;
}

/// Error that may occur when importing a [`Dmabuf`].
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Buffer import failed for a renderer implementation specific reason.
    ///
    /// Depending on the request sent by the client, this error may notify the client that buffer import
    /// failed or will kill the client.
    #[error("buffer import failed")]
    Failed,

    /// The format and plane combination is not valid.
    ///
    /// This specific error will kill the client providing the dmabuf.
    #[error("format and plane combination is not valid")]
    InvalidFormat,
}

/// Gets the contents of a [`Dmabuf`] backed [`Buffer`].
///
/// If the buffer is managed by the dmabuf handler, the [`Dmabuf`] is returned.
///
/// If the buffer is not managed by the dmabuf handler (whether the buffer is a different kind of buffer,
/// such as an shm buffer or is not managed by smithay), this function will return an [`UnmanagedResource`]
/// error.
pub fn get_dmabuf(buffer: &Buffer) -> Result<Dmabuf, UnmanagedResource> {
    Ok(buffer.buffer_data::<Dmabuf>()?.ok_or(UnmanagedResource)?.clone())
}

/// Macro to delegate implementation of the linux dmabuf to [`DmabufState`].
///
/// You must also implement [`DmabufHandler`] to use this.
#[macro_export]
macro_rules! delegate_dmabuf {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __ZwpLinuxDmabufV1 =
            $crate::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
        type __ZwpLinuxBufferParamsV1 =
            $crate::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwpLinuxDmabufV1: $crate::wayland::dmabuf::DmabufGlobalData
        ] => $crate::wayland::dmabuf::DmabufState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwpLinuxDmabufV1: $crate::wayland::dmabuf::DmabufData
        ] => $crate::wayland::dmabuf::DmabufState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwpLinuxBufferParamsV1: $crate::wayland::dmabuf::DmabufParamsData
        ] => $crate::wayland::dmabuf::DmabufState);
    };
}

const GLOBAL_VERSION: u32 = 3;

impl DmabufParamsData {
    /// Emits a protocol error if the params have already been used to create a dmabuf.
    ///
    /// This returns true if the protocol object has not been used.
    fn ensure_unused(
        &self,
        dh: &DisplayHandle,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    ) -> bool {
        if !self.used.load(Ordering::Relaxed) {
            return true;
        }

        params.post_error(
            dh,
            zwp_linux_buffer_params_v1::Error::AlreadyUsed,
            "This buffer_params has already been used to create a buffer.",
        );

        false
    }

    /// Attempt to create a Dmabuf from the parameters.
    ///
    /// This function will perform the necessary validation of all the parameters, emitting protocol errors as
    /// needed.
    ///
    /// A return value of [`None`] indicates buffer import has failed and the client has been killed.
    fn create_dmabuf(
        &self,
        dh: &DisplayHandle,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        width: i32,
        height: i32,
        format: u32,
        flags: WEnum<zwp_linux_buffer_params_v1::Flags>,
    ) -> Option<Dmabuf> {
        // We cannot create a dmabuf if the parameters have already been used.
        if !self.ensure_unused(dh, params) {
            return None;
        }

        self.used.store(true, Ordering::Relaxed);

        let format = match Fourcc::try_from(format) {
            Ok(format) => format,
            Err(_) => {
                params.post_error(
                    dh,
                    zwp_linux_buffer_params_v1::Error::InvalidFormat,
                    format!("Format {:x} is not supported", format),
                );

                return None;
            }
        };

        // Validate buffer parameters:
        // 1. Must have known format
        if !self.formats.iter().any(|f| f.code == format) {
            params.post_error(
                dh,
                zwp_linux_buffer_params_v1::Error::InvalidFormat,
                format!("Format {:?}/{:x} is not supported.", format, format as u32),
            );
            return None;
        }

        // 2. Width and height must be positive
        if width < 1 {
            params.post_error(
                dh,
                zwp_linux_buffer_params_v1::Error::InvalidDimensions,
                "invalid width",
            );
        }

        if height < 1 {
            params.post_error(
                dh,
                zwp_linux_buffer_params_v1::Error::InvalidDimensions,
                "invalid height",
            );
        }

        // 3. Validate all the planes
        let mut planes = self.planes.lock().unwrap();

        for plane in &*planes {
            // Must not overflow
            let end = match plane
                .stride
                .checked_mul(height as u32)
                .and_then(|o| o.checked_add(plane.offset))
            {
                Some(e) => e,

                None => {
                    params.post_error(
                        dh,
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!("Size overflow for plane {}.", plane.plane_idx),
                    );

                    return None;
                }
            };

            if let Ok(size) = unistd::lseek(plane.fd.unwrap(), 0, unistd::Whence::SeekEnd) {
                // Reset seek point
                let _ = unistd::lseek(plane.fd.unwrap(), 0, unistd::Whence::SeekSet);

                if plane.offset as libc::off_t > size {
                    params.post_error(
                        dh,
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!("Invalid offset {} for plane {}.", plane.offset, plane.plane_idx),
                    );

                    return None;
                }

                if (plane.offset + plane.stride) as libc::off_t > size {
                    params.post_error(
                        dh,
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!("Invalid stride {} for plane {}.", plane.stride, plane.plane_idx),
                    );

                    return None;
                }

                // Planes > 0 can be subsampled, in which case 'size' will be smaller than expected.
                if plane.plane_idx == 0 && end as libc::off_t > size {
                    params.post_error(
                        dh,
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!(
                            "Invalid stride ({}) or height ({}) for plane {}.",
                            plane.stride, height, plane.plane_idx
                        ),
                    );

                    return None;
                }
            }
        }

        let mut buf = Dmabuf::builder(
            (width, height),
            format,
            DmabufFlags::from_bits_truncate(flags.into()),
        );

        for (i, plane) in planes.drain(..).enumerate() {
            let offset = plane.offset;
            let stride = plane.stride;
            let modi = plane.modifier;
            buf.add_plane(plane.into_raw_fd(), i as u32, offset, stride, modi);
        }

        let dmabuf = match buf.build() {
            Some(buf) => buf,

            None => {
                params.post_error(
                    dh,
                    zwp_linux_buffer_params_v1::Error::Incomplete as u32,
                    "Provided buffer is incomplete, it has zero planes",
                );
                return None;
            }
        };

        Some(dmabuf)
    }
}

id_gen!(next_global_id, DMABUF_GLOBAL_ID, DMABUF_GLOBAL_IDS);
