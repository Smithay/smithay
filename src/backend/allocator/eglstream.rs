use crate::{
    backend::{
        allocator::{Allocator, Buffer, Format, Fourcc, Modifier, dmabuf::{AsDmabuf, Dmabuf, DmabufFlags}},
        egl::{display::EGLDisplayHandle, context::{GlAttributes, PixelFormatRequirements}, EGLDevice, EGLDisplay, EGLContext, Error, EGLError, MakeCurrentError, ffi::egl as egl},
        renderer::gles2::ffi as gl,
    },
    utils::{Size, Buffer as BufferCoords},
};

use std::{
    convert::TryFrom,
    ptr,
    sync::{Arc, Weak},
};

#[derive(Debug)]
pub struct EglStreamAllocator {
    egl: EGLDisplay,
    logger: ::slog::Logger,
}

#[derive(Debug, thiserror::Error)]
pub enum EglStreamAllocatorInitError {
    #[error("One or more of the following required extensions is missing: {0:?}")]
    ExtensionsMissing(&'static [&'static str]),
    #[error("Failed to create an EGLDisplayy: {0:?}")]
    DisplayCreateError(#[from] Error)
}

#[derive(Debug, thiserror::Error)]
pub enum EglStreamAllocatorBufferError {
    #[error("Unsupported color format: {0:?}")]
    UnsupportedFormatError(Fourcc),
    #[error("Unsupported modifier created by GLES: {0:?}")]
    UnsupportedModifierError(Modifier),
    #[error("Error creating EGLStream: {0:?}")]
    StreamCreationError(#[source] Option<EGLError>),
    #[error("Error connecting EGLStream to consumer: {0:?}")]
    StreamConnectError(#[source] Option<EGLError>),
    #[error(transparent)]
    ContextCreationError(#[from] Error),
    #[error("Error creating EGLSurface: {0:?}")]
    SurfaceCreationError(#[source] Option<EGLError>),
    #[error(transparent)]
    MakeCurrentError(#[from] MakeCurrentError),
    #[error("Error creating EGLImage: {0:?}")]
    ImageCreationError(#[source] Option<EGLError>),
    #[error("Error acquiring EGLImage: {0:?}")]
    ImageAcquireError(#[source] Option<EGLError>),
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

impl EglStreamAllocator {
    pub fn new<L>(device: &EGLDevice, logger: L) -> Result<EglStreamAllocator, EglStreamAllocatorInitError>
    where
        L: Into<Option<::slog::Logger>>
    {
        let log = crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "allocator_eglstream"));
        let display = EGLDisplay::new(device, log.clone())?;

        if !display.get_extensions().iter().any(|ext| ext == "EGL_KHR_stream_producer_eglsurface")
        || !display.get_extensions().iter().any(|ext| ext == "EGL_NV_stream_consumer_eglimage")
        || !display.get_extensions().iter().any(|ext| ext == "EGL_MESA_image_dma_buf_export") {
            return Err(EglStreamAllocatorInitError::ExtensionsMissing(&["EGL_KHR_stream_producer_eglsurface", "EGL_NV_stream_consumer_eglimage", "EGL_MESA_image_dmabuf_export"]));
        } 
        
        Ok(EglStreamAllocator {
            egl: display,
            logger: log,
        })
    }
}

impl Allocator<EglBuffer> for EglStreamAllocator {
    type Error = EglStreamAllocatorBufferError;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<EglBuffer, EglStreamAllocatorBufferError> {
        let reqs = drm_format_to_reqs(fourcc).ok_or(EglStreamAllocatorBufferError::UnsupportedFormatError(fourcc))?;

        // create an eglstream that will obey to our modifiers
        let stream = unsafe { egl::CreateStreamKHR(**self.egl.display, ptr::null()) };
        if stream == egl::NO_STREAM_KHR {
            slog::error!(self.logger, "Failed to create egl stream");
            return Err(EglStreamAllocatorBufferError::StreamCreationError(EGLError::from_last_call().err()));
        }

        let mut mods = modifiers.iter().map(|x| (*x).into()).collect::<Vec<u64>>();
        if unsafe { egl::StreamImageConsumerConnectNV(**self.egl.display, stream, modifiers.len() as i32, mods.as_mut_ptr(), ptr::null()) } == 0 {
            slog::error!(self.logger, "Unable to link EGLImage consumer to egl stream");
            return Err(EglStreamAllocatorBufferError::StreamConnectError(EGLError::from_last_call().err()));
        }

        // create a context
        let context = EGLContext::new_with_config(
            &self.egl,
            GlAttributes {
                version: (2, 0),
                profile: None,
                debug: false,
                vsync: false,
            },
            reqs,
            self.logger.clone(),
        )?;

        // create a surface
        let surface_attributes = [
            egl::WIDTH as i32,
            width as i32,
            egl::HEIGHT as i32,
            height as i32,
            egl::NONE as i32,
        ];
        let surface = unsafe { egl::CreateStreamProducerSurfaceKHR(**self.egl.display, context.config_id(), stream, surface_attributes.as_ptr()) };
        if surface == egl::NO_SURFACE {
            let error = unsafe { egl::GetError() };
            slog::error!(self.logger, "Failed to create surface: 0x{:X}",  error);
            return Err(EglStreamAllocatorBufferError::SurfaceCreationError(Some(EGLError::from(error as u32))));
        }

        // generate an empty frame
        unsafe {
            egl::MakeCurrent(**self.egl.display, surface, surface, context.context);
            let gl = gl::Gles2::load_with(|s| crate::backend::egl::get_proc_address(s) as *const _);
            gl.ClearColor(0.0, 0.0, 0.0, 1.0);
            egl::SwapBuffers(**self.egl.display, surface);
        }
        
        // acquire that frame
        let image = unsafe { egl::CreateImage(**self.egl.display, context.context, egl::STREAM_CONSUMER_IMAGE_NV, stream as egl::types::EGLClientBuffer, ptr::null()) };
        if image == egl::NO_IMAGE_KHR {
            let error = EGLError::from_last_call().err();
            slog::error!(self.logger, "Failed to create image: {:?}", error);
            return Err(EglStreamAllocatorBufferError::ImageCreationError(error));
        }
        
        let mut stream_image = egl::NO_IMAGE_KHR;
        if unsafe { egl::StreamAcquireImageNV(**self.egl.display, stream, &mut stream_image as *mut _, egl::NO_SYNC) } == egl::FALSE {
            let error = EGLError::from_last_call().err();
            slog::error!(self.logger, "Failed to create image: {:?}", error);
            return Err(EglStreamAllocatorBufferError::ImageAcquireError(error));
        }

        debug_assert!(stream_image == image);

        // check the format & modifiers of the frame

        let mut format: nix::libc::c_int = 0;
        let mut modifier: egl::types::EGLuint64KHR = 0;
            
        if unsafe { egl::ExportDMABUFImageQueryMESA(**self.egl.display, image, &mut format as *mut _, ptr::null_mut(), &mut modifier as *mut _) } == egl::FALSE {
            return Err(EglStreamAllocatorBufferError::ImageQueryError(EGLError::from_last_call().err()));
        }

        let modifier = modifier.into();
        let buffer = EglBuffer {
            display: Arc::downgrade(&self.egl.get_display_handle()),
            image,
            size: (width as i32, height as i32).into(),
            format: Format {
                code: Fourcc::try_from(format as u32)?,
                modifier,
            }
        };

        if modifier != Modifier::Invalid && !modifiers.contains(&modifier) {
            return Err(EglStreamAllocatorBufferError::UnsupportedModifierError(modifier));
        }

        // cleanup
        unsafe {
            egl::DestroySurface(**self.egl.display, surface);
            std::mem::drop(context);
            egl::DestroyStreamKHR(**self.egl.display, stream);
        }

        Ok(buffer)
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

fn drm_format_to_reqs(fourcc: Fourcc) -> Option<PixelFormatRequirements> {
    Some(match fourcc {
        Fourcc::Abgr8888 | Fourcc::Argb8888 => PixelFormatRequirements {
            hardware_accelerated: Some(true),
            color_bits: Some(24),
            alpha_bits: Some(8),
            float_color_buffer: false,
            depth_bits: None,
            stencil_bits: None,
            multisampling: None,
        },
        Fourcc::Xbgr8888 | Fourcc::Xrgb8888 => PixelFormatRequirements {
            hardware_accelerated: Some(true),
            color_bits: Some(24),
            alpha_bits: Some(0),
            float_color_buffer: false,
            depth_bits: None,
            stencil_bits: None,
            multisampling: None,
        },
        _ => return None,
    })
}