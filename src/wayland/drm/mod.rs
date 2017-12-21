use ::backend::graphics::egl::ffi;
use ::backend::graphics::egl::{EGLContext, NativeSurface};
use ::backend::graphics::egl::EGLImage;
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::Resource;

/// Error that can occur when accessing an EGL buffer
#[derive(Debug)]
pub enum BufferAccessError {
    /// This buffer is not managed by EGL
    NotManaged,
    /// Failed to create EGLImages from the buffer
    FailedToCreateEGLImage,
}

#[repr(i32)]
pub enum Format {
    RGB = ffi::egl::TEXTURE_RGB as i32,
    RGBA = ffi::egl::TEXTURE_RGBA as i32,
    External = ffi::egl::TEXTURE_EXTERNAL_WL,
    Y_UV = ffi::egl::TEXTURE_Y_UV_WL,
    Y_U_V = ffi::egl::TEXTURE_Y_U_V_WL,
    Y_XUXV = ffi::egl::TEXTURE_Y_XUXV_WL,
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

pub struct EGLImages {
    pub width: u32,
    pub height: u32,
    pub y_inverted: bool,
    pub format: Format,
    images: Vec<EGLImage>,
    buffer: WlBuffer,
}

pub fn buffer_contents<T: NativeSurface>(buffer: WlBuffer, context: &EGLContext<T>) -> Result<(Vec<EGLImages>, attributes: Attributes), BufferAccessError>
where
{
    let mut format: i32 = 0;
    if unsafe { ffi::egl::QueryWaylandBufferWL(context.display, buffer.ptr() as *mut _, ffi::egl::EGL_TEXTURE_FORMAT, &mut format as *mut _) == 0 } {
        return Err(BufferAccessError::NotManaged);
    }

    let mut width: i32 = 0;
    if unsafe { ffi::egl::QueryWaylandBufferWL(context.display, buffer.ptr() as *mut _, ffi::egl::WIDTH as i32, &mut width as *mut _) == 0 } {
        return Err(BufferAccessError::NotManaged);
    }

    let mut height: i32 = 0;
    if unsafe { ffi::egl::QueryWaylandBufferWL(context.display, buffer.ptr() as *mut _, ffi::egl::HEIGHT as i32, &mut height as *mut _) == 0 } {
        return Err(BufferAccessError::NotManaged);
    }

    let mut inverted: i32 = 0;
    if unsafe { ffi::egl::QueryWaylandBufferWL(context.display, buffer.ptr() as *mut _, ffi::egl::WAYLAND_Y_INVERTED_WL, &mut inverted as *mut _) == 0 } {
        inverted = 1;
    }

    let mut images = Vec::with_capacity(attributes.format.num_planes() as usize);
    for i in 0..attributes.format.num_planes() {
        let mut out = Vec::with_capacity(3);
        out.push(ffi::egl::WAYLAND_PLANE_WL as i32);
        out.push(i as i32);
		out.push(ffi::egl::NONE as i32);

        images.push({
		    let image =
                unsafe { ffi::egl::CreateImageKHR(
                    context.display,
                    ffi::egl::NO_CONTEXT,
    				ffi::egl::WAYLAND_BUFFER_WL,
    				buffer.ptr() as *mut _,
    				out.as_ptr(),
                ) };
            if image == ffi::egl::NO_IMAGE_KHR {
                return Err(BufferAccessError::FailedToCreateEGLImage);
            } else {
                image
            }
        });
    }

    let result = EGLImages {
        width: width as u32,
        height: height as u32,
        y_inverted: inverted != 0,
        format: match format {
            x if x == ffi::egl::TEXTURE_RGB as i32 => Format::RGB,
            x if x == ffi::egl::TEXTURE_RGBA as i32 => Format::RGBA,
            ffi::egl::TEXTURE_EXTERNAL_WL => Format::External,
            ffi::egl::TEXTURE_Y_UV_WL => Format::Y_UV,
            ffi::egl::TEXTURE_Y_U_V_WL => Format::Y_U_V,
            ffi::egl::TEXTURE_Y_XUXV_WL => Format::Y_XUXV,
            _ => panic!("EGL returned invalid texture type"),
        },
        images,
        buffer,
    };

    Ok(result)
}
