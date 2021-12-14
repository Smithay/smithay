use crate::{
    backend::{
        allocator::{Allocator, Buffer, Format, Fourcc, Modifier, dmabuf::{AsDmabuf, Dmabuf, DmabufFlags}},
        egl::{display::EGLDisplayHandle, EGLContext, EGLError, MakeCurrentError, ffi::egl as egl},
        renderer::gles2::ffi as gl,
    },
    utils::{Size, Buffer as BufferCoords},
};

use std::{
    convert::TryFrom,
    ptr,
    sync::{Arc, Weak},
};

pub struct EglAllocator {
    egl: EGLContext,
    gl: gl::Gles2,
}

impl std::fmt::Debug for EglAllocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EglAllocator")
        .field("egl", &self.egl)
        .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EglAllocatorInitError {
    #[error("One or more of the following required extensions is missing: {0:?}")]
    ExtensionsMissing(&'static [&'static str]),
    #[error(transparent)]
    MakeCurrentError(#[from] MakeCurrentError),
    #[error("Error initializing OpenGLES API")]
    GLInitError,
}

#[derive(Debug, thiserror::Error)]
pub enum EglAllocatorBufferError {
    #[error("Unsupported color format: {0:?}")]
    UnsupportedFormatError(Fourcc),
    #[error("Unsupported modifier created by GLES: {0:?}")]
    UnsupportedModifierError(Modifier),
    #[error(transparent)]
    MakeCurrentError(#[from] MakeCurrentError),
    #[error("Error creating EGLImage: {0:?}")]
    ImageCreateError(#[source] Option<EGLError>),
    #[error("Error querying EGLImage: {0:?}")]
    ImageQueryError(#[source] Option<EGLError>),
    #[error(transparent)]
    UnrecognizedFourcc(#[from] drm_fourcc::UnrecognizedFourcc),
}

#[derive(Debug, thiserror::Error)]
pub enum EglBufferExportError {
    #[error("Error querying EGLImage: {0:?}")]
    ImageQueryError(#[source] Option<EGLError>),
    #[error("Error exporting EGLImage: {0:?}")]
    ImageExportError(#[source] Option<EGLError>),
    #[error("Failed to create dmabuf")]
    DmabufError,
    #[error("The context for this buffer does not exist anymore")]
    ContextLost,
}

impl EglAllocator {
    pub unsafe fn new<L>(context: EGLContext, logger: L) -> Result<EglAllocator, EglAllocatorInitError>
    where
        L: Into<Option<::slog::Logger>>
    {
        let _log = crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "allocator_gles2"));
        
        if !context.display.get_extensions().iter().any(|ext| ext == "EGL_KHR_gl_texture_2D_image")
        || !context.display.get_extensions().iter().any(|ext| ext == "EGL_MESA_image_dma_buf_export") {
            return Err(EglAllocatorInitError::ExtensionsMissing(&["EGL_KHR_gl_texture_2D_image", "EGL_MESA_image_dmabuf_export"]));
        }

        context.make_current()?;

        let gl = gl::Gles2::load_with(|s| crate::backend::egl::get_proc_address(s) as *const _);
        let ext_ptr = gl.GetString(gl::EXTENSIONS);
        if ext_ptr.is_null() {
            return Err(EglAllocatorInitError::GLInitError);
        }
        
        Ok(EglAllocator {
            egl: context,
            gl,
        })
    }
}

impl Allocator<EglBuffer> for EglAllocator {
    type Error = EglAllocatorBufferError;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<EglBuffer, EglAllocatorBufferError> {
        let (internal, format, type_) = drm_format_to_gl(fourcc).ok_or(EglAllocatorBufferError::UnsupportedFormatError(fourcc))?;

        unsafe {
            if !self.egl.is_current() {
                self.egl.make_current()?;
            }

            let mut tex = 0;
            self.gl.GenTextures(1, &mut tex as *mut _);
            self.gl.BindTexture(gl::TEXTURE_2D, tex);
            self.gl.TexImage2D(gl::TEXTURE_2D, 0, internal, width as i32, height as i32, 0, format, type_, ptr::null());

            let image = egl::CreateImageKHR(**self.egl.display.display, self.egl.context, egl::GL_TEXTURE_2D, tex as egl::types::EGLClientBuffer, ptr::null());
            
            self.gl.BindTexture(gl::TEXTURE_2D, 0);
            self.gl.DeleteTextures(1, &mut tex as *mut _);

            if image == egl::NO_IMAGE_KHR {
                return Err(EglAllocatorBufferError::ImageCreateError(EGLError::from_last_call().err()));
            }
            
            let mut format: nix::libc::c_int = 0;
            let mut modifier: egl::types::EGLuint64KHR = 0;
                
            if egl::ExportDMABUFImageQueryMESA(**self.egl.display.display, image, &mut format as *mut _, ptr::null_mut(), &mut modifier as *mut _) == egl::FALSE {
                return Err(EglAllocatorBufferError::ImageQueryError(EGLError::from_last_call().err()));
            }

            let modifier = modifier.into();
            let buffer = EglBuffer {
                display: Arc::downgrade(&self.egl.display.get_display_handle()),
                image,
                size: (width as i32, height as i32).into(),
                format: Format {
                    code: Fourcc::try_from(format as u32)?,
                    modifier,
                }
            };

            if modifier != Modifier::Invalid && !modifiers.contains(&modifier) {
                return Err(EglAllocatorBufferError::UnsupportedModifierError(modifier));
            }

            Ok(buffer)
        }
    }
}

#[derive(Debug)]
pub struct EglBuffer {
    display: Weak<EGLDisplayHandle>,
    image: egl::types::EGLImageKHR,
    size: Size<i32, BufferCoords>,
    format: Format,
}

impl Buffer for EglBuffer {
    fn size(&self) -> Size<i32, BufferCoords> {
        self.size
    }
    fn format(&self) -> Format {
        self.format
    }
}

impl AsDmabuf for EglBuffer {
    type Error = EglBufferExportError;

    fn export(&self) -> Result<Dmabuf, EglBufferExportError> {
        if let Some(display) = self.display.upgrade() {
            let mut dma = Dmabuf::builder(self.size, self.format.code, DmabufFlags::empty());

            unsafe {
                let mut num_planes: nix::libc::c_int = 0;
            
                if egl::ExportDMABUFImageQueryMESA(**display, self.image, ptr::null_mut(), &mut num_planes as *mut _, ptr::null_mut()) == egl::FALSE {
                    return Err(EglBufferExportError::ImageQueryError(EGLError::from_last_call().err()));
                }

                let mut fds: Vec<nix::libc::c_int> = Vec::with_capacity(num_planes as usize);
                let mut strides: Vec<egl::types::EGLint> = Vec::with_capacity(num_planes as usize);
                let mut offsets: Vec<egl::types::EGLint> = Vec::with_capacity(num_planes as usize);
                
                if egl::ExportDMABUFImageMESA(**display, self.image, fds.as_mut_ptr(), strides.as_mut_ptr(), offsets.as_mut_ptr()) == egl::FALSE {
                    return Err(EglBufferExportError::ImageExportError(EGLError::from_last_call().err()));
                }

                fds.set_len(num_planes as usize);
                strides.set_len(num_planes as usize);
                offsets.set_len(num_planes as usize);

                for i in 0..num_planes {
                    dma.add_plane(fds[i as usize], i as u32, offsets[i as usize] as u32, strides[i as usize] as u32, self.format.modifier);
                }
            }

            dma.build().ok_or(EglBufferExportError::DmabufError)
        } else {
            Err(EglBufferExportError::ContextLost)
        }
    }
}

impl Drop for EglBuffer {
    fn drop(&mut self) {
        if let Some(display) = self.display.upgrade() {
            unsafe {
                egl::DestroyImageKHR(**display, self.image);
            }
        }
    }
}

fn drm_format_to_gl(fourcc: Fourcc) -> Option<(gl::types::GLint, gl::types::GLenum, gl::types::GLenum)> {
    Some(match fourcc {
        Fourcc::Argb8888 => (gl::BGRA_EXT as i32, gl::BGRA_EXT, gl::UNSIGNED_BYTE),
        Fourcc::Xrgb8888 => (gl::BGRA_EXT as i32, gl::BGRA_EXT, gl::UNSIGNED_BYTE),
        Fourcc::Abgr8888 => (gl::RGBA as i32, gl::RGBA, gl::UNSIGNED_BYTE),
        Fourcc::Xbgr8888 => (gl::RGBA as i32, gl::RGB, gl::UNSIGNED_BYTE),
        _ => return None,
    })

    /* To be done later
    Some(match fourcc {
        Fourcc::Abgr1555 => (ffi::RGB5_A1, ffi::RGBA, ffi::UNSIGNED_SHORT_5_5_5_1),
        Fourcc::Abgr16161616f => (ffi::RGBA16F, ffi::RGBA, ffi::HALF_FLOAT),
        Fourcc::Abgr2101010 => (ffi::RGB10_A2, ffi::RGBA, ffi::UNSIGNED_INT_10_10_10_2),
        Fourcc::Abgr4444 => (ffi::RGBA4, ffi::RGBA, ffi::UNSIGNED_SHORT_4_4_4_4)
        Fourcc::Abgr8888 => (if modifiers.contains(Modifier::Invalid) { ffi::COMPRESSED_RGBA } else { ffi::RGBA8 }, ffi::RGBA, ffi::UNSIGNED_INT_8_8_8_8),
        Fourcc::Argb1555 => (ffi::BGR5_A1, ffi::BGRA, ffi::UNSIGNED_SHORT_1_5_5_5_REV),
        Fourcc::Argb16161616f => (ffi::BGRA16F, ffi::BGRA, ffi::HALF_FLOAT),
        Fourcc::Argb2101010 => (ffi::BGR10_A2, ffi::BGRA, ffi::UNSIGNED_INT_2_10_10_10_REV),
        Fourcc::Argb4444 => (ffi::BGRA4, ffi::BGRA, ffi::UNSIGNED_SHORT_4_4_4_4_REV),
        Fourcc::Argb8888 => ()
        Fourcc::Bgr233 => ()
        Fourcc::Bgr565 => ()
        Fourcc::Bgr565_a8 => ()
        Fourcc::Bgr888 => ()
        Fourcc::Bgr888_a8 => ()
        Fourcc::Bgra1010102 => ()
        Fourcc::Bgra4444 => ()
        Fourcc::Bgra5551 => ()
        Fourcc::Bgra8888 => ()
        Fourcc::Bgrx1010102 => ()
        Fourcc::Bgrx4444 => ()
        Fourcc::Bgrx5551 => ()
        Fourcc::Bgrx8888 => ()
        Fourcc::Bgrx8888_a8 => ()
        Fourcc::Gr1616 => ()
        Fourcc::Gr88 => ()
        Fourcc::R16 => ()
        Fourcc::R8 => ()
        Fourcc::Rg1616 => ()
        Fourcc::Rg88 => ()
        Fourcc::Rgb332 => ()
        Fourcc::Rgb565 => ()
        Fourcc::Rgb565_a8 => ()
        Fourcc::Rgb888 => ()
        Fourcc::Rgb888_a8 => ()
        Fourcc::Rgba1010102 => ()
        Fourcc::Rgba4444 => ()
        Fourcc::Rgba5551 => ()
        Fourcc::Rgba8888 => ()
        Fourcc::Rgbx1010102 => ()
        Fourcc::Rgbx4444 => ()
        Fourcc::Rgbx5551 => ()
        Fourcc::Rgbx8888 => ()
        Fourcc::Rgbx8888_a8 => ()
        Fourcc::Xbgr1555 => ()
        Fourcc::Xbgr16161616f => ()
        Fourcc::Xbgr2101010 => ()
        Fourcc::Xbgr4444 => ()
        Fourcc::Xbgr8888 => ()
        Fourcc::Xbgr8888_a8 => ()
        Fourcc::Xrgb1555 => ()
        Fourcc::Xrgb16161616f => ()
        Fourcc::Xrgb2101010 => ()
        Fourcc::Xrgb4444 => ()
        Fourcc::Xrgb8888 => ()
        Fourcc::Xrgb8888_a8 => (),
        _ => return None,
    })
    */
}