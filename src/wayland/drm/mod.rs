use ::backend::graphics::egl::ffi;
use ::backend::graphics::egl::EGLContext;
pub use ::backend::graphics::egl::ffi::EGLImage;
use nix::libc::c_int;
use wayland_server::protocol::wl_buffer::WlBuffer;

/// Error that can occur when accessing an EGL buffer
#[derive(Debug)]
pub enum BufferAccessError {
    /// This buffer is not managed by EGL
    NotManaged,
    /// Failed to create EGLImages from the buffer
    FailedToCreateEGLImage,
}

#[repr(u32)]
pub enum Format {
    RGB = ffi::TEXTURE_RGB,
    RGBA = ffi::TEXTURE_RGBA,
    External = ffi::TEXTURE_EXTERNAL_WL,
    Y_UV = ffi::TEXTURE_Y_UV_WL,
    Y_U_V = ffi::TEXTURE_Y_U_V_WL,
    Y_XUXV = ffi::TEXTURE_Y_XUXV_WL,
}

impl Format {
    pub fn num_planes(&self) -> u32 {
        match *self {
            Format::RGB | Format::RGBA | Format::External => 1,
            Format::Y_UV | Format::Y_XUXV => 2,
            Format::Y_U_V => 3,
        }
    }
}

pub struct Attributes {
    width: u32,
    height: u32,
    y_inverted: bool,
    format: Format,
}

pub fn with_buffer_contents<F>(buffer: &WlBuffer, context: &EGLContext, f: F) -> Result<(), BufferAccessError>
where
    F: FnOnce(Attributes, Vec<EGLImage>)
{
    let mut format: u32 = 0;
    if context.egl.QueryWaylandBufferWL(context.display, buffer.ptr(), ffi::egl::TEXTURE_FORMAT, &mut format as *mut _) == 0 {
        return Err(BufferAccessError::NotManaged);
    }

    let mut width: u32 = 0;
    if context.egl.QueryWaylandBufferWL(context.display, buffer.ptr(), ffi::egl::WIDTH, &mut width as *mut _) == 0 {
        return Err(BufferAccessError::NotManaged);
    }

    let mut height: u32 = 0;
    if context.egl.QueryWaylandBufferWL(context.display, buffer.ptr(), ffi::egl::HEIGHT, &mut height as *mut _) == 0 {
        return Err(BufferAccessError::NotManaged);
    }

    let mut inverted: u32 = 0;
    if context.egl.QueryWaylandBufferWL(context.display, buffer.ptr(), ffi::egl::WAYLAND_Y_INVERTED_WL, &mut inverted as *mut _) == 0 {
        inverted = 1;
    }

    let attributes = Attributes {
        width,
        height,
        y_inverted = inverted != 0,
        format: format as Format,
    };

    let mut images = Vec::with_capacity(attributes.format.num_planes());
    for _ in 0..attributes.format.num_planes() {
        let mut out = Vec::with_capacity(3);
        out.push(ffi::egl::WAYLAND_PLANE_WL as i32);
        out.push(i as i32);
		out.push(ffi::egl::NONE as i32);

        images.push({
		    let image =
                ffi::egl::CreateImageKHR(
                    context.display,
                    ffi::egl::NO_CONTEXT,
    				ffi::egl::WAYLAND_BUFFER_WL,
    				buffer.ptr(),
    				out.as_ptr(),
                );
            if image == ffi::egl::NO_IMAGE_KHR {
                return Err(BufferAccessError::FailedToCreateEGLImage);
            } else {
                image
            }
        });
    }

    f(attributes, images)
}
