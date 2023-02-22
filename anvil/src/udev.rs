#[cfg(feature = "xwayland")]
use std::ffi::OsString;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::hash_map::{Entry, HashMap},
    convert::TryInto,
    os::unix::io::FromRawFd,
    path::PathBuf,
    rc::Rc,
    sync::{atomic::Ordering, Mutex},
    time::Duration,
};

use crate::{
    drawing::*,
    render::*,
    shell::WindowElement,
    state::{post_repaint, take_presentation_feedback, AnvilState, Backend, CalloopData},
};
#[cfg(feature = "debug")]
use smithay::backend::renderer::ImportMem;
#[cfg(feature = "egl")]
use smithay::{
    backend::renderer::{ImportDma, ImportEgl},
    delegate_dmabuf,
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportError},
};
use smithay::{
    backend::{
        allocator::{
            dmabuf::{AnyError, Dmabuf, DmabufAllocator},
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            vulkan::{ImageUsageFlags, VulkanAllocator},
            Allocator,
        },
        drm::{
            compositor::DrmCompositor, DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventMetadata, DrmNode,
            DrmSurface, GbmBufferedSurface, NodeType,
        },
        egl::{EGLContext, EGLDevice, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            damage::{DamageTrackedRenderer, DamageTrackedRendererError},
            element::{texture::TextureBuffer, AsRenderElements, RenderElement, RenderElementStates},
            gles2::{Gles2Renderbuffer, Gles2Renderer},
            multigpu::{gbm::GbmGlesBackend, GpuManager, MultiRenderer, MultiTexture},
            Bind, DebugFlags, ExportMem, Offscreen, Renderer,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
        vulkan::{version::Version, Instance, PhysicalDevice},
        SwapBuffersError,
    },
    desktop::{
        space::{Space, SurfaceTree},
        utils::OutputPresentationFeedback,
    },
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        ash::vk::ExtPhysicalDeviceDrmFn,
        calloop::{
            timer::{TimeoutAction, Timer},
            Dispatcher, EventLoop, LoopHandle, RegistrationToken,
        },
        drm::{
            self,
            control::{
                connector::{Info as ConnectorInfo, State as ConnectorState},
                crtc,
                encoder::Info as EncoderInfo,
                Device as ControlDevice,
            },
            Device,
        },
        input::Libinput,
        nix::{fcntl::OFlag, sys::stat::dev_t},
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{backend::GlobalId, protocol::wl_surface, Display, DisplayHandle},
    },
    utils::{Clock, DeviceFd, IsAlive, Logical, Monotonic, Point, Scale, Transform},
    wayland::{
        compositor,
        input_method::{InputMethodHandle, InputMethodSeat},
    },
};
use tracing::{debug, error, info, trace, warn};

type UdevRenderer<'a, 'b> =
    MultiRenderer<'a, 'a, 'b, GbmGlesBackend<Gles2Renderer>, GbmGlesBackend<Gles2Renderer>>;

#[derive(Debug, PartialEq)]
struct UdevOutputId {
    device_id: DrmNode,
    crtc: crtc::Handle,
}

pub struct UdevData {
    pub session: LibSeatSession,
    dh: DisplayHandle,
    #[cfg(feature = "egl")]
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    primary_gpu: DrmNode,
    allocator: Option<Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>>>,
    gpus: GpuManager<GbmGlesBackend<Gles2Renderer>>,
    backends: HashMap<DrmNode, BackendData>,
    pointer_images: Vec<(xcursor::parser::Image, TextureBuffer<MultiTexture>)>,
    pointer_element: PointerElement<MultiTexture>,
    #[cfg(feature = "debug")]
    fps_texture: Option<MultiTexture>,
    pointer_image: crate::cursor::Cursor,
    debug_flags: DebugFlags,
}

impl UdevData {
    pub fn set_debug_flags(&mut self, flags: DebugFlags) {
        if self.debug_flags != flags {
            self.debug_flags = flags;

            for (_, backend) in self.backends.iter_mut() {
                let surfaces = backend.surfaces.borrow();
                for (_, surface) in surfaces.iter() {
                    surface.borrow_mut().compositor.set_debug_flags(flags);
                }
            }
        }
    }

    pub fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }
}

#[cfg(feature = "egl")]
impl DmabufHandler for AnvilState<UdevData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf) -> Result<(), ImportError> {
        self.backend_data
            .gpus
            .single_renderer(&self.backend_data.primary_gpu)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .map(|_| ())
            .map_err(|_| ImportError::Failed)
    }
}
#[cfg(feature = "egl")]
delegate_dmabuf!(AnvilState<UdevData>);

impl Backend for UdevData {
    const HAS_RELATIVE_MOTION: bool = true;

    fn seat_name(&self) -> String {
        self.session.seat()
    }

    fn reset_buffers(&mut self, output: &Output) {
        if let Some(id) = output.user_data().get::<UdevOutputId>() {
            if let Some(gpu) = self.backends.get(&id.device_id) {
                let surfaces = gpu.surfaces.borrow();
                if let Some(surface) = surfaces.get(&id.crtc) {
                    surface.borrow_mut().compositor.reset_buffers();
                }
            }
        }
    }

    fn early_import(&mut self, surface: &wl_surface::WlSurface) {
        if let Err(err) = self
            .gpus
            .early_import(Some(self.primary_gpu), self.primary_gpu, surface)
        {
            warn!("Early buffer import failed: {}", err);
        }
    }
}

pub fn run_udev() {
    let mut event_loop = EventLoop::try_new().unwrap();
    let mut display = Display::new().unwrap();

    /*
     * Initialize session
     */
    let (session, notifier) = match LibSeatSession::new() {
        Ok(ret) => ret,
        Err(err) => {
            error!("Could not initialize a session: {}", err);
            return;
        }
    };

    /*
     * Initialize the compositor
     */
    let primary_gpu = if let Ok(var) = std::env::var("ANVIL_DRM_DEVICE") {
        DrmNode::from_path(var).expect("Invalid drm device path")
    } else {
        primary_gpu(&session.seat())
            .unwrap()
            .and_then(|x| DrmNode::from_path(x).ok()?.node_with_type(NodeType::Render)?.ok())
            .unwrap_or_else(|| {
                all_gpus(&session.seat())
                    .unwrap()
                    .into_iter()
                    .find_map(|x| DrmNode::from_path(x).ok())
                    .expect("No GPU!")
            })
    };
    info!("Using {} as primary gpu.", primary_gpu);

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let gpus = GpuManager::new(GbmGlesBackend::default()).unwrap();

    let data = UdevData {
        dh: display.handle(),
        #[cfg(feature = "egl")]
        dmabuf_state: None,
        session,
        primary_gpu,
        gpus,
        allocator: None,
        backends: HashMap::new(),
        pointer_image: crate::cursor::Cursor::load(),
        pointer_images: Vec::new(),
        pointer_element: PointerElement::default(),
        #[cfg(feature = "debug")]
        fps_texture: None,
        debug_flags: DebugFlags::empty(),
    };
    let mut state = AnvilState::init(&mut display, event_loop.handle(), data, true);

    /*
     * Initialize the udev backend
     */
    let udev_backend = match UdevBackend::new(&state.seat_name) {
        Ok(ret) => ret,
        Err(err) => {
            error!(error = ?err, "Failed to initialize udev backend");
            return;
        }
    };

    /*
     * Initialize libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.backend_data.session.clone().into(),
    );
    libinput_context.udev_assign_seat(&state.seat_name).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    /*
     * Bind all our objects that get driven by the event loop
     */
    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, data| {
            let dh = data.state.backend_data.dh.clone();
            data.state.process_input_event(&dh, event)
        })
        .unwrap();
    let handle = event_loop.handle();
    event_loop
        .handle()
        .insert_source(notifier, move |event, &mut (), data| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                for backend in data.state.backend_data.backends.values() {
                    backend.event_dispatcher.as_source_ref().pause();
                }
            }
            SessionEvent::ActivateSession => {
                if let Err(err) = libinput_context.resume() {
                    error!("Failed to resume libinput context: {:?}", err);
                }
                for (node, backend) in data
                    .state
                    .backend_data
                    .backends
                    .iter()
                    .map(|(handle, backend)| (*handle, backend))
                {
                    backend.event_dispatcher.as_source_ref().activate();
                    let surfaces = backend.surfaces.borrow();
                    for surface in surfaces.values() {
                        if let Err(err) = surface.borrow().compositor.surface().reset_state() {
                            warn!("Failed to reset drm surface state: {}", err);
                        }
                    }
                    handle.insert_idle(move |data| data.state.render(node, None));
                }
            }
        })
        .unwrap();
    for (dev, path) in udev_backend.device_list() {
        state.device_added(&mut display, dev, path.into())
    }

    let skip_vulkan = std::env::var("ANVIL_NO_VULKAN")
        .map(|x| {
            x == "1" || x.to_lowercase() == "true" || x.to_lowercase() == "yes" || x.to_lowercase() == "y"
        })
        .unwrap_or(false);

    if !skip_vulkan {
        if let Ok(instance) = Instance::new(Version::VERSION_1_2, None) {
            if let Some(physical_device) = PhysicalDevice::enumerate(&instance).ok().and_then(|devices| {
                devices
                    .filter(|phd| phd.has_device_extension(ExtPhysicalDeviceDrmFn::name()))
                    .find(|phd| {
                        phd.primary_node().unwrap() == Some(primary_gpu)
                            || phd.render_node().unwrap() == Some(primary_gpu)
                    })
            }) {
                match VulkanAllocator::new(
                    &physical_device,
                    ImageUsageFlags::COLOR_ATTACHMENT | ImageUsageFlags::SAMPLED,
                ) {
                    Ok(allocator) => {
                        state.backend_data.allocator = Some(Box::new(DmabufAllocator(allocator))
                            as Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>>);
                    }
                    Err(err) => {
                        warn!("Failed to create vulkan allocator: {}", err);
                    }
                }
            }
        }
    }

    if state.backend_data.allocator.is_none() {
        info!("No vulkan allocator found, using GBM.");
        let gbm = state
            .backend_data
            .backends
            .get(&primary_gpu)
            // If the primary_gpu failed to initialize, we likely have a kmsro device
            .or_else(|| state.backend_data.backends.values().next())
            // Don't fail, if there is no allocator. There is a chance, that this a single gpu system and we don't need one.
            .map(|backend| backend.gbm.clone());
        state.backend_data.allocator = gbm.map(|gbm| {
            Box::new(DmabufAllocator(GbmAllocator::new(gbm, GbmBufferFlags::RENDERING))) as Box<_>
        });
    }

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    #[cfg(any(feature = "egl", feature = "debug"))]
    let mut renderer = state.backend_data.gpus.single_renderer(&primary_gpu).unwrap();

    #[cfg(feature = "debug")]
    {
        let fps_image =
            image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
                .decode()
                .unwrap();
        let fps_texture = renderer
            .import_memory(
                &fps_image.to_rgba8(),
                (fps_image.width() as i32, fps_image.height() as i32).into(),
                false,
            )
            .expect("Unable to upload FPS texture");

        for backend in state.backend_data.backends.values_mut() {
            for surface in backend.surfaces.borrow_mut().values_mut() {
                surface.borrow_mut().fps_element = Some(FpsElement::new(fps_texture.clone()));
            }
        }
        state.backend_data.fps_texture = Some(fps_texture);
    }

    // init dmabuf support with format list from our primary gpu
    // TODO: This does not necessarily depend on egl, but mesa makes no use of it without wl_drm right now
    #[cfg(feature = "egl")]
    {
        info!(?primary_gpu, "Trying to initialize EGL Hardware Acceleration",);

        if renderer.bind_wl_display(&display.handle()).is_ok() {
            info!("EGL hardware-acceleration enabled");
            let dmabuf_formats = renderer.dmabuf_formats().cloned().collect::<Vec<_>>();
            let mut dmabuf_state = DmabufState::new();
            let global =
                dmabuf_state.create_global::<AnvilState<UdevData>>(&display.handle(), dmabuf_formats);
            state.backend_data.dmabuf_state = Some((dmabuf_state, global));
        }
    };

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, data| match event {
            UdevEvent::Added { device_id, path } => {
                data.state.device_added(&mut data.display, device_id, path)
            }
            UdevEvent::Changed { device_id } => data.state.device_changed(&mut data.display, device_id),
            UdevEvent::Removed { device_id } => data.state.device_removed(device_id),
        })
        .unwrap();

    /*
     * Start XWayland if supported
     */
    #[cfg(feature = "xwayland")]
    if let Err(e) = state.xwayland.start(
        state.handle.clone(),
        None,
        std::iter::empty::<(OsString, OsString)>(),
        |_| {},
    ) {
        error!("Failed to start XWayland: {}", e);
    }

    /*
     * And run our loop
     */

    while state.running.load(Ordering::SeqCst) {
        let mut calloop_data = CalloopData { state, display };
        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut calloop_data);
        CalloopData { state, display } = calloop_data;

        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.refresh();
            state.popups.cleanup();
            display.flush_clients().unwrap();
        }
    }
}

pub type RenderSurface = GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, Option<OutputPresentationFeedback>>;

pub type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmDevice<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

enum SurfaceComposition {
    Surface {
        surface: RenderSurface,
        dtr: DamageTrackedRenderer,
        debug_flags: DebugFlags,
    },
    Compositor(GbmDrmCompositor),
}

impl SurfaceComposition {
    fn frame_submitted(&mut self) -> Result<Option<Option<OutputPresentationFeedback>>, SwapBuffersError> {
        match self {
            SurfaceComposition::Compositor(c) => c.frame_submitted().map_err(Into::<SwapBuffersError>::into),
            SurfaceComposition::Surface { surface, .. } => {
                surface.frame_submitted().map_err(Into::<SwapBuffersError>::into)
            }
        }
    }

    fn format(&self) -> smithay::reexports::gbm::Format {
        match self {
            SurfaceComposition::Compositor(c) => c.format(),
            SurfaceComposition::Surface { surface, .. } => surface.format(),
        }
    }

    fn surface(&self) -> &DrmSurface {
        match self {
            SurfaceComposition::Compositor(c) => c.surface(),
            SurfaceComposition::Surface { surface, .. } => surface.surface(),
        }
    }

    fn reset_buffers(&mut self) {
        match self {
            SurfaceComposition::Compositor(c) => c.reset_buffers(),
            SurfaceComposition::Surface { surface, .. } => surface.reset_buffers(),
        }
    }

    fn queue_frame(&mut self, user_data: Option<OutputPresentationFeedback>) -> Result<(), SwapBuffersError> {
        match self {
            SurfaceComposition::Surface { surface, .. } => surface
                .queue_buffer(None, user_data)
                .map_err(Into::<SwapBuffersError>::into),
            SurfaceComposition::Compositor(c) => {
                c.queue_frame(user_data).map_err(Into::<SwapBuffersError>::into)
            }
        }
    }

    fn render_frame<'a, R, E, Target>(
        &mut self,
        renderer: &mut R,
        elements: &'a [E],
        clear_color: [f32; 4],
    ) -> Result<(bool, RenderElementStates), SwapBuffersError>
    where
        R: Renderer + Bind<Dmabuf> + Bind<Target> + Offscreen<Target> + ExportMem,
        <R as Renderer>::TextureId: 'static,
        <R as Renderer>::Error: Into<SwapBuffersError>,
        E: RenderElement<R>,
    {
        match self {
            SurfaceComposition::Surface {
                surface,
                dtr,
                debug_flags,
            } => {
                let (dmabuf, age) = surface.next_buffer().map_err(Into::<SwapBuffersError>::into)?;
                renderer.bind(dmabuf).map_err(Into::<SwapBuffersError>::into)?;
                let current_debug_flags = renderer.debug_flags();
                renderer.set_debug_flags(*debug_flags);
                let res = dtr
                    .render_output(renderer, age.into(), elements, clear_color)
                    .map(|(damage, states)| (damage.is_some(), states))
                    .map_err(|err| match err {
                        DamageTrackedRendererError::Rendering(err) => err.into(),
                        _ => unreachable!(),
                    });
                renderer.set_debug_flags(current_debug_flags);
                res
            }
            SurfaceComposition::Compositor(compositor) => compositor
                .render_frame(renderer, elements, clear_color)
                .map(|render_frame_result| (render_frame_result.damage.is_some(), render_frame_result.states))
                .map_err(|err| match err {
                    smithay::backend::drm::compositor::RenderFrameError::PrepareFrame(err) => err.into(),
                    smithay::backend::drm::compositor::RenderFrameError::RenderFrame(
                        DamageTrackedRendererError::Rendering(err),
                    ) => err.into(),
                    _ => unreachable!(),
                }),
        }
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        match self {
            SurfaceComposition::Surface {
                surface, debug_flags, ..
            } => {
                *debug_flags = flags;
                surface.reset_buffers();
            }
            SurfaceComposition::Compositor(c) => c.set_debug_flags(flags),
        }
    }
}

struct SurfaceData {
    dh: DisplayHandle,
    device_id: DrmNode,
    render_node: DrmNode,
    global: Option<GlobalId>,
    compositor: SurfaceComposition,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
    #[cfg(feature = "debug")]
    fps_element: Option<FpsElement<MultiTexture>>,
}

impl Drop for SurfaceData {
    fn drop(&mut self) {
        if let Some(global) = self.global.take() {
            self.dh.remove_global::<AnvilState<UdevData>>(global);
        }
    }
}

struct BackendData {
    surfaces: Rc<RefCell<HashMap<crtc::Handle, Rc<RefCell<SurfaceData>>>>>,
    gbm: GbmDevice<DrmDeviceFd>,
    render_node: DrmNode,
    registration_token: RegistrationToken,
    event_dispatcher: Dispatcher<'static, DrmDevice, CalloopData<UdevData>>,
}

#[allow(clippy::too_many_arguments)]
fn scan_connectors(
    device_id: DrmNode,
    device: &DrmDevice,
    gbm: &GbmDevice<DrmDeviceFd>,
    display: &mut Display<AnvilState<UdevData>>,
    space: &mut Space<WindowElement>,
    #[cfg(feature = "debug")] fps_texture: Option<&MultiTexture>,
    debug_flags: DebugFlags,
) -> HashMap<crtc::Handle, Rc<RefCell<SurfaceData>>> {
    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = device.resource_handles().unwrap();

    // Find all connected output ports.
    let connector_infos: Vec<ConnectorInfo> = res_handles
        .connectors()
        .iter()
        .map(|conn| device.get_connector(*conn, true).unwrap())
        .filter(|conn| conn.state() == ConnectorState::Connected)
        .inspect(|conn| info!("Connected: {:?}", conn.interface()))
        .collect();

    let mut backends = HashMap::new();

    let (render_node, formats) = {
        let display = EGLDisplay::new(gbm.clone()).unwrap();
        let node = match EGLDevice::device_for_display(&display)
            .ok()
            .and_then(|x| x.try_get_render_node().ok().flatten())
        {
            Some(node) => node,
            None => return HashMap::new(),
        };
        let context = EGLContext::new(&display).unwrap();
        (node, context.dmabuf_render_formats().clone())
    };

    // very naive way of finding good crtc/encoder/connector combinations. This problem is np-complete
    for connector_info in connector_infos {
        let encoder_infos = connector_info
            .encoders()
            .iter()
            .flat_map(|encoder_handle| device.get_encoder(*encoder_handle))
            .collect::<Vec<EncoderInfo>>();

        let crtcs = encoder_infos
            .iter()
            .flat_map(|encoder_info| res_handles.filter_crtcs(encoder_info.possible_crtcs()));

        for crtc in crtcs {
            // Skip CRTCs used by previous connectors.
            let entry = match backends.entry(crtc) {
                Entry::Vacant(entry) => entry,
                Entry::Occupied(_) => continue,
            };

            info!(
                ?crtc,
                "Trying to setup connector {:?}-{}",
                connector_info.interface(),
                connector_info.interface_id(),
            );

            let mode = connector_info.modes()[0];
            let surface = match device.create_surface(crtc, mode, &[connector_info.handle()]) {
                Ok(surface) => surface,
                Err(err) => {
                    warn!("Failed to create drm surface: {}", err);
                    continue;
                }
            };

            let size = mode.size();
            let mode = Mode {
                size: (size.0 as i32, size.1 as i32).into(),
                refresh: mode.vrefresh() as i32 * 1000,
            };

            let interface_short_name = match connector_info.interface() {
                drm::control::connector::Interface::DVII => Cow::Borrowed("DVI-I"),
                drm::control::connector::Interface::DVID => Cow::Borrowed("DVI-D"),
                drm::control::connector::Interface::DVIA => Cow::Borrowed("DVI-A"),
                drm::control::connector::Interface::SVideo => Cow::Borrowed("S-VIDEO"),
                drm::control::connector::Interface::DisplayPort => Cow::Borrowed("DP"),
                drm::control::connector::Interface::HDMIA => Cow::Borrowed("HDMI-A"),
                drm::control::connector::Interface::HDMIB => Cow::Borrowed("HDMI-B"),
                drm::control::connector::Interface::EmbeddedDisplayPort => Cow::Borrowed("eDP"),
                other => Cow::Owned(format!("{:?}", other)),
            };

            let output_name = format!("{}-{}", interface_short_name, connector_info.interface_id());

            let (phys_w, phys_h) = connector_info.size().unwrap_or((0, 0));
            let output = Output::new(
                output_name,
                PhysicalProperties {
                    size: (phys_w as i32, phys_h as i32).into(),
                    subpixel: Subpixel::Unknown,
                    make: "Smithay".into(),
                    model: "Generic DRM".into(),
                },
            );
            let global = output.create_global::<AnvilState<UdevData>>(&display.handle());
            let position = (
                space
                    .outputs()
                    .fold(0, |acc, o| acc + space.output_geometry(o).unwrap().size.w),
                0,
            )
                .into();
            output.change_current_state(Some(mode), None, None, Some(position));
            output.set_preferred(mode);
            space.map_output(&output, position);

            output
                .user_data()
                .insert_if_missing(|| UdevOutputId { crtc, device_id });

            #[cfg(feature = "debug")]
            let fps_element = fps_texture.cloned().map(FpsElement::new);

            let allocator =
                GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);

            let compositor = if std::env::var("ANVIL_DISABLE_DRM_COMPOSITOR").is_ok() {
                let gbm_surface = match GbmBufferedSurface::new(surface, allocator, formats.clone()) {
                    Ok(renderer) => renderer,
                    Err(err) => {
                        warn!("Failed to create rendering surface: {}", err);
                        continue;
                    }
                };
                SurfaceComposition::Surface {
                    surface: gbm_surface,
                    dtr: DamageTrackedRenderer::from_output(&output),
                    debug_flags,
                }
            } else {
                let driver = match device.get_driver() {
                    Ok(driver) => driver,
                    Err(err) => {
                        warn!("Failed to query drm driver: {}", err);
                        continue;
                    }
                };

                let mut planes = match surface.planes() {
                    Ok(planes) => planes,
                    Err(err) => {
                        warn!("Failed to query surface planes: {}", err);
                        continue;
                    }
                };

                // Using an overlay plane on a nvidia card breaks
                if driver.name().to_string_lossy().to_lowercase().contains("nvidia")
                    || driver
                        .description()
                        .to_string_lossy()
                        .to_lowercase()
                        .contains("nvidia")
                {
                    planes.overlay = vec![];
                }

                let mut compositor = match DrmCompositor::new(
                    &output,
                    surface,
                    Some(planes),
                    allocator,
                    gbm.clone(),
                    formats.clone(),
                    device.cursor_size(),
                    Some(gbm.clone()),
                ) {
                    Ok(compositor) => compositor,
                    Err(err) => {
                        warn!("Failed to create drm compositor: {}", err);
                        continue;
                    }
                };
                compositor.set_debug_flags(debug_flags);
                SurfaceComposition::Compositor(compositor)
            };

            entry.insert(Rc::new(RefCell::new(SurfaceData {
                dh: display.handle(),
                device_id,
                render_node,
                global: Some(global),
                compositor,
                #[cfg(feature = "debug")]
                fps: fps_ticker::Fps::default(),
                #[cfg(feature = "debug")]
                fps_element,
            })));

            break;
        }
    }

    backends
}

impl AnvilState<UdevData> {
    fn device_added(&mut self, display: &mut Display<Self>, device_id: dev_t, path: PathBuf) {
        // Try to open the device
        let open_flags = OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOCTTY | OFlag::O_NONBLOCK;
        let device_fd = self.backend_data.session.open(&path, open_flags).ok();
        let devices = device_fd
            .map(|fd| DrmDeviceFd::new(unsafe { DeviceFd::from_raw_fd(fd) }))
            .map(|fd| (DrmDevice::new(fd.clone(), true), GbmDevice::new(fd)));

        // Report device open failures.
        let (device, gbm) = match devices {
            Some((Ok(drm), Ok(gbm))) => (drm, gbm),
            Some((Err(err), _)) => {
                warn!("Skipping device {:?}, because of drm error: {}", device_id, err);
                return;
            }
            Some((_, Err(err))) => {
                // TODO try DumbBuffer allocator in this case
                warn!("Skipping device {:?}, because of gbm error: {}", device_id, err);
                return;
            }
            None => return,
        };

        let node = match DrmNode::from_dev_id(device_id) {
            Ok(node) => node,
            Err(err) => {
                warn!("Failed to access drm node for {}: {}", device_id, err);
                return;
            }
        };

        let backends = Rc::new(RefCell::new(scan_connectors(
            node,
            &device,
            &gbm,
            display,
            &mut self.space,
            #[cfg(feature = "debug")]
            self.backend_data.fps_texture.as_ref(),
            self.backend_data.debug_flags,
        )));

        let event_dispatcher =
            Dispatcher::new(
                device,
                move |event, metadata, data: &mut CalloopData<_>| match event {
                    DrmEvent::VBlank(crtc) => {
                        data.state.frame_finish(node, crtc, metadata);
                    }
                    DrmEvent::Error(error) => {
                        error!("{:?}", error);
                    }
                },
            );
        let registration_token = self.handle.register_dispatcher(event_dispatcher.clone()).unwrap();

        let render_node = {
            let display = EGLDisplay::new(gbm.clone()).unwrap();
            match EGLDevice::device_for_display(&display)
                .ok()
                .and_then(|x| x.try_get_render_node().ok().flatten())
            {
                Some(node) => node,
                None => node,
            }
        };
        if let Err(err) = self.backend_data.gpus.as_mut().add_node(render_node, gbm.clone()) {
            warn!("Failed to create renderer for GBM device {}: {}", device_id, err);
            return;
        };
        for backend in backends.borrow_mut().values() {
            // render first frame
            trace!("Scheduling frame");
            schedule_initial_render(&mut self.backend_data.gpus, backend.clone(), &self.handle);
        }

        self.backend_data.backends.insert(
            node,
            BackendData {
                registration_token,
                event_dispatcher,
                render_node,
                surfaces: backends,
                gbm,
            },
        );
    }

    fn device_changed(&mut self, display: &mut Display<Self>, device: dev_t) {
        let node = match DrmNode::from_dev_id(device).ok() {
            Some(node) => node,
            None => return, // we already logged a warning on device_added
        };

        //quick and dirty, just re-init all backends
        if let Some(ref mut backend_data) = self.backend_data.backends.get_mut(&node) {
            let loop_handle = self.handle.clone();

            // scan_connectors will recreate the outputs (and sadly also reset the scales)
            for output in self
                .space
                .outputs()
                .filter(|o| {
                    o.user_data()
                        .get::<UdevOutputId>()
                        .map(|id| id.device_id == node)
                        .unwrap_or(false)
                })
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
            {
                self.space.unmap_output(&output);
            }

            let source = backend_data.event_dispatcher.as_source_mut();
            let mut backends = backend_data.surfaces.borrow_mut();
            *backends = scan_connectors(
                node,
                &source,
                &backend_data.gbm,
                display,
                &mut self.space,
                #[cfg(feature = "debug")]
                self.backend_data.fps_texture.as_ref(),
                self.backend_data.debug_flags,
            );

            // fixup window coordinates
            crate::shell::fixup_positions(&mut self.space);

            for surface in backends.values() {
                // render first frame
                schedule_initial_render(&mut self.backend_data.gpus, surface.clone(), &loop_handle);
            }
        }
    }

    fn device_removed(&mut self, device: dev_t) {
        let node = match DrmNode::from_dev_id(device).ok() {
            Some(node) => node,
            None => return, // we already logged a warning on device_added
        };
        // drop the backends on this side
        if let Some(backend_data) = self.backend_data.backends.remove(&node) {
            self.backend_data
                .gpus
                .as_mut()
                .remove_node(&backend_data.render_node);

            // drop surfaces
            backend_data.surfaces.borrow_mut().clear();
            debug!("Surfaces dropped");

            for output in self
                .space
                .outputs()
                .filter(|o| {
                    o.user_data()
                        .get::<UdevOutputId>()
                        .map(|id| id.device_id == node)
                        .unwrap_or(false)
                })
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
            {
                self.space.unmap_output(&output);
            }
            crate::shell::fixup_positions(&mut self.space);

            self.handle.remove(backend_data.registration_token);
            let _device = backend_data.event_dispatcher.into_source_inner();

            debug!("Dropping device");
        }
    }

    fn frame_finish(&mut self, dev_id: DrmNode, crtc: crtc::Handle, metadata: &mut Option<DrmEventMetadata>) {
        let device_backend = match self.backend_data.backends.get_mut(&dev_id) {
            Some(backend) => backend,
            None => {
                error!("Trying to finish frame on non-existent backend {}", dev_id);
                return;
            }
        };

        let surfaces = device_backend.surfaces.borrow();
        let surface = match surfaces.get(&crtc) {
            Some(surface) => surface,
            None => {
                error!("Trying to finish frame on non-existent crtc {:?}", crtc);
                return;
            }
        };

        let mut surface = surface.borrow_mut();

        let output = if let Some(output) = self.space.outputs().find(|o| {
            o.user_data().get::<UdevOutputId>()
                == Some(&UdevOutputId {
                    device_id: surface.device_id,
                    crtc,
                })
        }) {
            output.clone()
        } else {
            // somehow we got called with an invalid output
            return;
        };

        let schedule_render = match surface
            .compositor
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into)
        {
            Ok(user_data) => {
                if let Some(mut feedback) = user_data.flatten() {
                    let tp = metadata.as_ref().and_then(|metadata| match metadata.time {
                        smithay::backend::drm::DrmEventTime::Monotonic(tp) => Some(tp),
                        smithay::backend::drm::DrmEventTime::Realtime(_) => None,
                    });
                    let seq = metadata.as_ref().map(|metadata| metadata.sequence).unwrap_or(0);

                    let (clock, flags) = if let Some(tp) = tp {
                        (
                            tp.into(),
                            wp_presentation_feedback::Kind::Vsync
                                | wp_presentation_feedback::Kind::HwClock
                                | wp_presentation_feedback::Kind::HwCompletion,
                        )
                    } else {
                        (self.clock.now(), wp_presentation_feedback::Kind::Vsync)
                    };

                    feedback.presented(
                        clock,
                        output
                            .current_mode()
                            .map(|mode| mode.refresh as u32)
                            .unwrap_or_default(),
                        seq as u64,
                        flags,
                    );
                }

                true
            }
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => true,
                    SwapBuffersError::TemporaryFailure(err) => matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(&DrmError::DeviceInactive)
                            | Some(&DrmError::Access {
                                source: drm::SystemError::PermissionDenied,
                                ..
                            })
                    ),
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
                }
            }
        };

        if schedule_render {
            let output_refresh = match output.current_mode() {
                Some(mode) => mode.refresh,
                None => return,
            };
            // What are we trying to solve by introducing a delay here:
            //
            // Basically it is all about latency of client provided buffers.
            // A client driven by frame callbacks will wait for a frame callback
            // to repaint and submit a new buffer. As we send frame callbacks
            // as part of the repaint in the compositor the latency would always
            // be approx. 2 frames. By introducing a delay before we repaint in
            // the compositor we can reduce the latency to approx. 1 frame + the
            // remaining duration from the repaint to the next VBlank.
            //
            // With the delay it is also possible to further reduce latency if
            // the client is driven by presentation feedback. As the presentation
            // feedback is directly sent after a VBlank the client can submit a
            // new buffer during the repaint delay that can hit the very next
            // VBlank, thus reducing the potential latency to below one frame.
            //
            // Choosing a good delay is a topic on its own so we just implement
            // a simple strategy here. We just split the duration between two
            // VBlanks into two steps, one for the client repaint and one for the
            // compositor repaint. Theoretically the repaint in the compositor should
            // be faster so we give the client a bit more time to repaint. On a typical
            // modern system the repaint in the compositor should not take more than 2ms
            // so this should be safe for refresh rates up to at least 120 Hz. For 120 Hz
            // this results in approx. 3.33ms time for repainting in the compositor.
            // A too big delay could result in missing the next VBlank in the compositor.
            //
            // A more complete solution could work on a sliding window analyzing past repaints
            // and do some prediction for the next repaint.
            let repaint_delay =
                Duration::from_millis(((1_000_000f32 / output_refresh as f32) * 0.6f32) as u64);

            let timer = if self.backend_data.primary_gpu != surface.render_node {
                // However, if we need to do a copy, that might not be enough.
                // (And without actual comparision to previous frames we cannot really know.)
                // So lets ignore that in those cases to avoid thrashing performance.
                trace!("scheduling repaint timer immediately on {:?}", crtc);
                Timer::immediate()
            } else {
                trace!(
                    "scheduling repaint timer with delay {:?} on {:?}",
                    repaint_delay,
                    crtc
                );
                Timer::from_duration(repaint_delay)
            };

            self.handle
                .insert_source(timer, move |_, _, data| {
                    data.state.render(dev_id, Some(crtc));
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        }
    }

    // If crtc is `Some()`, render it, else render all crtcs
    fn render(&mut self, dev_id: DrmNode, crtc: Option<crtc::Handle>) {
        let device_backend = match self.backend_data.backends.get_mut(&dev_id) {
            Some(backend) => backend,
            None => {
                error!("Trying to render on non-existent backend {}", dev_id);
                return;
            }
        };
        // setup two iterators on the stack, one over all surfaces for this backend, and
        // one containing only the one given as argument.
        // They make a trait-object to dynamically choose between the two
        let surfaces = device_backend.surfaces.borrow();
        let mut surfaces_iter = surfaces.iter();
        let mut option_iter = crtc
            .iter()
            .flat_map(|crtc| surfaces.get(crtc).map(|surface| (crtc, surface)));

        let to_render_iter: &mut dyn Iterator<Item = (&crtc::Handle, &Rc<RefCell<SurfaceData>>)> =
            if crtc.is_some() {
                &mut option_iter
            } else {
                &mut surfaces_iter
            };

        for (&crtc, surface) in to_render_iter {
            // TODO get scale from the rendersurface when supporting HiDPI
            let frame = self
                .backend_data
                .pointer_image
                .get_image(1 /*scale*/, self.clock.now().try_into().unwrap());

            let node = surface.borrow().render_node;
            let primary_gpu = self.backend_data.primary_gpu;
            let mut renderer = if primary_gpu == node {
                self.backend_data.gpus.single_renderer(&node)
            } else {
                let format = surface.borrow().compositor.format();
                self.backend_data.gpus.renderer(
                    &primary_gpu,
                    &node,
                    self.backend_data
                        .allocator
                        .as_mut()
                        // TODO: We could build some kind of `GLAllocator` using Renderbuffers in theory for this case.
                        //  That would work for memcpy's of offscreen contents.
                        .expect("We need an allocator for multigpu systems")
                        .as_mut(),
                    format,
                )
            }
            .unwrap();

            let pointer_images = &mut self.backend_data.pointer_images;
            let pointer_image = pointer_images
                .iter()
                .find_map(|(image, texture)| {
                    if image == &frame {
                        Some(texture.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| {
                    let texture = TextureBuffer::from_memory(
                        &mut renderer,
                        &frame.pixels_rgba,
                        (frame.width as i32, frame.height as i32),
                        false,
                        1,
                        Transform::Normal,
                        None,
                    )
                    .expect("Failed to import cursor bitmap");
                    pointer_images.push((frame, texture.clone()));
                    texture
                });

            let output = if let Some(output) = self.space.outputs().find(|o| {
                o.user_data().get::<UdevOutputId>()
                    == Some(&UdevOutputId {
                        device_id: surface.borrow().device_id,
                        crtc,
                    })
            }) {
                output.clone()
            } else {
                // somehow we got called with an invalid output
                continue;
            };

            let result = render_surface(
                &mut surface.borrow_mut(),
                &mut renderer,
                &self.space,
                &output,
                self.seat.input_method().unwrap(),
                self.pointer_location,
                &pointer_image,
                &mut self.backend_data.pointer_element,
                &self.dnd_icon,
                &mut self.cursor_status.lock().unwrap(),
                &self.clock,
                self.show_window_preview,
            );
            let reschedule = match &result {
                Ok(has_rendered) => !has_rendered,
                Err(err) => {
                    warn!("Error during rendering: {:?}", err);
                    match err {
                        SwapBuffersError::AlreadySwapped => false,
                        SwapBuffersError::TemporaryFailure(err) => !matches!(
                            err.downcast_ref::<DrmError>(),
                            Some(&DrmError::DeviceInactive)
                                | Some(&DrmError::Access {
                                    source: drm::SystemError::PermissionDenied,
                                    ..
                                })
                        ),
                        SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
                    }
                }
            };

            if reschedule {
                let output_refresh = match output.current_mode() {
                    Some(mode) => mode.refresh,
                    None => return,
                };
                // If reschedule is true we either hit a temporary failure or more likely rendering
                // did not cause any damage on the output. In this case we just re-schedule a repaint
                // after approx. one frame to re-test for damage.
                let reschedule_duration =
                    Duration::from_millis((1_000_000f32 / output_refresh as f32) as u64);
                trace!(
                    "reschedule repaint timer with delay {:?} on {:?}",
                    reschedule_duration,
                    crtc,
                );
                let timer = Timer::from_duration(reschedule_duration);
                self.handle
                    .insert_source(timer, move |_, _, data| {
                        data.state.render(dev_id, Some(crtc));
                        TimeoutAction::Drop
                    })
                    .expect("failed to schedule frame timer");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_surface<'a, 'b>(
    surface: &'a mut SurfaceData,
    renderer: &mut UdevRenderer<'a, 'b>,
    space: &Space<WindowElement>,
    output: &Output,
    input_method: &InputMethodHandle,
    pointer_location: Point<f64, Logical>,
    pointer_image: &TextureBuffer<MultiTexture>,
    pointer_element: &mut PointerElement<MultiTexture>,
    dnd_icon: &Option<wl_surface::WlSurface>,
    cursor_status: &mut CursorImageStatus,
    clock: &Clock<Monotonic>,
    show_window_preview: bool,
) -> Result<bool, SwapBuffersError> {
    let output_geometry = space.output_geometry(output).unwrap();
    let scale = Scale::from(output.current_scale().fractional_scale());

    let mut custom_elements: Vec<CustomRenderElements<_>> = Vec::new();
    // draw input method surface if any
    let rectangle = input_method.coordinates();
    let position = Point::from((
        rectangle.loc.x + rectangle.size.w,
        rectangle.loc.y + rectangle.size.h,
    ));
    input_method.with_surface(|surface| {
        custom_elements.extend(AsRenderElements::<UdevRenderer<'a, 'b>>::render_elements(
            &SurfaceTree::from_surface(surface),
            renderer,
            position.to_physical_precise_round(scale),
            scale,
        ));
    });

    if output_geometry.to_f64().contains(pointer_location) {
        let cursor_hotspot = if let CursorImageStatus::Surface(ref surface) = cursor_status {
            compositor::with_states(surface, |states| {
                states
                    .data_map
                    .get::<Mutex<CursorImageAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .hotspot
            })
        } else {
            (0, 0).into()
        };
        let cursor_pos = pointer_location - output_geometry.loc.to_f64() - cursor_hotspot.to_f64();
        let cursor_pos_scaled = cursor_pos.to_physical(scale).to_i32_round();

        // set cursor
        pointer_element.set_texture(pointer_image.clone());

        // draw the cursor as relevant
        {
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Surface(ref surface) = *cursor_status {
                reset = !surface.alive();
            }
            if reset {
                *cursor_status = CursorImageStatus::Default;
            }

            pointer_element.set_status(cursor_status.clone());
        }

        custom_elements.extend(pointer_element.render_elements(renderer, cursor_pos_scaled, scale));

        // draw the dnd icon if applicable
        {
            if let Some(wl_surface) = dnd_icon.as_ref() {
                if wl_surface.alive() {
                    custom_elements.extend(AsRenderElements::<UdevRenderer<'a, 'b>>::render_elements(
                        &SurfaceTree::from_surface(wl_surface),
                        renderer,
                        cursor_pos_scaled,
                        scale,
                    ));
                }
            }
        }
    }

    #[cfg(feature = "debug")]
    if let Some(element) = surface.fps_element.as_mut() {
        element.update_fps(surface.fps.avg().round() as u32);
        surface.fps.tick();
        custom_elements.push(CustomRenderElements::Fps(element.clone()));
    }

    let (elements, clear_color) =
        output_elements(output, space, custom_elements, renderer, show_window_preview);
    let (rendered, states) =
        surface
            .compositor
            .render_frame::<_, _, Gles2Renderbuffer>(renderer, &elements, clear_color)?;

    post_repaint(output, &states, space, clock.now());

    if rendered {
        let output_presentation_feedback = take_presentation_feedback(output, space, &states);
        surface
            .compositor
            .queue_frame(Some(output_presentation_feedback))
            .map_err(Into::<SwapBuffersError>::into)?;
    }

    Ok(rendered)
}

fn schedule_initial_render(
    gpus: &mut GpuManager<GbmGlesBackend<Gles2Renderer>>,
    surface: Rc<RefCell<SurfaceData>>,
    evt_handle: &LoopHandle<'static, CalloopData<UdevData>>,
) {
    let node = surface.borrow().render_node;
    let result = {
        let mut renderer = gpus.single_renderer(&node).unwrap();
        let mut surface = surface.borrow_mut();
        initial_render(&mut surface, &mut renderer)
    };
    if let Err(err) = result {
        match err {
            SwapBuffersError::AlreadySwapped => {}
            SwapBuffersError::TemporaryFailure(err) => {
                // TODO dont reschedule after 3(?) retries
                warn!("Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |data| {
                    schedule_initial_render(&mut data.state.backend_data.gpus, surface, &handle)
                });
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}

fn initial_render(
    surface: &mut SurfaceData,
    renderer: &mut UdevRenderer<'_, '_>,
) -> Result<(), SwapBuffersError> {
    surface
        .compositor
        .render_frame::<_, CustomRenderElements<_>, Gles2Renderbuffer>(renderer, &[], CLEAR_COLOR)?;
    surface.compositor.queue_frame(None)?;
    surface.compositor.reset_buffers();

    Ok(())
}
