//! Device-wide synchronization helpers

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
        renderer::{element::RenderElement, Bind, Color32F, DebugFlags, Renderer, RendererSuper, Texture},
    },
    output::OutputModeSource,
};

use super::{
    compositor::{
        DrmCompositor, FrameError, FrameFlags, FrameResult, PrimaryPlaneElement, RenderFrameError,
        RenderFrameErrorType, RenderFrameResult,
    },
    exporter::ExportFramebuffer,
    DrmDevice, DrmError, Planes,
};

type CompositorList<A, F, U, G> = Arc<RwLock<HashMap<crtc::Handle, Mutex<DrmCompositor<A, F, U, G>>>>>;

/// Provides synchronization between a [`DrmDevice`] and derived [`DrmOutput`]s.
///
/// When working with a single [`DrmDevice`] with multiple outputs and using the [`DrmCompositor`]
/// for hardware scanout one can quickly run into a set of bandwidth issues, where plane usage on
/// on [`DrmCompositor`] can cause commits on another to fail. Especially in multi-threaded contexts
/// these scenarios are difficult to handle as your need information over the state of all outputs at once.
///
/// The `DrmOutputManager` provides a way to handle operation commonly affected by this by locking the
/// whole device, while still providing [`DrmOutput`]-handles to drive individual surfaces.
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
    /// Access the underlying managed device
    pub fn device(&self) -> &DrmDevice {
        &self.device
    }

    /// Mutably access the underlying managed device
    pub fn device_mut(&mut self) -> &mut DrmDevice {
        &mut self.device
    }

    /// Access the [`Allocator`] of this output manager
    pub fn allocator(&self) -> &A {
        &self.allocator
    }
}

impl<A, F, U, G> DrmOutputManager<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    /// Pause the underlying device. See [`DrmDevice::pause`].
    pub fn pause(&mut self) {
        self.device.pause();
    }
}

/// Errors returned by `DrmOutputManager`'s methods
#[derive(thiserror::Error, Debug)]
pub enum DrmOutputManagerError<A, B, F, R>
where
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
    R: std::error::Error + Send + Sync + 'static,
{
    /// The specified CRTC is already in use
    #[error("The specified CRTC {0:?} is already in use.")]
    DuplicateCrtc(crtc::Handle),
    /// The underlying drm device returned an error
    #[error(transparent)]
    Drm(#[from] DrmError),
    /// The underlying [`DrmCompositor`] returned an error
    #[error(transparent)]
    Frame(FrameError<A, B, F>),
    /// The underlying [`DrmCompositor`] returned an error upon rendering a frame
    #[error(transparent)]
    RenderFrame(RenderFrameError<A, B, F, R>),
}

/// Result returned by `DrmOutputManager`'s methods
pub type DrmOutputManagerResult<U, A, F, R> = Result<
    U,
    DrmOutputManagerError<
        <A as Allocator>::Error,
        <<A as Allocator>::Buffer as AsDmabuf>::Error,
        <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Error,
        <R as RendererSuper>::Error,
    >,
>;

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
    /// Create a new [`DrmOutputManager`] from a [`DrmDevice`].
    ///
    /// - `device` the underlying [`DrmDevice`]
    /// - `allocator` used by created [`DrmOutput`]s for primary plane swapchains.
    /// - `exporter` used by created [`DrmOutput`]s to create drm framebuffers
    ///   for the swapchain buffers (and if possible for element buffers)
    ///   for scan-out.
    /// - `gbm` device used by created [`DrmOutput`]s for creating buffers for the
    ///   cursor plane, `None` will disable the cursor plane.
    /// - `color_formats` as tested in order when creating a new [`DrmOutput`]
    /// - `renderer_formats` as reported by the used renderer, used to build the
    ///   intersection between the possible scan-out formats of the
    ///   primary plane of created [`DrmOutput`]s and the renderer
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
            allocator,
            exporter,
            gbm,
            compositor: Default::default(),
            color_formats: color_formats.into_iter().collect(),
            renderer_formats: renderer_formats.into_iter().collect(),
        }
    }

    /// Create a new [`DrmOutput`] for the provided crtc of the underlying device.
    ///
    /// The [`OutputModeSource`] can be created from an [`Output`](crate::output::Output), which will automatically track
    /// the output's mode changes. An [`OutputModeSource::Static`] variant should only be used when
    /// manually updating modes using [`DrmCompositor::set_output_mode_source`].
    ///
    /// This might cause commits on other surfaces to meet the bandwidth
    /// requirements of the output by temporarily disabling additional planes,
    /// forcing composition or falling back to implicit modifiers.
    ///
    /// - `crtc` - the crtc the underlying surface should drive
    /// - `mode` - the mode the underlying surface should be initialized with
    /// - `connectors` - the set of connectors the underlying surface should be initialized with
    /// - `output_mode_source`  used to to determine the size, scale and transform
    /// - `planes` defines which planes the compositor is allowed to use for direct scan-out.
    ///   `None` will result in the compositor to use all planes as specified by [`DrmSurface::planes`][super::DrmSurface::planes]
    /// - `renderer` used for compositing, when commits are necessarily to realize bandwidth constraints
    /// - `render_elements` used for rendering, when commits are necessarily to realize bandwidth constraints
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_output<R, E>(
        &mut self,
        crtc: crtc::Handle,
        mode: control::Mode,
        connectors: &[connector::Handle],
        output_mode_source: impl Into<OutputModeSource> + std::fmt::Debug,
        planes: Option<Planes>,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> DrmOutputManagerResult<DrmOutput<A, F, U, G>, A, F, R>
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        R::TextureId: Texture + 'static,
        R::Error: Send + Sync + 'static,
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
                    let compositor = compositor.get_mut().unwrap();
                    if let Err(err) = render_elements.submit_composited_frame(&mut *compositor, renderer) {
                        if !matches!(err, DrmOutputManagerError::Frame(FrameError::EmptyFrame)) {
                            return Err(err);
                        }
                    }
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
                            let compositor = compositor.get_mut().unwrap();

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
                                    let compositor = compositor.get_mut().unwrap();

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
        // when downstream potentially uses `FrameFlags::DEFAULT` immediately after this.

        let compositor = write_guard.get_mut(&crtc).unwrap();
        let compositor = compositor.get_mut().unwrap();
        render_elements.submit_composited_frame(&mut *compositor, renderer)?;

        Ok(DrmOutput {
            compositor: self.compositor.clone(),
            crtc,
            allocator: self.allocator.clone(),
            renderer_formats: self.renderer_formats.clone(),
        })
    }

    /// Grants exclusive access to all underlying [`DrmCompositor`]s.
    pub fn with_compositors<R>(
        &mut self,
        f: impl FnOnce(&HashMap<crtc::Handle, Mutex<DrmCompositor<A, F, U, G>>>) -> R,
    ) -> R {
        let write_guard = self.compositor.write().unwrap();
        f(&*write_guard)
    }

    /// Tries to apply a new [`Mode`] for the provided `crtc`.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`] or any of the pending [`connector`]s.
    ///
    /// This might cause commits on other surfaces to meet the bandwidth
    /// requirements of the new mode by temporarily disabling additional planes
    /// and forcing composition.
    pub fn use_mode<R, E>(
        &mut self,
        crtc: &crtc::Handle,
        mode: Mode,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> DrmOutputManagerResult<(), A, F, R>
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        R::TextureId: Texture + 'static,
        R::Error: Send + Sync + 'static,
    {
        use_mode_internal(
            &self.compositor,
            crtc,
            mode,
            &self.allocator,
            &self.renderer_formats,
            renderer,
            render_elements,
        )
    }

    /// Tries to restore explicit modifiers on all surfaces.
    ///
    /// Adding new outputs (via [`DrmOutputManager::initialize_output`]) might cause
    /// surfaces to fall back to implicit modifiers to satisfy bandwidth requirements
    /// of the new output. These should generally be avoided, so it is recommended
    /// to call this method after destroying an individual output to try and re-allocate
    /// implicit buffers used for the remaining outputs.
    pub fn try_to_restore_modifiers<R, E>(
        &mut self,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> DrmOutputManagerResult<(), A, F, R>
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        R::TextureId: Texture + 'static,
        R::Error: Send + Sync + 'static,
    {
        let mut write_guard = self.compositor.write().unwrap();

        // check if implicit modifiers are in use
        if write_guard
            .values_mut()
            .any(|c| c.get_mut().unwrap().modifiers() == [DrmModifier::Invalid])
        {
            // if so, first lower the bandwidth by disabling planes on all compositors
            for compositor in write_guard.values_mut() {
                let compositor = compositor.get_mut().unwrap();
                if let Err(err) = render_elements.submit_composited_frame(&mut *compositor, renderer) {
                    if !matches!(err, DrmOutputManagerError::Frame(FrameError::EmptyFrame)) {
                        return Err(err);
                    }
                }
            }

            for compositor in write_guard.values_mut() {
                let compositor = compositor.get_mut().unwrap();
                if compositor.modifiers() != [DrmModifier::Invalid] {
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

    /// Activates a previously paused device.
    ///
    /// Specifying `true` for `disable_connectors` will call [`DrmDevice::reset_state`] if
    /// the device was not active before. Otherwise you need to make sure there are no
    /// conflicting requirements when enabling or creating surfaces or you are prepared
    /// to handle errors caused by those.
    pub fn activate(&mut self, disable_connectors: bool) -> Result<(), DrmError> {
        self.device.activate(disable_connectors)?;

        // We request a write guard here to guarantee unique access
        let mut write_guard = self.compositor.write().unwrap();
        for compositor in write_guard.values_mut() {
            if let Err(err) = compositor.get_mut().unwrap().reset_state() {
                tracing::warn!("Failed to reset drm surface state: {}", err);
            }
        }

        Ok(())
    }
}

/// A handle to an underlying [`DrmCompositor`] handled by an [`DrmOutputManager`].
pub struct DrmOutput<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    compositor: CompositorList<A, F, U, G>,
    crtc: crtc::Handle,
    allocator: A,
    renderer_formats: Vec<DrmFormat>,
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

    /// Marks the current frame as submitted.
    ///
    /// *Note*: Needs to be called, after the vblank event of the matching [`DrmDevice`]
    /// was received after calling [`DrmOutput::queue_frame`] on this surface.
    /// Otherwise the underlying swapchain will run out of buffers eventually.
    pub fn frame_submitted(&self) -> FrameResult<Option<U>, A, F> {
        self.with_compositor(|compositor| compositor.frame_submitted())
    }

    /// Get the format of the underlying swapchain
    pub fn format(&self) -> DrmFourcc {
        self.with_compositor(|compositor| compositor.format())
    }

    /// Render the next frame
    ///
    /// - `elements` for this frame in front-to-back order
    /// - `frame_mode` specifies techniques allowed to realize the frame
    pub fn render_frame<'a, R, E>(
        &mut self,
        renderer: &mut R,
        elements: &'a [E],
        clear_color: impl Into<Color32F>,
        frame_mode: FrameFlags,
    ) -> Result<RenderFrameResult<'a, A::Buffer, F::Framebuffer, E>, RenderFrameErrorType<A, F, R>>
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        R::TextureId: Texture + 'static,
        R::Error: Send + Sync + 'static,
    {
        self.with_compositor(|compositor| {
            compositor.render_frame(renderer, elements, clear_color, frame_mode)
        })
    }

    /// Queues the current frame for scan-out.
    ///
    /// If `render_frame` has not been called prior to this function or returned no damage
    /// this function will return [`FrameError::EmptyFrame`]. Instead of calling `queue_frame` it
    /// is the callers responsibility to re-schedule the frame. A simple strategy for frame
    /// re-scheduling is to queue a one-shot timer that will trigger after approximately one
    /// retrace duration.
    ///
    /// *Note*: It is your responsibility to synchronize rendering if the [`RenderFrameResult`]
    /// returned by the previous [`render_frame`](DrmOutput::render_frame) call returns `true` on [`RenderFrameResult::needs_sync`].
    ///
    /// *Note*: This function needs to be followed up with [`DrmOutput::frame_submitted`]
    /// when a vblank event is received, that denotes successful scan-out of the frame.
    /// Otherwise the underlying swapchain will eventually run out of buffers.
    ///
    /// `user_data` can be used to attach some data to a specific buffer and later retrieved with [`DrmCompositor::frame_submitted`]    
    pub fn queue_frame(&mut self, user_data: U) -> FrameResult<(), A, F> {
        self.with_compositor(|compositor| compositor.queue_frame(user_data))
    }

    /// Commits the current frame for scan-out.
    ///
    /// If `render_frame` has not been called prior to this function or returned no damage
    /// this function will return [`FrameError::EmptyFrame`]. Instead of calling `commit_frame` it
    /// is the callers responsibility to re-schedule the frame. A simple strategy for frame
    /// re-scheduling is to queue a one-shot timer that will trigger after approximately one
    /// retrace duration.
    ///
    /// *Note*: It is your responsibility to synchronize rendering if the [`RenderFrameResult`]
    /// returned by the previous [`render_frame`](DrmOutput::render_frame) call returns `true` on [`RenderFrameResult::needs_sync`].
    ///
    /// *Note*: This function should not be followed up with [`DrmOutput::frame_submitted`]
    /// and will not generate a vblank event on the underlying device.
    pub fn commit_frame(&mut self) -> FrameResult<(), A, F> {
        self.with_compositor(|compositor| compositor.commit_frame())
    }

    /// Tries to apply a new [`Mode`] for this `DrmOutput`.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`] or any of the pending [`connector`]s.
    ///
    /// This might cause commits on other surfaces to meet the bandwidth
    /// requirements of the new mode by temporarily disabling additional planes
    /// and forcing composition.
    pub fn use_mode<R, E>(
        &mut self,
        mode: Mode,
        renderer: &mut R,
        render_elements: &DrmOutputRenderElements<R, E>,
    ) -> DrmOutputManagerResult<(), A, F, R>
    where
        E: RenderElement<R>,
        R: Renderer + Bind<Dmabuf>,
        R::TextureId: Texture + 'static,
        R::Error: Send + Sync + 'static,
    {
        use_mode_internal(
            &self.compositor,
            &self.crtc,
            mode,
            &self.allocator,
            &self.renderer_formats,
            renderer,
            render_elements,
        )
    }
}

impl<A, F, U, G> DrmOutput<A, F, U, G>
where
    A: Allocator,
    F: ExportFramebuffer<<A as Allocator>::Buffer>,
    <F as ExportFramebuffer<<A as Allocator>::Buffer>>::Framebuffer: std::fmt::Debug + 'static,
    G: AsFd + 'static,
{
    /// Returns the underlying [`crtc`] of this surface
    pub fn crtc(&self) -> crtc::Handle {
        self.crtc
    }

    /// Provides exclusive access to the underlying [`DrmCompositor`]
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
    allocator: &A,
    renderer_formats: &[DrmFormat],
    renderer: &mut R,
    render_elements: &DrmOutputRenderElements<R, E>,
) -> DrmOutputManagerResult<(), A, F, R>
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
    R::TextureId: Texture + 'static,
    R::Error: Send + Sync + 'static,
{
    let mut write_guard = compositor.write().unwrap();

    let res = write_guard.get(crtc).unwrap().lock().unwrap().use_mode(mode);

    if let Err(err @ FrameError::DrmError(DrmError::TestFailed(_))) = res.as_ref() {
        tracing::warn!(?crtc, ?err, "failed to set mode, trying to lower bandwidth usage");

        for compositor in write_guard.values_mut() {
            let compositor = compositor.get_mut().unwrap();
            if let Err(err) = render_elements.submit_composited_frame(&mut *compositor, renderer) {
                if !matches!(err, DrmOutputManagerError::Frame(FrameError::EmptyFrame)) {
                    return Err(err);
                }
            }
        }

        let compositor = write_guard.get_mut(crtc).unwrap().get_mut().unwrap();
        match compositor.use_mode(mode) {
            Ok(_) => {
                if let Err(err) = render_elements.submit_composited_frame(&mut *compositor, renderer) {
                    if !matches!(err, DrmOutputManagerError::Frame(FrameError::EmptyFrame)) {
                        return Err(err);
                    }
                }
            }
            Err(err @ FrameError::DrmError(DrmError::TestFailed(_))) => {
                tracing::warn!(?crtc, ?err, "failed to set mode, trying implicit modifiers");

                for compositor in write_guard.values_mut() {
                    let compositor = compositor.get_mut().unwrap();

                    let current_format = compositor.format();
                    if let Err(err) =
                        compositor.set_format(allocator.clone(), current_format, [DrmModifier::Invalid])
                    {
                        tracing::warn!(?err, "failed to set new format");
                        continue;
                    }

                    render_elements.submit_composited_frame(&mut *compositor, renderer)?;
                }

                let compositor = write_guard.get_mut(crtc).unwrap().get_mut().unwrap();
                match compositor.use_mode(mode) {
                    Ok(_) => render_elements.submit_composited_frame(&mut *compositor, renderer)?,
                    Err(err) => {
                        // try to reset format

                        for compositor in write_guard.values_mut() {
                            let compositor = compositor.get_mut().unwrap();

                            let current_format = compositor.format();
                            if let Err(err) = compositor.set_format(
                                allocator.clone(),
                                current_format,
                                renderer_formats
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
            Err(err) => return Err(DrmOutputManagerError::Frame(err)),
        };
    }

    Ok(())
}

/// Set of render elements for a set of outputs managed by an [`DrmOutputManager`].
///
/// A few methods of the [`DrmOutputManager`] and [`DrmOutput`] might need to do
/// commits to multiple surfaces to satisfy bandwidth constraints. To not render
/// a black screen this struct can be populated with screen contents to be used
/// when such an operation is required. Outputs not provided via
/// [`DrmOutputRenderElements::add_output`] will fallback to black.
#[derive(Debug)]
pub struct DrmOutputRenderElements<R, E>
where
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    R::TextureId: Texture + 'static,
    R::Error: Send + Sync + 'static,
{
    render_elements: HashMap<crtc::Handle, (Vec<E>, Color32F)>,
    _renderer: PhantomData<R>,
}

impl<R, E> DrmOutputRenderElements<R, E>
where
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    R::TextureId: Texture + 'static,
    R::Error: Send + Sync + 'static,
{
    /// Construct a new empty set of render elements
    pub fn new() -> Self {
        DrmOutputRenderElements {
            render_elements: HashMap::new(),
            _renderer: PhantomData,
        }
    }

    /// Construct a new empty set of render elements with pre-allocated capacity
    /// for a number of crtcs.
    pub fn with_capacity(cap: usize) -> Self {
        DrmOutputRenderElements {
            render_elements: HashMap::with_capacity(cap),
            _renderer: PhantomData,
        }
    }
}

impl<R, E> Default for DrmOutputRenderElements<R, E>
where
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    R::TextureId: Texture + 'static,
    R::Error: Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<R, E> DrmOutputRenderElements<R, E>
where
    E: RenderElement<R>,
    R: Renderer + Bind<Dmabuf>,
    R::TextureId: Texture + 'static,
    R::Error: Send + Sync + 'static,
{
    /// Adds elements to be used when rendering for a given `crtc`.
    ///
    /// Outputs not provided via this will fallback to black when
    /// this struct is passed to any consuming function.
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
    ) -> DrmOutputManagerResult<(), A, F, R>
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
        let frame_result = compositor
            .render_frame(renderer, elements, *clear_color, FrameFlags::empty())
            .map_err(DrmOutputManagerError::RenderFrame)?;
        if frame_result.needs_sync() {
            if let PrimaryPlaneElement::Swapchain(primary_swapchain_element) = frame_result.primary_element {
                let _ = primary_swapchain_element.sync.wait();
            }
        }
        compositor.commit_frame().map_err(DrmOutputManagerError::Frame)?;
        Ok(())
    }
}
