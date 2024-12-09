use std::{
    collections::HashMap,
    fmt,
    marker::PhantomData,
    os::fd::AsFd,
    sync::{Arc, Mutex, RwLock, TryLockError},
};

use drm::control::{self, connector, crtc, Mode};
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};

use crate::{
    backend::{
        allocator::{
            dmabuf::{AsDmabuf, Dmabuf},
            gbm::GbmDevice,
            Allocator,
        },
        renderer::{element::RenderElement, Bind, Color32F, DebugFlags, Renderer, Texture},
    },
    output::OutputModeSource,
};

use super::{
    compositor::{
        DrmCompositor, FrameError, FrameMode, FrameResult, RenderFrameError, RenderFrameErrorType,
        RenderFrameResult,
    },
    exporter::ExportFramebuffer,
    DrmDevice, DrmError, Planes,
};

type CompositorList<A, F, U, G> = Arc<RwLock<HashMap<crtc::Handle, Mutex<DrmCompositor<A, F, U, G>>>>>;

pub struct DrmOutputManager<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: fmt::Debug + 'static,
    G: AsFd + 'static,
{
    device: DrmDevice,
    allocator: A,
    exporter: F,
    gbm: Option<GbmDevice<G>>,
    compositor: CompositorList<A, F, U, G>,
    color_formats: Vec<DrmFourcc>,
    renderer_formats: Vec<DrmFormat>,
}

impl<A, F, U, G> fmt::Debug for DrmOutputManager<A, F, U, G>
where
    A: Allocator + fmt::Debug,
    <A as Allocator>::Buffer: fmt::Debug,
    F: ExportFramebuffer<<A as Allocator>::Buffer> + fmt::Debug,
    U: fmt::Debug,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: fmt::Debug + 'static,
    G: AsFd + fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DrmOutputManager")
            .field("device", &self.device)
            .field("allocator", &self.allocator)
            .field("exporter", &self.exporter)
            .field("gbm", &self.gbm)
            .field("compositor", &self.compositor)
            .field("color_formats", &self.color_formats)
            .field("renderer_formats", &self.renderer_formats)
            .finish()
    }
}

impl<A, F, U, G> DrmOutputManager<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: fmt::Debug + 'static,
    G: AsFd + 'static,
{
    pub fn device(&self) -> &DrmDevice {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut DrmDevice {
        &mut self.device
    }
}

impl<A, F, U, G> DrmOutputManager<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    pub fn pause(&mut self) {
        self.device.pause();
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DrmOutputManagerError<A, B, F, R>
where
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
    R: std::error::Error + Send + Sync + 'static,
{
    #[error("The specified CRTC {0:?} is already in use.")]
    DuplicateCrtc(crtc::Handle),
    #[error(transparent)]
    Drm(#[from] DrmError),
    #[error(transparent)]
    Frame(FrameError<A, B, F>),
    #[error(transparent)]
    RenderFrame(RenderFrameError<A, B, F, R>),
}

impl<A, F, U, G> DrmOutputManager<A, F, U, G>
where
    A: Allocator + std::clone::Clone + std::fmt::Debug,
    <A as Allocator>::Buffer: AsDmabuf,
    <A as Allocator>::Error: Send + Sync + 'static,
    <<A as crate::backend::allocator::Allocator>::Buffer as AsDmabuf>::Error:
        std::marker::Send + std::marker::Sync + 'static,
    F: ExportFramebuffer<<A as Allocator>::Buffer> + std::clone::Clone,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error:
        std::marker::Send + std::marker::Sync + 'static,
    G: AsFd + std::clone::Clone + 'static,
    U: 'static,
{
    pub fn new(
        device: DrmDevice,
        allocator: A,
        exporter: F,
        gbm: Option<GbmDevice<G>>,
        color_formats: impl IntoIterator<Item = DrmFourcc>,
        renderer_formats: impl IntoIterator<Item = DrmFormat>,
    ) -> Self {
        Self {
            device,
            allocator: allocator,
            exporter,
            gbm,
            compositor: Default::default(),
            color_formats: color_formats.into_iter().collect(),
            renderer_formats: renderer_formats.into_iter().collect(),
        }
    }

    pub fn initialize_output<'a, R, E>(
        &'a mut self,
        crtc: crtc::Handle,
        mode: control::Mode,
        connectors: &[connector::Handle],
        output_mode_source: impl Into<OutputModeSource> + std::fmt::Debug,
        planes: Option<Planes>,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> Result<
        DrmOutput<A, F, U, G>,
        DrmOutputManagerError<
            <A as Allocator>::Error,
            <<A as Allocator>::Buffer as AsDmabuf>::Error,
            <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
            R::Error,
        >,
    >
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        <R as Renderer>::TextureId: Texture + 'static,
        <R as Renderer>::Error: Send + Sync + 'static,
    {
        let output_mode_source = output_mode_source.into();

        let mut write_guard = self.compositor.write().unwrap();
        if write_guard.contains_key(&crtc) {
            return Err(DrmOutputManagerError::DuplicateCrtc(crtc));
        }

        let mut create_compositor =
            |implicit_modifiers: bool| -> FrameResult<DrmCompositor<A, F, U, G>, A, F> {
                let surface = self.device.create_surface(crtc, mode, connectors)?;

                if implicit_modifiers {
                    DrmCompositor::<A, F, U, G>::new(
                        output_mode_source.clone(),
                        surface,
                        planes.clone(),
                        self.allocator.clone(),
                        self.exporter.clone(),
                        self.color_formats.iter().copied(),
                        self.renderer_formats
                            .iter()
                            .filter(|f| f.modifier == DrmModifier::Invalid)
                            .copied(),
                        self.device.cursor_size(),
                        self.gbm.clone(),
                    )
                } else {
                    DrmCompositor::<A, F, U, G>::new(
                        output_mode_source.clone(),
                        surface,
                        planes.clone(),
                        self.allocator.clone(),
                        self.exporter.clone(),
                        self.color_formats.iter().copied(),
                        self.renderer_formats.iter().copied(),
                        self.device.cursor_size(),
                        self.gbm.clone(),
                    )
                }
            };

        let compositor = create_compositor(false);

        // Okay, so this can fail for various reasons...
        //
        //  Enabling an additional CRTC might fail because the bandwidth
        //  requirement is higher then supported with the current configuration.
        //
        // * Bandwidth limitation caused by overlay plane usage:
        //   Each overlay plane requires some certain bandwidth and we only
        //   test that during plane assignment implicitly through an atomic test.
        //   When trying to enable an additional CRTC we might hit some limit and the
        //   only way to resolve this might be to disable all overlay planes and
        //   retry enabling the CRTC.
        //
        // * Bandwidth limitation caused by the primary plane format:
        //   Different formats (might) require a higher memory bandwidth than others.
        //   This also applies to the same fourcc with different modifiers. For example
        //   the Intel CCS Formats use an additional plane to transport meta-data.
        //   So if we fail to enable an additional CRTC we might be able to resolve
        //   the issue by using a different format. Again the only way to know is by
        //   trying out what works.
        //
        // So for now we try to disable overlay planes first. If that doesn't work,
        // we set the modifiers to `Invalid` (for now) to give the driver the opportunity
        // to re-allocate in the background. (TODO: Do this ourselves).

        match compositor {
            Ok(compositor) => {
                write_guard.insert(crtc, Mutex::new(compositor));
            }
            Err(err) => {
                tracing::warn!(
                    ?crtc,
                    ?err,
                    "failed to initialize crtc, trying to lower bandwidth"
                );

                for compositor in write_guard.values_mut() {
                    let mut compositor = compositor.lock().unwrap();
                    compositor.reset_buffer_ages();
                    render_elements.submit_composited_frame(&mut *compositor, renderer)?;
                }

                let compositor = create_compositor(false);

                match compositor {
                    Ok(compositor) => {
                        write_guard.insert(crtc, Mutex::new(compositor));
                    }
                    Err(err) => {
                        tracing::warn!(
                            ?crtc,
                            ?err,
                            "failed to initialize crtc, trying implicit modifiers"
                        );

                        for compositor in write_guard.values_mut() {
                            let mut compositor = compositor.lock().unwrap();

                            let current_format = compositor.format();
                            if let Err(err) = compositor.set_format(
                                self.allocator.clone(),
                                current_format,
                                [DrmModifier::Invalid],
                            ) {
                                tracing::warn!(?err, "failed to set new format");
                                continue;
                            }

                            render_elements.submit_composited_frame(&mut *compositor, renderer)?;
                        }

                        let compositor = create_compositor(true);

                        match compositor {
                            Ok(compositor) => {
                                write_guard.insert(crtc, Mutex::new(compositor));
                            }
                            Err(err) => {
                                // try to reset formats
                                for compositor in write_guard.values_mut() {
                                    let mut compositor = compositor.lock().unwrap();

                                    let current_format = compositor.format();
                                    if let Err(err) = compositor.set_format(
                                        self.allocator.clone(),
                                        current_format,
                                        self.renderer_formats
                                            .iter()
                                            .filter(|f| f.code == current_format)
                                            .map(|f| f.modifier),
                                    ) {
                                        tracing::warn!(?err, "failed to reset format");
                                        continue;
                                    }

                                    render_elements.submit_composited_frame(&mut *compositor, renderer)?;
                                }

                                return Err(DrmOutputManagerError::Frame(err));
                            }
                        }
                    }
                }
            }
        };

        // We need to render the new output once to lock in the primary plane as used with the new format, so we don't hit the bandwidth issue,
        // when downstream potentially uses `FrameMode::ALL` immediately after this.

        let compositor = write_guard.get_mut(&crtc).unwrap();
        let mut compositor = compositor.lock().unwrap();
        render_elements.submit_composited_frame(&mut *compositor, renderer)?;

        Ok(DrmOutput {
            crtc,
            compositor: self.compositor.clone(),
        })
    }

    pub fn with_compositors<R>(
        &mut self,
        f: impl FnOnce(&HashMap<crtc::Handle, Mutex<DrmCompositor<A, F, U, G>>>) -> R,
    ) -> R {
        let write_guard = self.compositor.write().unwrap();
        f(&*write_guard)
    }

    pub fn use_mode<R, E>(
        &mut self,
        crtc: &crtc::Handle,
        mode: Mode,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> Result<
        (),
        DrmOutputManagerError<
            <A as Allocator>::Error,
            <<A as Allocator>::Buffer as AsDmabuf>::Error,
            <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
            R::Error,
        >,
    >
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        <R as Renderer>::TextureId: Texture + 'static,
        <R as Renderer>::Error: Send + Sync + 'static,
    {
        use_mode_internal(&self.compositor, crtc, mode, renderer, render_elements)
    }

    pub fn try_to_restore_modifiers<R, E>(
        &mut self,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> Result<
        (),
        DrmOutputManagerError<
            <A as Allocator>::Error,
            <<A as Allocator>::Buffer as AsDmabuf>::Error,
            <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
            R::Error,
        >,
    >
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        <R as Renderer>::TextureId: Texture + 'static,
        <R as Renderer>::Error: Send + Sync + 'static,
    {
        let mut write_guard = self.compositor.write().unwrap();

        // check if implicit modifiers are in use
        if write_guard
            .values_mut()
            .any(|c| c.lock().unwrap().modifiers() == &[DrmModifier::Invalid])
        {
            // if so, first lower the bandwidth by disabling planes on all compositors
            for compositor in write_guard.values_mut() {
                let mut compositor = compositor.lock().unwrap();
                compositor.reset_buffer_ages();
                render_elements.submit_composited_frame(&mut *compositor, renderer)?;
            }

            for compositor in write_guard.values_mut() {
                let mut compositor = compositor.lock().unwrap();
                if compositor.modifiers() != &[DrmModifier::Invalid] {
                    continue;
                }

                let current_format = compositor.format();
                if let Err(err) = compositor.set_format(
                    self.allocator.clone(),
                    current_format,
                    self.renderer_formats
                        .iter()
                        .filter(|f| f.code == current_format)
                        .map(|f| f.modifier),
                ) {
                    tracing::warn!(?err, "failed to reset format");
                    continue;
                }

                render_elements.submit_composited_frame(&mut *compositor, renderer)?;
            }
        }

        Ok(())
    }

    pub fn activate(&mut self, disable_connectors: bool) -> Result<(), DrmError> {
        self.device.activate(disable_connectors)?;

        // We request a write guard here to guarantee unique access
        let write_guard = self.compositor.write().unwrap();
        for compositor in write_guard.values() {
            if let Err(err) = compositor.lock().unwrap().reset_state() {
                tracing::warn!("Failed to reset drm surface state: {}", err);
            }
        }

        Ok(())
    }
}

pub struct DrmOutput<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    crtc: crtc::Handle,
    compositor: CompositorList<A, F, U, G>,
}

impl<A, F, U, G> fmt::Debug for DrmOutput<A, F, U, G>
where
    A: Allocator + fmt::Debug,
    <A as Allocator>::Buffer: fmt::Debug,
    F: ExportFramebuffer<<A as Allocator>::Buffer> + fmt::Debug,
    U: fmt::Debug,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: fmt::Debug + 'static,
    G: AsFd + fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("DrmOutput");
        match self.compositor.try_read() {
            Ok(guard) => d.field("compositor", &guard.get(&self.crtc)),
            Err(TryLockError::Poisoned(err)) => d.field("compositor", &&**err.get_ref()),
            Err(TryLockError::WouldBlock) => d.field("compositor", &"<locked>"),
        };
        d.finish()
    }
}

impl<A, F, U, G> DrmOutput<A, F, U, G>
where
    A: Allocator + std::clone::Clone + fmt::Debug,
    <A as Allocator>::Buffer: AsDmabuf,
    <A as Allocator>::Error: Send + Sync + 'static,
    <<A as Allocator>::Buffer as AsDmabuf>::Error: std::marker::Send + std::marker::Sync + 'static,
    F: ExportFramebuffer<<A as Allocator>::Buffer> + std::clone::Clone,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error:
        std::marker::Send + std::marker::Sync + 'static,
    G: AsFd + std::clone::Clone + 'static,
    U: 'static,
{
    /// Set the [`DebugFlags`] to use
    ///
    /// Note: This will reset the primary plane swapchain if
    /// the flags differ from the current flags
    pub fn set_debug_flags(&self, flags: DebugFlags) {
        self.with_compositor(|compositor| compositor.set_debug_flags(flags));
    }

    /// Reset the underlying buffers
    pub fn reset_buffers(&self) {
        self.with_compositor(|compositor| compositor.reset_buffers());
    }

    pub fn frame_submitted(&self) -> FrameResult<Option<U>, A, F> {
        self.with_compositor(|compositor| compositor.frame_submitted())
    }

    pub fn format(&self) -> DrmFourcc {
        self.with_compositor(|compositor| compositor.format())
    }

    pub fn render_frame<'a, R, E>(
        &mut self,
        renderer: &mut R,
        elements: &'a [E],
        clear_color: impl Into<Color32F>,
        frame_mode: FrameMode,
    ) -> Result<RenderFrameResult<'a, A::Buffer, F::Framebuffer, E>, RenderFrameErrorType<A, F, R>>
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        <R as Renderer>::TextureId: Texture + 'static,
        <R as Renderer>::Error: Send + Sync + 'static,
    {
        self.with_compositor(|compositor| {
            compositor.render_frame(renderer, elements, clear_color, frame_mode)
        })
    }

    pub fn queue_frame(&mut self, user_data: U) -> FrameResult<(), A, F> {
        self.with_compositor(|compositor| compositor.queue_frame(user_data))
    }

    pub fn commit_frame(&mut self) -> FrameResult<(), A, F> {
        self.with_compositor(|compositor| compositor.commit_frame())
    }

    pub fn use_mode<R, E>(
        &mut self,
        mode: Mode,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> Result<
        (),
        DrmOutputManagerError<
            <A as Allocator>::Error,
            <<A as Allocator>::Buffer as AsDmabuf>::Error,
            <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
            R::Error,
        >,
    >
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        <R as Renderer>::TextureId: Texture + 'static,
        <R as Renderer>::Error: Send + Sync + 'static,
    {
        use_mode_internal(&self.compositor, &self.crtc, mode, renderer, render_elements)
    }
}

impl<A, F, U, G> DrmOutput<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    pub fn crtc(&self) -> crtc::Handle {
        self.crtc
    }

    pub fn with_compositor<T, R>(&self, f: T) -> R
    where
        T: FnOnce(&mut DrmCompositor<A, F, U, G>) -> R,
    {
        let read_guard = self.compositor.read().unwrap();
        let mut compositor_guard = read_guard.get(&self.crtc).unwrap().lock().unwrap();
        f(&mut compositor_guard)
    }
}

impl<A, F, U, G> Drop for DrmOutput<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    fn drop(&mut self) {
        let mut write_guard = self.compositor.write().unwrap();
        write_guard.remove(&self.crtc);
    }
}

fn use_mode_internal<A, F, U, G, R, E>(
    compositor: &CompositorList<A, F, U, G>,
    crtc: &crtc::Handle,
    mode: Mode,
    renderer: &mut R,
    render_elements: &DrmOutputRenderElements<R, E>,
) -> Result<
    (),
    DrmOutputManagerError<
        <A as Allocator>::Error,
        <<A as Allocator>::Buffer as AsDmabuf>::Error,
        <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
        R::Error,
    >,
>
where
    A: Allocator + std::clone::Clone + fmt::Debug,
    <A as Allocator>::Buffer: AsDmabuf,
    <A as Allocator>::Error: Send + Sync + 'static,
    <<A as crate::backend::allocator::Allocator>::Buffer as AsDmabuf>::Error:
        std::marker::Send + std::marker::Sync + 'static,
    F: ExportFramebuffer<<A as Allocator>::Buffer> + std::clone::Clone,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error:
        std::marker::Send + std::marker::Sync + 'static,
    G: AsFd + std::clone::Clone + 'static,
    U: 'static,
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    <R as Renderer>::TextureId: Texture + 'static,
    <R as Renderer>::Error: Send + Sync + 'static,
{
    let mut write_guard = compositor.write().unwrap();

    let mut res = {
        let mut compositor_guard = write_guard.get(crtc).unwrap().lock().unwrap();
        compositor_guard.use_mode(mode)
    };

    if res.is_err() {
        for compositor in write_guard.values_mut() {
            let mut compositor = compositor.lock().unwrap();
            render_elements.submit_composited_frame(&mut compositor, renderer)?;
        }

        let compositor = write_guard.get_mut(crtc).unwrap();
        let mut compositor = compositor.lock().unwrap();
        res = compositor.use_mode(mode);
    }

    res.map_err(DrmOutputManagerError::Frame)
}

#[derive(Debug)]
pub struct DrmOutputRenderElements<R, E>
where
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    <R as Renderer>::TextureId: Texture + 'static,
    <R as Renderer>::Error: Send + Sync + 'static,
{
    render_elements: HashMap<crtc::Handle, (Vec<E>, Color32F)>,
    _renderer: PhantomData<R>,
}

impl<R, E> Default for DrmOutputRenderElements<R, E>
where
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    <R as Renderer>::TextureId: Texture + 'static,
    <R as Renderer>::Error: Send + Sync + 'static,
{
    fn default() -> Self {
        DrmOutputRenderElements {
            render_elements: HashMap::new(),
            _renderer: PhantomData,
        }
    }
}

impl<R, E> DrmOutputRenderElements<R, E>
where
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    <R as Renderer>::TextureId: Texture + 'static,
    <R as Renderer>::Error: Send + Sync + 'static,
{
    pub fn add_output(
        &mut self,
        crtc: &crtc::Handle,
        clear_color: Color32F,
        elements: impl IntoIterator<Item = E>,
    ) {
        self.render_elements
            .insert(*crtc, (elements.into_iter().collect(), clear_color));
    }

    fn submit_composited_frame<A, F, U, G>(
        &self,
        compositor: &mut DrmCompositor<A, F, U, G>,
        renderer: &mut R,
    ) -> Result<
        (),
        DrmOutputManagerError<
            <A as Allocator>::Error,
            <<A as Allocator>::Buffer as AsDmabuf>::Error,
            <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
            R::Error,
        >,
    >
    where
        A: Allocator + std::clone::Clone + std::fmt::Debug,
        <A as Allocator>::Buffer: AsDmabuf,
        <A as Allocator>::Error: Send + Sync + 'static,
        <<A as crate::backend::allocator::Allocator>::Buffer as AsDmabuf>::Error:
            std::marker::Send + std::marker::Sync + 'static,
        F: ExportFramebuffer<<A as Allocator>::Buffer> + std::clone::Clone,
        <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
        <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error:
            std::marker::Send + std::marker::Sync + 'static,
        G: AsFd + std::clone::Clone + 'static,
        U: 'static,
    {
        let (elements, clear_color) = self
            .render_elements
            .get(&compositor.crtc())
            .map(|(ref elements, ref color)| (&**elements, color))
            .unwrap_or((&[], &Color32F::BLACK));
        compositor
            .render_frame(renderer, &elements, *clear_color, FrameMode::COMPOSITE)
            .map_err(DrmOutputManagerError::RenderFrame)?;
        compositor.commit_frame().map_err(DrmOutputManagerError::Frame)?;
        Ok(())
    }
}
