use backend::graphics::egl::{EGLContext, EglExtensionNotSupportedError, ffi, native};
use backend::graphics::egl::error::*;
use backend::graphics::egl::ffi::egl::types::EGLImage;
use nix::libc::{c_uint};
use std::rc::{Rc, Weak};
use std::fmt;
use wayland_server::{Display, Resource, StateToken, StateProxy};
use wayland_server::protocol::wl_buffer::WlBuffer;

/// Error that can occur when accessing an EGL buffer
pub enum BufferAccessError {
    /// The corresponding Context is not alive anymore
    ContextLost,
    /// This buffer is not managed by the EGL buffer
    NotManaged(WlBuffer),
    /// Failed to create EGLImages from the buffer
    EGLImageCreationFailed,
    /// The required EGL extension is not supported by the underlying EGL implementation
    EglExtensionNotSupported(EglExtensionNotSupportedError),
}

impl fmt::Debug for BufferAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> ::std::result::Result<(), fmt::Error> {
        match *self {
            BufferAccessError::ContextLost => write!(formatter, "BufferAccessError::ContextLost"),
            BufferAccessError::NotManaged(_) => write!(formatter, "BufferAccessError::NotManaged"),
            BufferAccessError::EGLImageCreationFailed => write!(formatter, "BufferAccessError::EGLImageCreationFailed"),
            BufferAccessError::EglExtensionNotSupported(ref err) => write!(formatter, "{:?}", err),
        }
    }
}

impl fmt::Display for BufferAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> ::std::result::Result<(), fmt::Error> {
        use ::std::error::Error;
        match *self {
            BufferAccessError::ContextLost | BufferAccessError::NotManaged(_) | BufferAccessError::EGLImageCreationFailed => write!(formatter, "{}", self.description()),
            BufferAccessError::EglExtensionNotSupported(ref err) => err.fmt(formatter),
        }
    }
}

impl ::std::error::Error for BufferAccessError {
    fn description(&self) -> &str {
        match *self {
            BufferAccessError::ContextLost => "The corresponding context was lost",
            BufferAccessError::NotManaged(_) => "This buffer is not mananged by EGL",
            BufferAccessError::EGLImageCreationFailed => "Failed to create EGLImages from the buffer",
            BufferAccessError::EglExtensionNotSupported(ref err) => err.description(),
        }
    }

    fn cause(&self) -> Option<&::std::error::Error> {
        match *self {
            BufferAccessError::EglExtensionNotSupported(ref err) => Some(err),
            _ => None,
        }
    }
}

impl From<EglExtensionNotSupportedError> for BufferAccessError {
    fn from(error: EglExtensionNotSupportedError) -> Self {
        BufferAccessError::EglExtensionNotSupported(error)
    }
}

/// Error that might happen when binding an EGLImage to a gl texture
#[derive(Debug, Clone, PartialEq)]
pub enum TextureCreationError {
    /// The given plane index is out of bounds
    PlaneIndexOutOfBounds,
    /// The OpenGL context has been lost and needs to be recreated.
    ///
    /// All the objects associated to it (textures, buffers, programs, etc.)
    /// need to be recreated from scratch.
    ///
    /// Operations will have no effect. Functions that read textures, buffers, etc.
    /// from OpenGL will return uninitialized data instead.
    ///
    /// A context loss usually happens on mobile devices when the user puts the
    /// application on sleep and wakes it up later. However any OpenGL implementation
    /// can theoretically lose the context at any time.
    ContextLost,
    /// Failed to bind the EGLImage to the given texture
    ///
    /// The given argument is the gl error code
    TextureBindingFailed(u32),
}

impl fmt::Display for TextureCreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> ::std::result::Result<(), fmt::Error> {
        use ::std::error::Error;
        match *self {
            TextureCreationError::ContextLost => write!(formatter, "{}", self.description()),
            TextureCreationError::PlaneIndexOutOfBounds => write!(formatter, "{}", self.description()),
            TextureCreationError::TextureBindingFailed(code) => write!(formatter, "{}. Gl error code: {:?}", self.description(), code),
        }
    }
}

impl ::std::error::Error for TextureCreationError {
    fn description(&self) -> &str {
        match *self {
            TextureCreationError::ContextLost => "The context has been lost, it needs to be recreated",
            TextureCreationError::PlaneIndexOutOfBounds => "This buffer is not mananged by EGL",
            TextureCreationError::TextureBindingFailed(_) => "Failed to create EGLImages from the buffer",
        }
    }

    fn cause(&self) -> Option<&::std::error::Error> {
        None
    }
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
    pub fn num_planes(&self) -> usize {
        match *self {
            Format::RGB | Format::RGBA | Format::External => 1,
            Format::Y_UV | Format::Y_XUXV => 2,
            Format::Y_U_V => 3,
        }
    }
}

pub struct EGLImages {
    display: Weak<ffi::egl::types::EGLDisplay>,
    pub width: u32,
    pub height: u32,
    pub y_inverted: bool,
    pub format: Format,
    images: Vec<EGLImage>,
    buffer: WlBuffer,
}

impl EGLImages {
    pub fn num_planes(&self) -> usize {
        self.format.num_planes()
    }

    pub unsafe fn bind_to_texture(&self, plane: usize, tex_id: c_uint) -> ::std::result::Result<(), TextureCreationError> {
        if self.display.upgrade().is_some() {
            let mut old_tex_id: i32 = 0;
            ffi::gl::GetIntegerv(ffi::gl::TEXTURE_BINDING_2D, &mut old_tex_id);
            ffi::gl::BindTexture(ffi::gl::TEXTURE_2D, tex_id);
            ffi::gl::EGLImageTargetTexture2DOES(ffi::gl::TEXTURE_2D, *self.images.get(plane).ok_or(TextureCreationError::PlaneIndexOutOfBounds)?);
            let res = match ffi::egl::GetError() as u32 {
                ffi::egl::SUCCESS => Ok(()),
                err => Err(TextureCreationError::TextureBindingFailed(err)),
            };
            ffi::gl::BindTexture(ffi::gl::TEXTURE_2D, old_tex_id as u32);
            res
        } else {
            Err(TextureCreationError::ContextLost)
        }
    }
}

impl Drop for EGLImages {
    fn drop(&mut self) {
        if let Some(display) = self.display.upgrade() {
            for image in self.images.drain(..) {
                unsafe { ffi::egl::DestroyImageKHR(*display, image); }
            }
        }
        self.buffer.release();
    }
}

pub trait EGLWaylandExtensions {
    /// Binds this EGL context to the given Wayland display.
    ///
    /// This will allow clients to utilize EGL to create hardware-accelerated
    /// surfaces. The server will need to be able to handle egl-wl_buffers.
    /// See the `wayland::drm` module.
    ///
    /// ## Errors
    ///
    /// This might return `WlExtensionNotSupported` if binding is not supported
    /// by the EGL implementation.
    ///
    /// This might return `OtherEGLDisplayAlreadyBound` if called for the same
    /// `Display` multiple times, as only one context may be bound at any given time.
    fn bind_wl_display(&self, display: &Display) -> Result<EGLDisplay>;

    /// Unbinds this EGL context from the given Wayland display.
    ///
    /// This will stop clients from using previously available extensions
    /// to utilize hardware-accelerated surface via EGL.
    ///
    /// ## Errors
    ///
    /// This might return `WlExtensionNotSupported` if binding is not supported
    /// by the EGL implementation.
    ///
    /// This might return `OtherEGLDisplayAlreadyBound` if called for the same
    /// `Display` multiple times, as only one context may be bound at any given time.
    fn unbind_wl_display(&self, display: &Display) -> Result<()>;
}

pub struct EGLDisplay(Weak<ffi::egl::types::EGLDisplay>);

impl EGLDisplay {
    pub fn new<B: native::Backend, N: native::NativeDisplay<B>>(context: &EGLContext<B, N>) -> EGLDisplay {
        EGLDisplay(Rc::downgrade(&context.display))
    }

    pub fn egl_buffer_contents(&self, buffer: WlBuffer) -> ::std::result::Result<EGLImages, BufferAccessError> {
        if let Some(display) = self.0.upgrade() {
            let mut format: i32 = 0;
            if unsafe { ffi::egl::QueryWaylandBufferWL(*display, buffer.ptr() as *mut _, ffi::egl::EGL_TEXTURE_FORMAT, &mut format as *mut _) == 0 } {
                return Err(BufferAccessError::NotManaged(buffer));
            }
            let format = match format {
                x if x == ffi::egl::TEXTURE_RGB as i32 => Format::RGB,
                x if x == ffi::egl::TEXTURE_RGBA as i32 => Format::RGBA,
                ffi::egl::TEXTURE_EXTERNAL_WL => Format::External,
                ffi::egl::TEXTURE_Y_UV_WL => Format::Y_UV,
                ffi::egl::TEXTURE_Y_U_V_WL => Format::Y_U_V,
                ffi::egl::TEXTURE_Y_XUXV_WL => Format::Y_XUXV,
                _ => panic!("EGL returned invalid texture type"),
            };

            let mut width: i32 = 0;
            if unsafe { ffi::egl::QueryWaylandBufferWL(*display, buffer.ptr() as *mut _, ffi::egl::WIDTH as i32, &mut width as *mut _) == 0 } {
                return Err(BufferAccessError::NotManaged(buffer));
            }

            let mut height: i32 = 0;
            if unsafe { ffi::egl::QueryWaylandBufferWL(*display, buffer.ptr() as *mut _, ffi::egl::HEIGHT as i32, &mut height as *mut _) == 0 } {
                return Err(BufferAccessError::NotManaged(buffer));
            }

            let mut inverted: i32 = 0;
            if unsafe { ffi::egl::QueryWaylandBufferWL(*display, buffer.ptr() as *mut _, ffi::egl::WAYLAND_Y_INVERTED_WL, &mut inverted as *mut _) != 0 } {
                inverted = 1;
            }

            let mut images = Vec::with_capacity(format.num_planes());
            for i in 0..format.num_planes() {
                let mut out = Vec::with_capacity(3);
                out.push(ffi::egl::WAYLAND_PLANE_WL as i32);
                out.push(i as i32);
        		out.push(ffi::egl::NONE as i32);

                images.push({
        		    let image =
                        unsafe { ffi::egl::CreateImageKHR(
                            *display,
                            ffi::egl::NO_CONTEXT,
                            ffi::egl::WAYLAND_BUFFER_WL,
            				buffer.ptr() as *mut _,
            				out.as_ptr(),
                        ) };
                    if image == ffi::egl::NO_IMAGE_KHR {
                        return Err(BufferAccessError::EGLImageCreationFailed);
                    } else {
                        image
                    }
                });
            }

            Ok(EGLImages {
                display: Rc::downgrade(&display),
                width: width as u32,
                height: height as u32,
                y_inverted: inverted != 0,
                format,
                images,
                buffer,
            })
        } else {
            Err(BufferAccessError::ContextLost)
        }
    }
}

impl<E: EGLWaylandExtensions> EGLWaylandExtensions for Rc<E>
{
    fn bind_wl_display(&self, display: &Display) -> Result<EGLDisplay> {
        (**self).bind_wl_display(display)
    }
    fn unbind_wl_display(&self, display: &Display) -> Result<()> {
        (**self).unbind_wl_display(display)
    }
}

impl<B: native::Backend, N: native::NativeDisplay<B>> EGLWaylandExtensions for EGLContext<B, N> {
    fn bind_wl_display(&self, display: &Display) -> Result<EGLDisplay> {
        if !self.wl_drm_support {
            bail!(ErrorKind::EglExtensionNotSupported(&["EGL_WL_bind_wayland_display"]));
        }
        if !self.egl_to_texture_support {
            bail!(ErrorKind::EglExtensionNotSupported(&["GL_OES_EGL_image"]));
        }
        let res = unsafe { ffi::egl::BindWaylandDisplayWL(*self.display, display.ptr() as *mut _) };
        if res == 0 {
            bail!(ErrorKind::OtherEGLDisplayAlreadyBound);
        }
        Ok(EGLDisplay::new(self))
    }

    fn unbind_wl_display(&self, display: &Display) -> Result<()> {
        if !self.wl_drm_support {
            bail!(ErrorKind::EglExtensionNotSupported(&["EGL_WL_bind_wayland_display"]));
        }
        let res = unsafe { ffi::egl::UnbindWaylandDisplayWL(*self.display, display.ptr() as *mut _) };
        if res == 0 {
            bail!(ErrorKind::NoEGLDisplayBound);
        }
        Ok(())
    }
}
