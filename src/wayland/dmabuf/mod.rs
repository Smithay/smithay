//! Linux DMABUF protocol
//!
//! This module provides helper to handle the linux-dmabuf protocol, which allows clients to submit their
//! contents as dmabuf file descriptors. These handlers automate the aggregation of the metadata associated
//! with a dma buffer, and do some basic checking of the sanity of what the client sends.
//!
//! This module is only available if the `backend_drm` cargo feature is enabled.
//!
//! ## How to use
//!
//! To setup the dmabuf global, you will need to provide 2 things:
//!
//! - a list of the dmabuf formats you wish to support
//! - an implementation of the `DmabufHandler` trait
//!
//! The list of supported format is just a `Vec<Format>`, where you will enter all the (format, modifier)
//! couples you support.
//!
//! The implementation of the `DmabufHandler` trait will be called whenever a client has finished setting up
//! a dma buffer. You will be handled the full details of the client's submission as a `BufferInfo` struct,
//! and you need to validate it and maybe import it into your renderer. The `BufferData` associated type
//! allows you to store any metadata or handle to the resource you need into the created `wl_buffer`,
//! user data, to then retrieve it when it is attached to a surface to re-identify the dmabuf.
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! use smithay::wayland::dmabuf::{DmabufHandler, BufferInfo, init_dmabuf_global};
//!
//! struct MyDmabufHandler;
//!
//! struct MyBufferData {
//!     /* ... */
//! }
//!
//! impl Drop for MyBufferData {
//!     fn drop(&mut self) {
//!         // This is called when all handles to this buffer have been dropped,
//!         // both client-side and server side.
//!         // You can now free the associated resources in your renderer.
//!     }
//! }
//!
//! impl DmabufHandler for MyDmabufHandler {
//!     type BufferData = MyBufferData;
//!     fn validate_dmabuf(&mut self, info: BufferInfo) -> Result<Self::BufferData, ()> {
//!         /* validate the dmabuf and import it into your renderer state */
//!         Ok(MyBufferData { /* ... */ })
//!     }
//! }
//!
//! // Once this is defined, you can in your setup initialize the dmabuf global:
//!
//! # let mut display = wayland_server::Display::new();
//! // define your supported formats
//! let formats = vec![
//!     /* ... */
//! ];
//! let dmabuf_global = init_dmabuf_global(
//!     &mut display,
//!     formats,
//!     MyDmabufHandler,
//!     None // we don't provide a logger in this example
//! );
//! ```

use std::{cell::RefCell, os::unix::io::RawFd, rc::Rc};

pub use wayland_protocols::unstable::linux_dmabuf::v1::server::zwp_linux_buffer_params_v1::Flags;
use wayland_protocols::unstable::linux_dmabuf::v1::server::{
    zwp_linux_buffer_params_v1::{
        Error as ParamError, Request as ParamsRequest, ZwpLinuxBufferParamsV1 as BufferParams,
    },
    zwp_linux_dmabuf_v1,
};
use wayland_server::{protocol::wl_buffer, Display, Global, Main, Filter};

/// Representation of a Dmabuf format, as advertized to the client
pub struct Format {
    /// The format identifier.
    pub format: ::drm::buffer::PixelFormat,
    /// The supported dmabuf layout modifier.
    ///
    /// This is an opaque token. Drivers use this token to express tiling, compression, etc. driver-specific
    /// modifications to the base format defined by the DRM fourcc code.
    pub modifier: u64,
    /// Number of planes used by this format
    pub plane_count: u32,
}

/// A plane send by the client
pub struct Plane {
    /// The file descriptor
    pub fd: RawFd,
    /// The plane index
    pub plane_idx: u32,
    /// Offset from the start of the Fd
    pub offset: u32,
    /// Stride for this plane
    pub stride: u32,
    /// Modifier for this plane
    pub modifier: u64,
}

bitflags! {
    pub struct BufferFlags: u32 {
        /// The buffer content is Y-inverted
        const Y_INVERT = 1;
        /// The buffer content is interlaced
        const INTERLACED = 2;
        /// The buffer content if interlaced is bottom-field first
        const BOTTOM_FIRST = 4;
    }
}

/// The complete information provided by the client to create a dmabuf buffer
pub struct BufferInfo {
    /// The submitted planes
    pub planes: Vec<Plane>,
    /// The width of this buffer
    pub width: i32,
    /// The height of this buffer
    pub height: i32,
    /// The format in use
    pub format: u32,
    /// The flags applied to it
    ///
    /// This is a bitflag, to be compared with the `Flags` enum reexported by this module.
    pub flags: BufferFlags,
}

/// Handler trait for dmabuf validation
///
/// You need to provide an implementation of this trait that will validate the parameters provided by the
/// client and import it as a dmabuf.
pub trait DmabufHandler {
    /// The data of a successfully imported dmabuf.
    ///
    /// This will be stored as the `user_data` of the `WlBuffer` associated with this dmabuf. If it has a
    /// destructor, it will be run when the client has destroyed the buffer and your compositor has dropped
    /// all of its `WlBuffer` handles to it.
    type BufferData: 'static;
    /// Validate a dmabuf
    ///
    /// From the information provided by the client, you need to validate and/or import the buffer.
    ///
    /// You can then store any information your compositor will need to handle it later, when the client has
    /// submitted the buffer by returning `Ok(BufferData)` where `BufferData` is the associated type of this,
    /// trait, a type of your choosing.
    ///
    /// If the buffer could not be imported, whatever the reason, return `Err(())`.
    fn validate_dmabuf(&mut self, info: BufferInfo) -> Result<Self::BufferData, ()>;
    /// Create a buffer from validated buffer data.
    ///
    /// This method is pre-implemented for you by storing the provided `BufferData` as the `user_data` of the
    /// provided `WlBuffer`. By default it assumes that your `BufferData` is not threadsafe.
    ///
    /// You can override it if you need your `BufferData` to be threadsafe, or which to register a destructor
    /// for the `WlBuffer` for example.
    fn create_buffer(
        &mut self,
        data: Self::BufferData,
        buffer: Main<wl_buffer::WlBuffer>,
    ) -> wl_buffer::WlBuffer {
        buffer.quick_assign(|_, _, _| {});
        buffer.as_ref().user_data().set(|| data);
        (*buffer).clone()
    }
}

/// Initialize a dmabuf global.
///
/// You need to provide a vector of the supported formats, as well as an implementation fo the `DmabufHandler`
/// trait, which will receive the buffer creation requests from the clients.
pub fn init_dmabuf_global<H, L>(
    display: &mut Display,
    formats: Vec<Format>,
    handler: H,
    logger: L,
) -> Global<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>
where
    L: Into<Option<::slog::Logger>>,
    H: DmabufHandler + 'static,
{
    let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "dmabuf_handler"));

    let max_planes = formats.iter().map(|f| f.plane_count).max().unwrap_or(0);
    let formats = Rc::new(formats);
    let handler = Rc::new(RefCell::new(handler));

    trace!(
        log,
        "Initializing DMABUF handler with {} supported formats",
        formats.len()
    );

    display.create_global(3, Filter::new(move |(dmabuf, version): (Main<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>, u32), _, _| {
        let dma_formats = formats.clone();
        let dma_handler = handler.clone();
        let dma_log = log.clone();
        dmabuf.quick_assign(
            move |_, req, _| {
                if let zwp_linux_dmabuf_v1::Request::CreateParams { params_id } = req {
                    let mut handler = ParamsHandler {
                        pending_planes: Vec::new(),
                        max_planes,
                        used: false,
                        formats: dma_formats.clone(),
                        handler: dma_handler.clone(),
                        log: dma_log.clone(),
                    };
                    params_id.quick_assign(move |params, req, _| match req {
                        ParamsRequest::Add { fd, plane_idx, offset, stride, modifier_hi, modifier_lo } => {
                            handler.add(&*params, fd, plane_idx, offset, stride, modifier_hi, modifier_lo)
                        },
                        ParamsRequest::Create { width, height, format, flags } => {
                            handler.create(&*params, width, height, format, flags)
                        },
                        ParamsRequest::CreateImmed { buffer_id, width, height, format, flags } => {
                            handler.create_immed(&*params, buffer_id, width, height, format, flags)
                        }
                        _ => {}
                    });
                }
            }
        );

        // send the supported formats
        for f in &*formats {
            dmabuf.format(f.format.as_raw());
            if version >= 3 {
                dmabuf.modifier(f.format.as_raw(), (f.modifier >> 32) as u32, f.modifier as u32);
            }
        }
    }))
}

struct ParamsHandler<H: DmabufHandler> {
    pending_planes: Vec<Plane>,
    max_planes: u32,
    used: bool,
    formats: Rc<Vec<Format>>,
    handler: Rc<RefCell<H>>,
    log: ::slog::Logger,
}

impl<H: DmabufHandler> ParamsHandler<H> {
    fn add(
        &mut self,
        params: &BufferParams,
        fd: RawFd,
        plane_idx: u32,
        offset: u32,
        stride: u32,
        modifier_hi: u32,
        modifier_lo: u32,
    ) {
        // protocol checks:
        // Cannot reuse a params:
        if self.used {
            params.as_ref().post_error(
                ParamError::AlreadyUsed as u32,
                "This buffer_params has already been used to create a buffer.".into(),
            );
            return;
        }
        // plane_idx is not too large
        if plane_idx >= self.max_planes {
            // plane_idx starts at 0
            params.as_ref().post_error(
                ParamError::PlaneIdx as u32,
                format!("Plane index {} is out of bounds.", plane_idx),
            );
            return;
        }
        // plane_idx has already been set
        if self.pending_planes.iter().any(|d| d.plane_idx == plane_idx) {
            params.as_ref().post_error(
                ParamError::PlaneSet as u32,
                format!("Plane index {} is already set.", plane_idx),
            );
            return;
        }
        // all checks passed, store the plane
        self.pending_planes.push(Plane {
            fd,
            plane_idx,
            offset,
            stride,
            modifier: ((modifier_hi as u64) << 32) + (modifier_lo as u64),
        });
    }

    fn create(&mut self, params: &BufferParams, width: i32, height: i32, format: u32, flags: u32) {
        // Cannot reuse a params:
        if self.used {
            params.as_ref().post_error(
                ParamError::AlreadyUsed as u32,
                "This buffer_params has already been used to create a buffer.".into(),
            );
            return;
        }
        self.used = true;
        if !buffer_basic_checks(
            &self.formats,
            &self.pending_planes,
            &params,
            format,
            width,
            height,
        ) {
            trace!(self.log, "Killing client providing bogus dmabuf buffer params.");
            return;
        }
        let info = BufferInfo {
            planes: ::std::mem::replace(&mut self.pending_planes, Vec::new()),
            width,
            height,
            format,
            flags: BufferFlags::from_bits_truncate(flags),
        };
        let mut handler = self.handler.borrow_mut();
        if let Ok(data) = handler.validate_dmabuf(info) {
            if let Some(buffer) = params
                .as_ref()
                .client()
                .and_then(|c| c.create_resource::<wl_buffer::WlBuffer>(1))
            {
                let buffer = handler.create_buffer(data, buffer);
                trace!(self.log, "Creating a new validated dma wl_buffer.");
                params.created(&buffer);
            }
        } else {
            trace!(self.log, "Refusing creation of an invalid dma wl_buffer.");
            params.failed();
        }
    }

    fn create_immed(
        &mut self,
        params: &BufferParams,
        buffer_id: Main<wl_buffer::WlBuffer>,
        width: i32,
        height: i32,
        format: u32,
        flags: u32,
    ) {
        // Cannot reuse a params:
        if self.used {
            params.as_ref().post_error(
                ParamError::AlreadyUsed as u32,
                "This buffer_params has already been used to create a buffer.".into(),
            );
            return;
        }
        self.used = true;
        if !buffer_basic_checks(
            &self.formats,
            &self.pending_planes,
            &params,
            format,
            width,
            height,
        ) {
            trace!(self.log, "Killing client providing bogus dmabuf buffer params.");
            return;
        }
        let info = BufferInfo {
            planes: ::std::mem::replace(&mut self.pending_planes, Vec::new()),
            width,
            height,
            format,
            flags: BufferFlags::from_bits_truncate(flags),
        };
        let mut handler = self.handler.borrow_mut();
        if let Ok(data) = handler.validate_dmabuf(info) {
            trace!(self.log, "Creating a new validated immediate dma wl_buffer.");
            handler.create_buffer(data, buffer_id);
        } else {
            trace!(
                self.log,
                "Refusing creation of an invalid immediate dma wl_buffer, killing client."
            );
            params.as_ref().post_error(
                ParamError::InvalidWlBuffer as u32,
                "create_immed resulted in an invalid buffer.".into(),
            );
        }
    }
}

fn buffer_basic_checks(
    formats: &[Format],
    pending_planes: &[Plane],
    params: &BufferParams,
    format: u32,
    width: i32,
    height: i32,
) -> bool {
    // protocol_checks:
    // This must be a known format
    let format = match formats.iter().find(|f| f.format.as_raw() == format) {
        Some(f) => f,
        None => {
            params.as_ref().post_error(
                ParamError::InvalidFormat as u32,
                format!("Format {:x} is not supported.", format),
            );
            return false;
        }
    };
    // The number of planes set must match what the format expects
    let max_plane_set = pending_planes.iter().map(|d| d.plane_idx + 1).max().unwrap_or(0);
    if max_plane_set != format.plane_count || pending_planes.len() < format.plane_count as usize {
        params.as_ref().post_error(
            ParamError::Incomplete as u32,
            format!(
                "Format {:?} requires {} planes but got {}.",
                format.format, format.plane_count, max_plane_set
            ),
        );
        return false;
    }
    // Width and height must be positivie
    if width < 1 || height < 1 {
        params.as_ref().post_error(
            ParamError::InvalidDimensions as u32,
            format!("Dimensions ({},{}) are not valid.", width, height),
        );
        return false;
    }
    // check the size of each plane buffer
    for plane in pending_planes {
        // check size for overflow
        let end = match plane
            .stride
            .checked_mul(height as u32)
            .and_then(|o| o.checked_add(plane.offset))
        {
            None => {
                params.as_ref().post_error(
                    ParamError::OutOfBounds as u32,
                    format!("Size overflow for plane {}.", plane.plane_idx),
                );
                return false;
            }
            Some(e) => e,
        };
        if let Ok(size) = ::nix::unistd::lseek(plane.fd, 0, ::nix::unistd::Whence::SeekEnd) {
            // reset the seek point
            let _ = ::nix::unistd::lseek(plane.fd, 0, ::nix::unistd::Whence::SeekSet);
            if plane.offset as i64 > size {
                params.as_ref().post_error(
                    ParamError::OutOfBounds as u32,
                    format!("Invalid offset {} for plane {}.", plane.offset, plane.plane_idx),
                );
                return false;
            }
            if (plane.offset + plane.stride) as i64 > size {
                params.as_ref().post_error(
                    ParamError::OutOfBounds as u32,
                    format!("Invalid stride {} for plane {}.", plane.stride, plane.plane_idx),
                );
                return false;
            }
            // Planes > 0 can be subsampled, in which case 'size' will be smaller
            // than expected.
            if plane.plane_idx == 0 && end as i64 > size {
                params.as_ref().post_error(
                    ParamError::OutOfBounds as u32,
                    format!(
                        "Invalid stride ({}) or height ({}) for plane {}.",
                        plane.stride, height, plane.plane_idx
                    ),
                );
                return false;
            }
        }
    }
    true
}
