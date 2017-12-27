use backend::graphics::egl::{EGLContext, EGLImage, ffi, native};
use backend::graphics::egl::error::*;
use nix::libc::{c_uint};
use std::rc::{Rc, Weak};
use wayland_server::{Display, Resource};
use wayland_server::protocol::wl_buffer::WlBuffer;

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

    pub unsafe fn bind_to_tex(&self, plane: usize, tex_id: c_uint) -> Result<()> {
        if self.display.upgrade().is_some() {
            ffi::gl::EGLImageTargetTexture2DOES(tex_id, *self.images.get(plane).chain_err(|| ErrorKind::PlaneIndexOutOfBounds)?);
            match ffi::egl::GetError() as u32 {
                ffi::gl::NO_ERROR => Ok(()),
                err => bail!(ErrorKind::Unknown(err)),
            }
        } else {
            bail!(ErrorKind::ContextLost)
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
    }
}

impl<B: native::Backend, N: native::NativeDisplay<B>> EGLContext<B, N> {
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
    pub fn bind_wl_display(&self, display: &Display) -> Result<()> {
        if !self.wl_drm_support {
            bail!(ErrorKind::EglExtensionNotSupported(&["EGL_WL_bind_wayland_display"]));
        }
        let res = unsafe { ffi::egl::BindWaylandDisplayWL(*self.display, display.ptr() as *mut _) };
        if res == 0 {
            bail!(ErrorKind::OtherEGLDisplayAlreadyBound);
        }
        Ok(())
    }

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
    pub fn unbind_wl_display(&self, display: &Display) -> Result<()> {
        if !self.wl_drm_support {
            bail!(ErrorKind::EglExtensionNotSupported(&["EGL_WL_bind_wayland_display"]));
        }
        let res = unsafe { ffi::egl::UnbindWaylandDisplayWL(*self.display, display.ptr() as *mut _) };
        if res == 0 {
            bail!(ErrorKind::NoEGLDisplayBound);
        }
        Ok(())
    }

    pub fn egl_buffer_contents<T: native::NativeSurface>(&self, buffer: WlBuffer) -> Result<EGLImages> {
        if !self.egl_to_texture_support {
            bail!(ErrorKind::EglExtensionNotSupported(&["GL_OES_EGL_image"]));
        }

        let mut format: i32 = 0;
        if unsafe { ffi::egl::QueryWaylandBufferWL(*self.display, buffer.ptr() as *mut _, ffi::egl::EGL_TEXTURE_FORMAT, &mut format as *mut _) == 0 } {
            bail!(ErrorKind::BufferNotManaged);
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
        if unsafe { ffi::egl::QueryWaylandBufferWL(*self.display, buffer.ptr() as *mut _, ffi::egl::WIDTH as i32, &mut width as *mut _) == 0 } {
            bail!(ErrorKind::BufferNotManaged);
        }

        let mut height: i32 = 0;
        if unsafe { ffi::egl::QueryWaylandBufferWL(*self.display, buffer.ptr() as *mut _, ffi::egl::HEIGHT as i32, &mut height as *mut _) == 0 } {
            bail!(ErrorKind::BufferNotManaged);
        }

        let mut inverted: i32 = 0;
        if unsafe { ffi::egl::QueryWaylandBufferWL(*self.display, buffer.ptr() as *mut _, ffi::egl::WAYLAND_Y_INVERTED_WL, &mut inverted as *mut _) == 0 } {
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
                        *self.display,
                        ffi::egl::NO_CONTEXT,
        				ffi::egl::WAYLAND_BUFFER_WL,
        				buffer.ptr() as *mut _,
        				out.as_ptr(),
                    ) };
                if image == ffi::egl::NO_IMAGE_KHR {
                    bail!(ErrorKind::EGLImageCreationFailed);
                } else {
                    image
                }
            });
        }

        Ok(EGLImages {
            display: Rc::downgrade(&self.display),
            width: width as u32,
            height: height as u32,
            y_inverted: inverted != 0,
            format,
            images,
            buffer,
        })
    }
}
