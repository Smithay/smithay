use std::collections::HashSet;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use cgmath::Matrix3;
use drm::buffer::PlanarBuffer;
use drm::control::{connector, crtc, framebuffer, plane, Device, Mode};
use gbm::{BufferObject, BufferObjectFlags, Device as GbmDevice};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

use super::{device::DevPath, surface::DrmSurfaceInternal, DrmError, DrmSurface};
use crate::backend::{
    allocator::{
        dmabuf::{AsDmabuf, Dmabuf}, Allocator, Buffer, Format, Fourcc, Modifier, Slot, Swapchain,
    },
};
use crate::backend::egl::EGLBuffer;
use crate::backend::renderer::{Bind, Renderer, Texture, Transform};
use crate::backend::SwapBuffersError;

/// Simplified by limited abstraction to link single [`DrmSurface`]s to renderers.
///
/// # Use-case
///
/// In some scenarios it might be enough to use of a drm-surface as the one and only target
/// of a single renderer. In these cases `DrmRenderSurface` provides a way to quickly
/// get up and running without manually handling and binding buffers.
pub struct DrmRenderSurface<
    D: AsRawFd + 'static,
    A: Allocator<B>,
    R: Bind<Dmabuf>,
    B: Buffer,
> {
    _format: Format,
    buffers: Buffers<D, B>,
    current_buffer: Option<(Slot<B, (Dmabuf, BufferObject<FbHandle<D>>)>, Dmabuf)>,
    swapchain: Swapchain<A, B, (Dmabuf, BufferObject<FbHandle<D>>)>,
    renderer: R,
    drm: Arc<DrmSurface<D>>,
}

impl<D, A, B, R, E1, E2, E3> DrmRenderSurface<D, A, R, B>
where
    D: AsRawFd + 'static,
    A: Allocator<B, Error = E1>,
    B: Buffer + AsDmabuf<Error=E2>,
    R: Bind<Dmabuf> + Renderer<Error = E3>,
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + 'static,
{
    /// Create a new `DrmRendererSurface` from a given compatible combination
    /// of a surface, an allocator and a renderer.
    ///
    /// To sucessfully call this function, you need to have a renderer,
    /// which can render into a Dmabuf, and an allocator, which can create
    /// a buffer type, which can be converted into a Dmabuf.
    ///
    /// The function will futhermore check for compatibility by enumerating
    /// supported pixel formats and choosing an appropriate one.
    #[allow(clippy::type_complexity)]
    pub fn new<L: Into<Option<::slog::Logger>>>(
        drm: DrmSurface<D>,
        allocator: A,
        renderer: R,
        log: L,
    ) -> Result<DrmRenderSurface<D, A, R, B>, Error<E1, E2, E3>> {
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
        let plane_formats = drm
            .supported_formats()
            .iter()
            .filter(|fmt| fmt.code == code)
            .cloned()
            .collect::<HashSet<_>>();
        let renderer_formats = Bind::<Dmabuf>::supported_formats(&renderer)
            .expect("Dmabuf renderer without formats")
            .iter()
            .filter(|fmt| fmt.code == code)
            .cloned()
            .collect::<HashSet<_>>();

        trace!(logger, "Remaining plane formats: {:?}", plane_formats);
        trace!(logger, "Remaining renderer formats: {:?}", renderer_formats);
        debug!(
            logger,
            "Remaining intersected formats: {:?}",
            plane_formats
                .intersection(&renderer_formats)
                .collect::<HashSet<_>>()
        );

        if plane_formats.is_empty() {
            return Err(Error::NoSupportedPlaneFormat);
        } else if renderer_formats.is_empty() {
            return Err(Error::NoSupportedRendererFormat);
        }

        let formats = {
            // Special case: if a format supports explicit LINEAR (but no implicit Modifiers)
            // and the other doesn't support any modifier, force LINEAR. This will force the allocator to
            // create a buffer with a LINEAR layout instead of an implicit modifier.
            if (plane_formats.len() == 1
                && plane_formats.iter().next().unwrap().modifier == Modifier::Invalid
                && renderer_formats.iter().all(|x| x.modifier != Modifier::Invalid)
                && renderer_formats.iter().any(|x| x.modifier == Modifier::Linear))
                || (renderer_formats.len() == 1
                    && renderer_formats.iter().next().unwrap().modifier == Modifier::Invalid
                    && plane_formats.iter().all(|x| x.modifier != Modifier::Invalid)
                    && plane_formats.iter().any(|x| x.modifier == Modifier::Linear))
            {
                vec![Format {
                    code,
                    modifier: Modifier::Linear,
                }]
            } else {
                plane_formats
                    .intersection(&renderer_formats)
                    .cloned()
                    .collect::<Vec<_>>()
            }
        };
        debug!(logger, "Testing Formats: {:?}", formats);

        // Test explicit formats first
        let drm = Arc::new(drm);
        let iter = formats
            .iter()
            .filter(|x| x.modifier != Modifier::Invalid && x.modifier != Modifier::Linear)
            .chain(formats.iter().find(|x| x.modifier == Modifier::Linear))
            .chain(formats.iter().find(|x| x.modifier == Modifier::Invalid))
            .cloned();

        DrmRenderSurface::new_internal(drm, allocator, renderer, iter, logger)
    }

    #[allow(clippy::type_complexity)]
    fn new_internal(
        drm: Arc<DrmSurface<D>>,
        allocator: A,
        mut renderer: R,
        mut formats: impl Iterator<Item = Format>,
        logger: ::slog::Logger,
    ) -> Result<DrmRenderSurface<D, A, R, B>, Error<E1, E2, E3>> {
        let format = formats.next().ok_or(Error::NoSupportedPlaneFormat)?;
        let mode = drm.pending_mode();

        let gbm = unsafe { GbmDevice::new_from_fd(drm.as_raw_fd())? };
        let mut swapchain = Swapchain::new(allocator, mode.size().0 as u32, mode.size().1 as u32, format);

        // Test format
        let buffer = swapchain.acquire().map_err(Error::SwapchainError)?.unwrap();
        let dmabuf = buffer.export().map_err(Error::AsDmabufError)?;

        {
            match renderer
                .bind(dmabuf.clone())
                .map_err(Error::<E1, E2, E3>::RenderError)
                .and_then(|_| {
                    renderer
                        .begin(mode.size().0 as u32, mode.size().1 as u32, Transform::Normal)
                        .map_err(Error::RenderError)
                })
                .and_then(|_| renderer.clear([0.0, 0.0, 0.0, 1.0]).map_err(Error::RenderError))
                .and_then(|_| renderer.finish().map_err(|_| Error::InitialRenderingError))
                .and_then(|_| renderer.unbind().map_err(Error::RenderError))
            {
                Ok(_) => {}
                Err(err) => {
                    warn!(logger, "Rendering failed with format {:?}: {}", format, err);
                    return DrmRenderSurface::new_internal(
                        drm,
                        swapchain.allocator,
                        renderer,
                        formats,
                        logger,
                    );
                }
            }
        }

        let bo = import_dmabuf(&drm, &gbm, &dmabuf)?;
        let fb = bo.userdata().unwrap().unwrap().fb;
        buffer.set_userdata((dmabuf, bo));

        match drm.test_buffer(fb, &mode, true) {
            Ok(_) => {
                debug!(logger, "Success, choosen format: {:?}", format);
                let buffers = Buffers::new(drm.clone(), gbm, buffer);
                Ok(DrmRenderSurface {
                    drm,
                    _format: format,
                    renderer,
                    swapchain,
                    buffers,
                    current_buffer: None,
                })
            }
            Err(err) => {
                warn!(
                    logger,
                    "Mode-setting failed with buffer format {:?}: {}", format, err
                );
                DrmRenderSurface::new_internal(drm, swapchain.allocator, renderer, formats, logger)
            }
        }
    }

    /// Shortcut to [`Renderer::begin`] with the pending mode as dimensions.
    pub fn queue_frame(&mut self) -> Result<(), Error<E1, E2, E3>> {
        let mode = self.drm.pending_mode();
        let (width, height) = (mode.size().0 as u32, mode.size().1 as u32);
        self.begin(width, height, Transform::Normal)
    }

    /// Shortcut to abort the current frame.
    ///
    /// Allows [`DrmRenderSurface::queue_frame`] or [`Renderer::begin`] to be called again
    /// without displaying the current rendering context to the user.
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

    /// Marks the current frame as submitted.
    ///
    /// Needs to be called, after the vblank event of the matching [`DrmDevice`](super::DrmDevice)
    /// was received after calling [`Renderer::finish`] on this surface. Otherwise the rendering
    /// will run out of buffers eventually.
    pub fn frame_submitted(&mut self) -> Result<(), Error<E1, E2, E3>> {
        self.buffers.submitted()
    }

    /// Returns the underlying [`crtc`](drm::control::crtc) of this surface
    pub fn crtc(&self) -> crtc::Handle {
        self.drm.crtc()
    }

    /// Returns the underlying [`plane`](drm::control::plane) of this surface
    pub fn plane(&self) -> plane::Handle {
        self.drm.plane()
    }

    /// Currently used [`connector`](drm::control::connector)s of this `Surface`
    pub fn current_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.drm.current_connectors()
    }

    /// Returns the pending [`connector`](drm::control::connector)s
    /// used after the next [`commit`](Surface::commit) of this [`Surface`]
    pub fn pending_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.drm.pending_connectors()
    }

    /// Tries to add a new [`connector`](drm::control::connector)
    /// to be used after the next commit.
    ///
    /// **Warning**: You need to make sure, that the connector is not used with another surface
    /// or was properly removed via `remove_connector` + `commit` before adding it to another surface.
    /// Behavior if failing to do so is undefined, but might result in rendering errors or the connector
    /// getting removed from the other surface without updating it's internal state.
    ///
    /// Fails if the `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).
    pub fn add_connector(&self, connector: connector::Handle) -> Result<(), Error<E1, E2, E3>> {
        self.drm.add_connector(connector).map_err(Error::DrmError)
    }

    /// Tries to mark a [`connector`](drm::control::connector)
    /// for removal on the next commit.    
    pub fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error<E1, E2, E3>> {
        self.drm.remove_connector(connector).map_err(Error::DrmError)
    }

    /// Tries to replace the current connector set with the newly provided one on the next commit.
    ///
    /// Fails if one new `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).    
    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error<E1, E2, E3>> {
        self.drm.set_connectors(connectors).map_err(Error::DrmError)
    }

    /// Returns the currently active [`Mode`](drm::control::Mode)
    /// of the underlying [`crtc`](drm::control::crtc)    
    pub fn current_mode(&self) -> Mode {
        self.drm.current_mode()
    }

    /// Returns the currently pending [`Mode`](drm::control::Mode)
    /// to be used after the next commit.    
    pub fn pending_mode(&self) -> Mode {
        self.drm.pending_mode()
    }

    /// Tries to set a new [`Mode`](drm::control::Mode)
    /// to be used after the next commit.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`](drm::control::crtc) or any of the
    /// pending [`connector`](drm::control::connector)s.
    pub fn use_mode(&self, mode: Mode) -> Result<(), Error<E1, E2, E3>> {
        self.drm.use_mode(mode).map_err(Error::DrmError)
    }
}

impl<D, A, B, T, R, E1, E2, E3> Renderer for DrmRenderSurface<D, A, R, B>
where
    D: AsRawFd + 'static,
    A: Allocator<B, Error = E1>,
    B: Buffer + AsDmabuf<Error=E2>,
    R: Bind<Dmabuf> + Renderer<Error = E3, TextureId = T>,
    T: Texture,
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + 'static,
{
    type Error = Error<E1, E2, E3>;
    type TextureId = T;

    #[cfg(feature = "image")]
    fn import_bitmap<C: std::ops::Deref<Target = [u8]>>(
        &mut self,
        image: &image::ImageBuffer<image::Rgba<u8>, C>,
    ) -> Result<Self::TextureId, Self::Error> {
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

    fn begin(&mut self, width: u32, height: u32, _transform: Transform) -> Result<(), Error<E1, E2, E3>> {
        if self.current_buffer.is_some() {
            return Ok(());
        }

        let slot = self.swapchain.acquire().map_err(Error::SwapchainError)?.ok_or(Error::NoFreeSlotsError)?;
        let dmabuf = match &*slot.userdata() {
            Some((buf, _)) => buf.clone(),
            None =>  (*slot).export().map_err(Error::AsDmabufError)?,
        };
        self.renderer.bind(dmabuf.clone()).map_err(Error::RenderError)?;
        self.current_buffer = Some((slot, dmabuf));
        self.renderer
            .begin(width, height, Transform::Flipped180 /* TODO: add Add<Transform> implementation to add and correct _transform here */)
            .map_err(Error::RenderError)
    }

    fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error> {
        self.renderer.clear(color).map_err(Error::RenderError)
    }

    fn render_texture(
        &mut self,
        texture: &Self::TextureId,
        matrix: Matrix3<f32>,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        self.renderer
            .render_texture(texture, matrix, alpha)
            .map_err(Error::RenderError)
    }

    fn finish(&mut self) -> Result<(), SwapBuffersError> {
        if self.current_buffer.is_none() {
            return Err(SwapBuffersError::AlreadySwapped);
        }

        let result = self.renderer.finish();
        if result.is_ok() {
            let (slot, dmabuf) = self.current_buffer.take().unwrap();
            match self
                .buffers
                .queue::<E1, E2, E3>(slot, dmabuf)
            {
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

struct Buffers<D: AsRawFd + 'static, B: Buffer> {
    gbm: GbmDevice<gbm::FdWrapper>,
    drm: Arc<DrmSurface<D>>,
    _current_fb: Slot<B, (Dmabuf, BufferObject<FbHandle<D>>)>,
    pending_fb: Option<Slot<B, (Dmabuf, BufferObject<FbHandle<D>>)>>,
    queued_fb: Option<Slot<B, (Dmabuf, BufferObject<FbHandle<D>>)>>,
}

impl<D, B> Buffers<D, B>
where
    B: Buffer + AsDmabuf,
    D: AsRawFd + 'static,
{
    pub fn new(
        drm: Arc<DrmSurface<D>>,
        gbm: GbmDevice<gbm::FdWrapper>,
        slot: Slot<B, (Dmabuf, BufferObject<FbHandle<D>>)>,
    ) -> Buffers<D, B> {
        Buffers {
            drm,
            gbm,
            _current_fb: slot,
            pending_fb: None,
            queued_fb: None,
        }
    }

    pub fn queue<E1, E2, E3>(
        &mut self,
        slot: Slot<B, (Dmabuf, BufferObject<FbHandle<D>>)>,
        dmabuf: Dmabuf,
    ) -> Result<(), Error<E1, E2, E3>>
    where
        B: AsDmabuf<Error=E2>,
        E1: std::error::Error + 'static,
        E2: std::error::Error + 'static,
        E3: std::error::Error + 'static,
    {
        if slot.userdata().is_none() {
            let bo = import_dmabuf(&self.drm, &self.gbm, &dmabuf)?;
            slot.set_userdata((dmabuf, bo));
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
        let fb = slot.userdata().as_ref().unwrap().1.userdata().unwrap().unwrap().fb;

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

fn import_dmabuf<A, E1, E2, E3>(
    drm: &Arc<DrmSurface<A>>,
    gbm: &GbmDevice<gbm::FdWrapper>,
    buffer: &Dmabuf,
) -> Result<BufferObject<FbHandle<A>>, Error<E1, E2, E3>>
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

    let fb = match if modifier.is_some() {
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
    } {
        Ok(fb) => fb,
        Err(source) => {
            // We only support this as a fallback of last resort for ARGB8888 visuals,
            // like xf86-video-modesetting does.
            if drm::buffer::Buffer::format(&bo) != Fourcc::Argb8888 || bo.handles()[1].is_some() {
                return Err(Error::DrmError(DrmError::Access {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                }));
            }
            debug!(logger, "Failed to add framebuffer, trying legacy method");
            drm.add_framebuffer(&bo, 32, 32)
                .map_err(|source| DrmError::Access {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?
        }
    };
    bo.set_userdata(FbHandle { drm: drm.clone(), fb }).unwrap();

    Ok(bo)
}

/// Errors thrown by a [`DrmRenderSurface`]
#[derive(Debug, thiserror::Error)]
pub enum Error<E1, E2, E3>
where
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    E3: std::error::Error + 'static,
{
    /// No supported pixel format for the given plane could be determined
    #[error("No supported plane buffer format found")]
    NoSupportedPlaneFormat,
    /// No supported pixel format for the given renderer could be determined
    #[error("No supported renderer buffer format found")]
    NoSupportedRendererFormat,
    /// The supported pixel formats of the renderer and plane are incompatible
    #[error("Supported plane and renderer buffer formats are incompatible")]
    FormatsNotCompatible,
    /// The swapchain is exhausted, you need to call `frame_submitted`
    #[error("Failed to allocate a new buffer")]
    NoFreeSlotsError,
    /// Failed to renderer using the given renderer
    #[error("Failed to render test frame")]
    InitialRenderingError,
    /// Error accessing the drm device
    #[error("The underlying drm surface encounted an error: {0}")]
    DrmError(#[from] DrmError),
    /// Error importing the rendered buffer to libgbm for scan-out
    #[error("The underlying gbm device encounted an error: {0}")]
    GbmError(#[from] std::io::Error),
    /// Error allocating or converting newly created buffers
    #[error("The swapchain encounted an error: {0}")]
    SwapchainError(#[source] E1),
    /// Error exporting as Dmabuf
    #[error("The allocated buffer could not be exported as a dmabuf: {0}")]
    AsDmabufError(#[source] E2),
    /// Error during rendering
    #[error("The renderer encounted an error: {0}")]
    RenderError(#[source] E3),
}

impl<
        E1: std::error::Error + 'static,
        E2: std::error::Error + 'static,
        E3: std::error::Error + Into<SwapBuffersError> + 'static,
    > From<Error<E1, E2, E3>> for SwapBuffersError
{
    fn from(err: Error<E1, E2, E3>) -> SwapBuffersError {
        match err {
            x @ Error::NoSupportedPlaneFormat
            | x @ Error::NoSupportedRendererFormat
            | x @ Error::FormatsNotCompatible
            | x @ Error::InitialRenderingError => SwapBuffersError::ContextLost(Box::new(x)),
            x @ Error::NoFreeSlotsError => SwapBuffersError::TemporaryFailure(Box::new(x)),
            Error::DrmError(err) => err.into(),
            Error::GbmError(err) => SwapBuffersError::ContextLost(Box::new(err)),
            Error::SwapchainError(err) => SwapBuffersError::ContextLost(Box::new(err)),
            Error::AsDmabufError(err) => SwapBuffersError::ContextLost(Box::new(err)),
            Error::RenderError(err) => err.into(),
        }
    }
}
