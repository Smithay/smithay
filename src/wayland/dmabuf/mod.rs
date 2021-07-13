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
//! The list of supported format is just a `Vec<Format>`, where you will enter all the (code, modifier)
//! couples you support. You can typically receive a list of supported formats for one renderer by calling
//! [`ImportDma::dmabuf_formats`](crate::backend::renderer::ImportDma::dmabuf_formats).
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! use smithay::{
//!     backend::allocator::dmabuf::Dmabuf,
//!     reexports::{wayland_server::protocol::wl_buffer::WlBuffer},
//!     wayland::dmabuf::init_dmabuf_global,
//! };
//!
//! # let mut display = wayland_server::Display::new();
//! // define your supported formats
//! let formats = vec![
//!     /* ... */
//! ];
//! let dmabuf_global = init_dmabuf_global(
//!     &mut display,
//!     formats,
//!     |buffer, dispatch_data| {
//!         /* validate the dmabuf and import it into your renderer state */
//!         true
//!     },
//!     None // we don't provide a logger in this example
//! );
//! ```

use std::{
    cell::RefCell,
    convert::TryFrom,
    os::unix::io::{IntoRawFd, RawFd},
    rc::Rc,
};

use wayland_protocols::unstable::linux_dmabuf::v1::server::{
    zwp_linux_buffer_params_v1::{
        Error as ParamError, Request as ParamsRequest, ZwpLinuxBufferParamsV1 as BufferParams,
    },
    zwp_linux_dmabuf_v1,
};
use wayland_server::{protocol::wl_buffer, DispatchData, Display, Filter, Global, Main};

use slog::{o, trace};

use crate::backend::allocator::{
    dmabuf::{Dmabuf, DmabufFlags, Plane},
    Format, Fourcc, Modifier,
};

/// Handler trait for dmabuf validation
///
/// You need to provide an implementation of this trait

/// Initialize a dmabuf global.
///
/// You need to provide a vector of the supported formats, as well as a closure,
/// that will validate the parameters provided by the client and tests the import as a dmabuf.
pub fn init_dmabuf_global<F, L>(
    display: &mut Display,
    formats: Vec<Format>,
    handler: F,
    logger: L,
) -> Global<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>
where
    L: Into<Option<::slog::Logger>>,
    F: for<'a> FnMut(&Dmabuf, DispatchData<'a>) -> bool + 'static,
{
    let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "dmabuf_handler"));

    let formats = Rc::<[Format]>::from(formats);
    let handler = Rc::new(RefCell::new(handler));

    trace!(
        log,
        "Initializing DMABUF handler with {} supported formats",
        formats.len()
    );

    display.create_global(
        3,
        Filter::new(
            move |(dmabuf, version): (Main<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>, u32), _, _| {
                let dma_formats = formats.clone();
                let dma_handler = handler.clone();
                let dma_log = log.clone();
                dmabuf.quick_assign(move |_, req, _| {
                    if let zwp_linux_dmabuf_v1::Request::CreateParams { params_id } = req {
                        let mut handler = ParamsHandler {
                            pending_planes: Vec::new(),
                            max_planes: 4,
                            used: false,
                            formats: dma_formats.clone(),
                            handler: dma_handler.clone(),
                            log: dma_log.clone(),
                        };
                        params_id.quick_assign(move |params, req, ddata| match req {
                            ParamsRequest::Add {
                                fd,
                                plane_idx,
                                offset,
                                stride,
                                modifier_hi,
                                modifier_lo,
                            } => handler.add(
                                &*params,
                                fd,
                                plane_idx,
                                offset,
                                stride,
                                ((modifier_hi as u64) << 32) + (modifier_lo as u64),
                            ),
                            ParamsRequest::Create {
                                width,
                                height,
                                format,
                                flags,
                            } => handler.create(&*params, width, height, format, flags, ddata),
                            ParamsRequest::CreateImmed {
                                buffer_id,
                                width,
                                height,
                                format,
                                flags,
                            } => {
                                handler.create_immed(&*params, buffer_id, width, height, format, flags, ddata)
                            }
                            _ => {}
                        });
                    }
                });

                // send the supported formats
                for f in &*formats {
                    dmabuf.format(f.code as u32);
                    if version >= 3 {
                        dmabuf.modifier(
                            f.code as u32,
                            (Into::<u64>::into(f.modifier) >> 32) as u32,
                            Into::<u64>::into(f.modifier) as u32,
                        );
                    }
                }
            },
        ),
    )
}

struct ParamsHandler<H: for<'a> FnMut(&Dmabuf, DispatchData<'a>) -> bool + 'static> {
    pending_planes: Vec<Plane>,
    max_planes: u32,
    used: bool,
    formats: Rc<[Format]>,
    handler: Rc<RefCell<H>>,
    log: ::slog::Logger,
}

impl<H> ParamsHandler<H>
where
    H: for<'a> FnMut(&Dmabuf, DispatchData<'a>) -> bool + 'static,
{
    fn add(
        &mut self,
        params: &BufferParams,
        fd: RawFd,
        plane_idx: u32,
        offset: u32,
        stride: u32,
        modifier: u64,
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
            fd: Some(fd),
            plane_idx,
            offset,
            stride,
            modifier: Modifier::from(modifier),
        });
    }

    fn create<'a>(
        &mut self,
        params: &BufferParams,
        width: i32,
        height: i32,
        format: u32,
        flags: u32,
        ddata: DispatchData<'a>,
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

        let format = match Fourcc::try_from(format) {
            Ok(format) => format,
            Err(_) => {
                params.as_ref().post_error(
                    ParamError::InvalidFormat as u32,
                    format!("Format {:x} is not supported", format),
                );
                return;
            }
        };

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

        let mut buf = Dmabuf::builder((width, height), format, DmabufFlags::from_bits_truncate(flags));
        let planes = std::mem::take(&mut self.pending_planes);
        for (i, plane) in planes.into_iter().enumerate() {
            let offset = plane.offset;
            let stride = plane.stride;
            let modi = plane.modifier;
            buf.add_plane(plane.into_raw_fd(), i as u32, offset, stride, modi);
        }
        let dmabuf = match buf.build() {
            Some(buf) => buf,
            None => {
                params.as_ref().post_error(
                    ParamError::Incomplete as u32,
                    "Provided buffer is incomplete, it has zero planes".to_string(),
                );
                return;
            }
        };

        let mut handler = self.handler.borrow_mut();
        if handler(&dmabuf, ddata) {
            if let Some(buffer) = params
                .as_ref()
                .client()
                .and_then(|c| c.create_resource::<wl_buffer::WlBuffer>(1))
            {
                buffer.as_ref().user_data().set_threadsafe(|| dmabuf);
                buffer.quick_assign(|_, _, _| {});

                trace!(self.log, "Created a new validated dma wl_buffer.");
                params.created(&buffer);
            } else {
                trace!(self.log, "Failed to create a wl_buffer");
                params.failed();
            }
        } else {
            trace!(self.log, "Refusing creation of an invalid dma wl_buffer.");
            params.failed();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_immed<'a>(
        &mut self,
        params: &BufferParams,
        buffer: Main<wl_buffer::WlBuffer>,
        width: i32,
        height: i32,
        format: u32,
        flags: u32,
        ddata: DispatchData<'a>,
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

        let format = match Fourcc::try_from(format) {
            Ok(format) => format,
            Err(_) => {
                params.as_ref().post_error(
                    ParamError::InvalidFormat as u32,
                    format!("Format {:x} is not supported", format),
                );
                return;
            }
        };

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

        let mut buf = Dmabuf::builder((width, height), format, DmabufFlags::from_bits_truncate(flags));
        let planes = ::std::mem::take(&mut self.pending_planes);
        for (i, plane) in planes.into_iter().enumerate() {
            let offset = plane.offset;
            let stride = plane.stride;
            let modi = plane.modifier;
            buf.add_plane(plane.into_raw_fd(), i as u32, offset, stride, modi);
        }
        let dmabuf = match buf.build() {
            Some(buf) => buf,
            None => {
                params.as_ref().post_error(
                    ParamError::Incomplete as u32,
                    "Provided buffer is incomplete, it has zero planes".to_string(),
                );
                return;
            }
        };

        let mut handler = self.handler.borrow_mut();
        if handler(&dmabuf, ddata) {
            buffer.as_ref().user_data().set_threadsafe(|| dmabuf);
            buffer.quick_assign(|_, _, _| {});
            trace!(self.log, "Created a new validated dma wl_buffer.");
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
    format: Fourcc,
    width: i32,
    height: i32,
) -> bool {
    // protocol_checks:
    // This must be a known format
    let _format = match formats.iter().find(|f| f.code == format) {
        Some(f) => f,
        None => {
            params.as_ref().post_error(
                ParamError::InvalidFormat as u32,
                format!("Format {:?}/{:x} is not supported.", format, format as u32),
            );
            return false;
        }
    };
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
        if let Ok(size) = ::nix::unistd::lseek(plane.fd.unwrap(), 0, ::nix::unistd::Whence::SeekEnd) {
            // reset the seek point
            let _ = ::nix::unistd::lseek(plane.fd.unwrap(), 0, ::nix::unistd::Whence::SeekSet);
            if plane.offset as libc::off_t > size {
                params.as_ref().post_error(
                    ParamError::OutOfBounds as u32,
                    format!("Invalid offset {} for plane {}.", plane.offset, plane.plane_idx),
                );
                return false;
            }
            if (plane.offset + plane.stride) as libc::off_t > size {
                params.as_ref().post_error(
                    ParamError::OutOfBounds as u32,
                    format!("Invalid stride {} for plane {}.", plane.stride, plane.plane_idx),
                );
                return false;
            }
            // Planes > 0 can be subsampled, in which case 'size' will be smaller
            // than expected.
            if plane.plane_idx == 0 && end as libc::off_t > size {
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
