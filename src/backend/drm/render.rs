use std::os::unix::io::AsRawFd;
use std::collections::HashSet;
use std::convert::TryInto;
use std::sync::Arc;

use cgmath::Matrix3;
use drm::buffer::PlanarBuffer;
use drm::control::{Device, Mode, crtc, connector, framebuffer, plane};
use gbm::{Device as GbmDevice, BufferObject, BufferObjectFlags};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_shm, wl_buffer};

use crate::backend::SwapBuffersError;
use crate::backend::allocator::{Allocator, Format, Fourcc, Modifier, Swapchain, SwapchainError, Slot, Buffer, dmabuf::Dmabuf};
use crate::backend::renderer::{Renderer, Bind, Transform, Texture};
use crate::backend::egl::EGLBuffer;
use super::{DrmSurface, DrmError, device::DevPath, surface::DrmSurfaceInternal};

pub struct DrmRenderSurface<
    D: AsRawFd + 'static,
    A: Allocator<B>,
    R: Bind<Dmabuf>,
    B: Buffer + TryInto<Dmabuf>,
> {
    format: Fourcc,
    buffers: Buffers<D>,
    current_buffer: Option<Slot<Dmabuf, BufferObject<FbHandle<D>>>>,
    swapchain: Swapchain<A, B, BufferObject<FbHandle<D>>, Dmabuf>,
    renderer: R,
    drm: Arc<DrmSurface<D>>,
}

impl<D, A, B, R, E1, E2, E3> DrmRenderSurface<D, A, R, B>
where
    D: AsRawFd + 'static,
    A: Allocator<B, Error=E1>,
    B: Buffer + TryInto<Dmabuf, Error=E2>,
    R: Bind<Dmabuf> + Renderer<Error=E3>,
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + 'static,
{
    pub fn new<L: Into<Option<::slog::Logger>>>(drm: DrmSurface<D>, allocator: A, renderer: R, log: L) -> Result<DrmRenderSurface<D, A, R, B>, Error<E1, E2, E3>>
    {
        // we cannot simply pick the first supported format of the intersection of *all* formats, because:
        // - we do not want something like Abgr4444, which looses color information
        // - some formats might perform terribly
        // - we might need some work-arounds, if one supports modifiers, but the other does not
        //
        // So lets just pick `ARGB8888` for now, it is widely supported.
        // Once we have proper color management and possibly HDR support,
        // we need to have a more sophisticated picker.
        // (Or maybe just pick ARGB2101010, if available, we will see.)
        let code = Fourcc::Argb8888;
        let logger = crate::slog_or_fallback(log).new(o!("backend" => "drm_render"));

        // select a format
        let plane_formats = drm.supported_formats().iter().filter(|fmt| fmt.code == code).cloned().collect::<HashSet<_>>();
        let mut renderer_formats = Bind::<Dmabuf>::supported_formats(&renderer).expect("Dmabuf renderer without formats")
            .iter().filter(|fmt| fmt.code == code).cloned().collect::<HashSet<_>>();

        trace!(logger, "Remaining plane formats: {:?}", plane_formats);
        trace!(logger, "Remaining renderer formats: {:?}", renderer_formats);
        debug!(logger, "Remaining intersected formats: {:?}", plane_formats.intersection(&renderer_formats).collect::<HashSet<_>>());
        
        if plane_formats.is_empty() {
            return Err(Error::NoSupportedPlaneFormat);
        } else if renderer_formats.is_empty() {
            return Err(Error::NoSupportedRendererFormat);
        }

        let formats = {
            // Special case: if a format supports explicit LINEAR (but no implicit Modifiers)
            // and the other doesn't support any modifier, force LINEAR. This will force the allocator to
            // create a buffer with a LINEAR layout instead of an implicit modifier.
            if 
                (plane_formats.len() == 1 &&
                    plane_formats.iter().next().unwrap().modifier == Modifier::Invalid
                    && renderer_formats.iter().all(|x| x.modifier != Modifier::Invalid)
                    && renderer_formats.iter().any(|x| x.modifier == Modifier::Linear)
                ) || (renderer_formats.len() == 1 &&
                    renderer_formats.iter().next().unwrap().modifier == Modifier::Invalid
                    && plane_formats.iter().all(|x| x.modifier != Modifier::Invalid)
                    && plane_formats.iter().any(|x| x.modifier == Modifier::Linear)
            ) {
                vec![Format {
                    code,
                    modifier: Modifier::Linear,
                }]
            } else {
                plane_formats.intersection(&renderer_formats).cloned().collect::<Vec<_>>()
            }
        };
        debug!(logger, "Testing Formats: {:?}", formats);

        // Test explicit formats first
        let drm = Arc::new(drm);
        let iter = formats.iter().filter(|x| x.modifier != Modifier::Invalid && x.modifier != Modifier::Linear)
            .chain(formats.iter().find(|x| x.modifier == Modifier::Linear))
            .chain(formats.iter().find(|x| x.modifier == Modifier::Invalid)).cloned();

        DrmRenderSurface::new_internal(drm, allocator, renderer, iter, logger)
    }

    fn new_internal(drm: Arc<DrmSurface<D>>, allocator: A, mut renderer: R, mut formats: impl Iterator<Item=Format>, logger: ::slog::Logger) -> Result<DrmRenderSurface<D, A, R, B>, Error<E1, E2, E3>>
    {
        let format = formats.next().ok_or(Error::NoSupportedPlaneFormat)?;
        let mode = drm.pending_mode();

        let gbm = unsafe { GbmDevice::new_from_fd(drm.as_raw_fd())? };
        let mut swapchain = Swapchain::new(allocator, mode.size().0 as u32, mode.size().1 as u32, format);

        // Test format
        let buffer = swapchain.acquire()?.unwrap();

        {
            let dmabuf: Dmabuf = (*buffer).clone();
            match renderer.bind(dmabuf).map_err(Error::<E1, E2, E3>::RenderError)
            .and_then(|_| renderer.begin(mode.size().0 as u32, mode.size().1 as u32, Transform::Normal).map_err(Error::RenderError))
            .and_then(|_| renderer.clear([0.0, 0.0, 0.0, 1.0]).map_err(Error::RenderError))
            .and_then(|_| renderer.finish().map_err(|_| Error::InitialRenderingError))
            .and_then(|_| renderer.unbind().map_err(Error::RenderError))
            {
                Ok(_) => {},
                Err(err) => {
                    warn!(logger, "Rendering failed with format {:?}: {}", format, err);
                    return DrmRenderSurface::new_internal(drm, swapchain.allocator, renderer, formats, logger);
                }
            }
        }

        let bo = import_dmabuf(&drm, &gbm, &*buffer)?;
        let fb = bo.userdata().unwrap().unwrap().fb;
        buffer.set_userdata(bo);

        match drm.test_buffer(fb, &mode, true)
        {
            Ok(_) => {
                debug!(logger, "Success, choosen format: {:?}", format);
                let buffers = Buffers::new(drm.clone(), gbm, buffer);
                Ok(DrmRenderSurface {
                    drm,
                    format: format.code,
                    renderer,
                    swapchain,
                    buffers,
                    current_buffer: None,
                })
            },
            Err(err) => {
                warn!(logger, "Mode-setting failed with buffer format {:?}: {}", format, err);
                DrmRenderSurface::new_internal(drm, swapchain.allocator, renderer, formats, logger)
            }
        }
    }
    
    pub fn queue_frame(&mut self) -> Result<(), Error<E1, E2, E3>> {
        let mode = self.drm.pending_mode();
        let (width, height) = (mode.size().0 as u32, mode.size().1 as u32);
        self.begin(width, height, Transform::Flipped180/* TODO */)
    }

    pub fn drop_frame(&mut self) -> Result<(), SwapBuffersError> {
        if self.current_buffer.is_none() {
            return Ok(());
        }

        // finish the renderer in case it needs it
        let result = self.renderer.finish();
        // but do not queue the buffer, drop it in any case
        let _ = self.current_buffer.take();
        result
    }

    pub fn frame_submitted(&mut self) -> Result<(), Error<E1, E2, E3>> {
        self.buffers.submitted()
    }
    
    pub fn crtc(&self) -> crtc::Handle {
        self.drm.crtc()
    }
    
    pub fn plane(&self) -> plane::Handle {
        self.drm.plane()
    }

    pub fn current_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.drm.current_connectors()
    }

    pub fn pending_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.drm.pending_connectors()
    }

    pub fn add_connector(&self, connector: connector::Handle) -> Result<(), Error<E1, E2, E3>> {
        self.drm.add_connector(connector).map_err(Error::DrmError)
    }

    pub fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error<E1, E2, E3>> {
        self.drm.remove_connector(connector).map_err(Error::DrmError)
    }

    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error<E1, E2, E3>> {
        self.drm.set_connectors(connectors).map_err(Error::DrmError)
    }

    pub fn current_mode(&self) -> Mode {
        self.drm.current_mode()
    }

    pub fn pending_mode(&self) -> Mode {
        self.drm.pending_mode()
    }

    pub fn use_mode(&self, mode: Mode) -> Result<(), Error<E1, E2, E3>> {
        self.drm.use_mode(mode).map_err(Error::DrmError)
    }
}


impl<D, A, B, T, R, E1, E2, E3> Renderer for DrmRenderSurface<D, A, R, B>
where
    D: AsRawFd + 'static,
    A: Allocator<B, Error=E1>,
    B: Buffer + TryInto<Dmabuf, Error=E2>,
    R: Bind<Dmabuf> + Renderer<Error=E3, TextureId=T>,
    T: Texture,
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + 'static,
{
    type Error = Error<E1, E2, E3>;
    type TextureId = T;

    #[cfg(feature = "image")]
    fn import_bitmap<C: std::ops::Deref<Target=[u8]>>(&mut self, image: &image::ImageBuffer<image::Rgba<u8>, C>) -> Result<Self::TextureId, Self::Error> {
        self.renderer.import_bitmap(image).map_err(Error::RenderError)
    }

    #[cfg(feature = "wayland_frontend")]
    fn shm_formats(&self) -> &[wl_shm::Format] {
        self.renderer.shm_formats()
    }

    #[cfg(feature = "wayland_frontend")]
    fn import_shm(&mut self, buffer: &wl_buffer::WlBuffer) -> Result<Self::TextureId, Self::Error> {
        self.renderer.import_shm(buffer).map_err(Error::RenderError)
    }
    
    #[cfg(feature = "wayland_frontend")]
    fn import_egl(&mut self, buffer: &EGLBuffer) -> Result<Self::TextureId, Self::Error> {
        self.renderer.import_egl(buffer).map_err(Error::RenderError)       
    }

    fn destroy_texture(&mut self, texture: Self::TextureId) -> Result<(), Self::Error> {
        self.renderer.destroy_texture(texture).map_err(Error::RenderError)
    }
    
    fn begin(&mut self, width: u32, height: u32, transform: Transform) -> Result<(), Error<E1, E2, E3>> {
        if self.current_buffer.is_some() {
            return Ok(());
        }

        let slot = self.swapchain.acquire()?.ok_or(Error::NoFreeSlotsError)?;
        self.renderer.bind((*slot).clone()).map_err(Error::RenderError)?;
        self.current_buffer = Some(slot);
        self.renderer.begin(width, height, transform).map_err(Error::RenderError)
    }

   fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error> {
        self.renderer.clear(color).map_err(Error::RenderError)
    }
    
    fn render_texture(&mut self, texture: &Self::TextureId, matrix: Matrix3<f32>, alpha: f32) -> Result<(), Self::Error> {
        self.renderer.render_texture(texture, matrix, alpha).map_err(Error::RenderError)
    }

    fn finish(&mut self) -> Result<(), SwapBuffersError> {
        if self.current_buffer.is_none() {
            return Err(SwapBuffersError::AlreadySwapped);
        }

        let result = self.renderer.finish();
        if result.is_ok() {
            match self.buffers.queue::<E1, E2, E3>(self.current_buffer.take().unwrap()) {
                Ok(()) => {}
                Err(Error::DrmError(drm)) => return Err(drm.into()),
                Err(Error::GbmError(err)) => return Err(SwapBuffersError::ContextLost(Box::new(err))),
                _ => unreachable!(),
            }
        }
        result
    }
}

struct FbHandle<D: AsRawFd + 'static> {
    drm: Arc<DrmSurface<D>>,
    fb: framebuffer::Handle,
}

impl<A: AsRawFd + 'static> Drop for FbHandle<A> {
    fn drop(&mut self) {
        let _ = self.drm.destroy_framebuffer(self.fb);
    }
}

struct Buffers<D: AsRawFd + 'static> {
    gbm: GbmDevice<gbm::FdWrapper>,
    drm: Arc<DrmSurface<D>>,
    _current_fb: Slot<Dmabuf, BufferObject<FbHandle<D>>>,
    pending_fb: Option<Slot<Dmabuf, BufferObject<FbHandle<D>>>>,
    queued_fb: Option<Slot<Dmabuf, BufferObject<FbHandle<D>>>>,
}

impl<D> Buffers<D>
where
    D: AsRawFd + 'static,
{
    pub fn new(drm: Arc<DrmSurface<D>>, gbm: GbmDevice<gbm::FdWrapper>, slot: Slot<Dmabuf, BufferObject<FbHandle<D>>>) -> Buffers<D> {
        Buffers {
            drm,
            gbm,
            _current_fb: slot,
            pending_fb: None,
            queued_fb: None,
        }
    }

    pub fn queue<E1, E2, E3>(&mut self, slot: Slot<Dmabuf, BufferObject<FbHandle<D>>>) -> Result<(), Error<E1, E2, E3>>
    where
        E1: std::error::Error + 'static,
        E2: std::error::Error + 'static,
        E3: std::error::Error + 'static,
    {
        if slot.userdata().is_none() {
            let bo = import_dmabuf(&self.drm, &self.gbm, &*slot)?;
            slot.set_userdata(bo);
        }

        self.queued_fb = Some(slot);
        if self.pending_fb.is_none() {
            self.submit()
        } else {
            Ok(())
        }
    }

    pub fn submitted<E1, E2, E3>(&mut self) -> Result<(), Error<E1, E2, E3>> 
    where
        E1: std::error::Error + 'static,
        E2: std::error::Error + 'static,
        E3: std::error::Error + 'static,
    {
        if self.pending_fb.is_none() {
            return Ok(());
        }
        self._current_fb = self.pending_fb.take().unwrap();
        if self.queued_fb.is_some() {
            self.submit()
        } else {
            Ok(())
        }
    }

    fn submit<E1, E2, E3>(&mut self) -> Result<(), Error<E1, E2, E3>> 
    where
        E1: std::error::Error + 'static,
        E2: std::error::Error + 'static,
        E3: std::error::Error + 'static,
    {
        // yes it does not look like it, but both of these lines should be safe in all cases.
        let slot = self.queued_fb.take().unwrap();
        let fb = slot.userdata().as_ref().unwrap().userdata().unwrap().unwrap().fb;

        let flip = if self.drm.commit_pending() {
            self.drm.commit(fb, true)
        } else {
            self.drm.page_flip(fb, true)
        };
        if flip.is_ok() {
            self.pending_fb = Some(slot);
        }
        flip.map_err(Error::DrmError)
    }
}

fn import_dmabuf<A, E1, E2, E3>(drm: &Arc<DrmSurface<A>>, gbm: &GbmDevice<gbm::FdWrapper>, buffer: &Dmabuf) -> Result<BufferObject<FbHandle<A>>, Error<E1, E2, E3>>
where
    A: AsRawFd + 'static,
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + 'static,
{
    // TODO check userdata and return early
    let mut bo = buffer.import(&gbm, BufferObjectFlags::SCANOUT)?;
    let modifier = match bo.modifier().unwrap() {
        Modifier::Invalid => None,
        x => Some(x),
    };
    
    let logger = match &*(*drm).internal {
        DrmSurfaceInternal::Atomic(surf) => surf.logger.clone(),
        DrmSurfaceInternal::Legacy(surf) => surf.logger.clone(),
    };

    let fb = match
        if modifier.is_some() {
            let num = bo.plane_count().unwrap();
            let modifiers = [
                modifier,
                if num > 1 { modifier } else { None },
                if num > 2 { modifier } else { None },
                if num > 3 { modifier } else { None },
            ];
            drm.add_planar_framebuffer(&bo, &modifiers, drm_ffi::DRM_MODE_FB_MODIFIERS)
        } else {
            drm.add_planar_framebuffer(&bo, &[None, None, None, None], 0)
        }
    {
        Ok(fb) => fb,
        Err(source) => {
            // We only support this as a fallback of last resort for ARGB8888 visuals,
            // like xf86-video-modesetting does.
            if drm::buffer::Buffer::format(&bo) != Fourcc::Argb8888
            || bo.handles()[1].is_some() {
                return Err(Error::DrmError(DrmError::Access {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                }));
            }
            debug!(logger, "Failed to add framebuffer, trying legacy method");
            drm.add_framebuffer(&bo, 32, 32).map_err(|source| DrmError::Access {
                errmsg: "Failed to add framebuffer",
                dev: drm.dev_path(),
                source,
            })?
        }
    };
    bo.set_userdata(FbHandle {
        drm: drm.clone(),
        fb,
    }).unwrap();

    Ok(bo)
}

#[derive(Debug, thiserror::Error)]
pub enum Error<E1, E2, E3>
where
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + 'static,
{
    #[error("No supported plane buffer format found")]
    NoSupportedPlaneFormat,
    #[error("No supported renderer buffer format found")]
    NoSupportedRendererFormat,
    #[error("Supported plane and renderer buffer formats are incompatible")]
    FormatsNotCompatible,
    #[error("Failed to allocate a new buffer")]
    NoFreeSlotsError,
    #[error("Failed to render test frame")]
    InitialRenderingError,
    #[error("The underlying drm surface encounted an error: {0}")]
    DrmError(#[from] DrmError),
    #[error("The underlying gbm device encounted an error: {0}")]
    GbmError(#[from] std::io::Error),
    #[error("The swapchain encounted an error: {0}")]
    SwapchainError(#[from] SwapchainError<E1, E2>),
    #[error("The renderer encounted an error: {0}")]
    RenderError(#[source] E3)
}

impl<
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + Into<SwapBuffersError> + 'static,
> From<Error<E1, E2, E3>> for SwapBuffersError {
    fn from(err: Error<E1, E2, E3>) -> SwapBuffersError {
        match err {
            x @ Error::NoSupportedPlaneFormat
            | x @ Error::NoSupportedRendererFormat
            | x @ Error::FormatsNotCompatible
            | x @ Error::InitialRenderingError
            => SwapBuffersError::ContextLost(Box::new(x)),
            x @ Error::NoFreeSlotsError => SwapBuffersError::TemporaryFailure(Box::new(x)),
            Error::DrmError(err) => err.into(),
            Error::GbmError(err) => SwapBuffersError::ContextLost(Box::new(err)),
            Error::SwapchainError(err) => SwapBuffersError::ContextLost(Box::new(err)),
            Error::RenderError(err) => err.into(),
        }
    }
}