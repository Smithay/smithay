use std::{cell::RefCell, os::unix::io::RawFd, rc::Rc};

pub use wayland_protocols::unstable::linux_dmabuf::v1::server::zwp_linux_buffer_params_v1::Flags;
use wayland_protocols::unstable::linux_dmabuf::v1::server::{
    zwp_linux_buffer_params_v1::{
        Error as ParamError, RequestHandler as ParamRequestHandler, ZwpLinuxBufferParamsV1 as BufferParams,
    },
    zwp_linux_dmabuf_v1,
};
use wayland_server::{protocol::wl_buffer, Display, Global, NewResource};

/// Representation of a Dmabuf format, as advertized to the client
pub struct Format {
    /// The format identifier
    pub format: u32,
    /// High part of the supported modifiers
    pub modifier_hi: u32,
    /// Low part of the supported modifiers
    pub modifier_lo: u32,
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
    /// High part of the modifiers for this plane
    pub modifier_hi: u32,
    /// Low part of the modifiers for this plane
    pub modifier_lo: u32,
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
    pub flags: u32,
}

/// Handler trait for dmabuf validation
///
/// You need to provide an implementation of this trait that will validate the parameters provided by the
/// client and import it as a dmabuf.
pub trait DmabufHandler {
    /// The data of a successfully imported dmabuf.
    ///
    /// This will be stored as the `user_data` of the `WlBuffer` associated with this dmabuf.
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
        buffer: NewResource<wl_buffer::WlBuffer>,
    ) -> wl_buffer::WlBuffer {
        buffer.implement_closure(|_, _| {}, None::<fn(_)>, data)
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
    let log = crate::slog_or_stdlog(logger);

    let max_planes = formats.iter().map(|f| f.plane_count).max().unwrap_or(0);
    let formats = Rc::new(formats);
    let handler = Rc::new(RefCell::new(handler));

    display.create_global(3, move |new_dmabuf, version| {
        let dma_formats = formats.clone();
        let dma_handler = handler.clone();
        let dmabuf: zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = new_dmabuf.implement_closure(
            move |req, _| match req {
                zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                    params_id.implement(
                        ParamsHandler {
                            pending_planes: Vec::new(),
                            max_planes,
                            used: false,
                            formats: dma_formats.clone(),
                            handler: dma_handler.clone(),
                        },
                        None::<fn(_)>,
                        (),
                    );
                }
                _ => (),
            },
            None::<fn(_)>,
            (),
        );

        // send the supported formats
        for f in &*formats {
            dmabuf.format(f.format);
            if version >= 3 {
                dmabuf.modifier(f.format, f.modifier_hi, f.modifier_lo);
            }
        }
    })
}

struct ParamsHandler<H: DmabufHandler> {
    pending_planes: Vec<Plane>,
    max_planes: u32,
    used: bool,
    formats: Rc<Vec<Format>>,
    handler: Rc<RefCell<H>>,
}

impl<H: DmabufHandler> ParamRequestHandler for ParamsHandler<H> {
    fn add(
        &mut self,
        params: BufferParams,
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
            modifier_hi,
            modifier_lo,
        });
    }

    fn create(&mut self, params: BufferParams, width: i32, height: i32, format: u32, flags: u32) {
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
            return;
        }
        let info = BufferInfo {
            planes: ::std::mem::replace(&mut self.pending_planes, Vec::new()),
            width,
            height,
            format,
            flags,
        };
        let mut handler = self.handler.borrow_mut();
        if let Ok(data) = handler.validate_dmabuf(info) {
            if let Some(buffer) = params
                .as_ref()
                .client()
                .and_then(|c| c.create_resource::<wl_buffer::WlBuffer>(1))
            {
                let buffer = handler.create_buffer(data, buffer);
                params.created(&buffer);
            }
        } else {
            params.failed();
        }
    }

    fn create_immed(
        &mut self,
        params: BufferParams,
        buffer_id: NewResource<wl_buffer::WlBuffer>,
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
            return;
        }
        let info = BufferInfo {
            planes: ::std::mem::replace(&mut self.pending_planes, Vec::new()),
            width,
            height,
            format,
            flags,
        };
        let mut handler = self.handler.borrow_mut();
        if let Ok(data) = handler.validate_dmabuf(info) {
            handler.create_buffer(data, buffer_id);
        } else {
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
    let format = match formats.iter().find(|f| f.format == format) {
        Some(f) => f,
        None => {
            params.as_ref().post_error(
                ParamError::InvalidFormat as u32,
                format!("Format {:x} is not supported.", format),
            );
            return false;;
        }
    };
    // The number of planes set must match what the format expects
    let max_plane_set = pending_planes.iter().map(|d| d.plane_idx + 1).max().unwrap_or(0);
    if max_plane_set != format.plane_count || pending_planes.len() < format.plane_count as usize {
        params.as_ref().post_error(
            ParamError::Incomplete as u32,
            format!(
                "Format {:x} requires {} planes but got {}.",
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
    return true;
}
