use std::{
    borrow::Cow,
    cell::RefCell,
    collections::hash_map::{Entry, HashMap},
    os::unix::io::{AsRawFd, RawFd},
    path::PathBuf,
    rc::Rc,
    sync::{atomic::Ordering, Mutex},
    time::Duration,
};

use slog::Logger;

use crate::{
    drawing::*,
    render::*,
    state::{AnvilState, Backend, CalloopData},
};
#[cfg(feature = "egl")]
use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{ImportDma, ImportEgl},
    },
    delegate_dmabuf,
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportError},
};
use smithay::{
    backend::{
        drm::{DrmDevice, DrmError, DrmEvent, DrmNode, GbmBufferedSurface, NodeType},
        egl::{EGLContext, EGLDevice, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            damage::{DamageTrackedRenderer, DamageTrackedRendererError},
            element::AsRenderElements,
            gles2::{Gles2Renderbuffer, Gles2Renderer},
            multigpu::{egl::EglGlesBackend, GpuManager, MultiRenderer, MultiTexture},
            Bind, Frame, ImportMem, Renderer,
        },
        session::{auto::AutoSession, Session, Signal as SessionSignal},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
        SwapBuffersError,
    },
    desktop::{
        space::{Space, SurfaceTree},
        Window,
    },
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
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
        },
        gbm::Device as GbmDevice,
        input::Libinput,
        nix::{fcntl::OFlag, sys::stat::dev_t},
        wayland_server::{backend::GlobalId, protocol::wl_surface, Display, DisplayHandle},
    },
    utils::{
        signaling::{Linkable, SignalToken, Signaler},
        IsAlive, Logical, Point, Rectangle, Scale, Transform,
    },
    wayland::{
        compositor,
        input_method::{InputMethodHandle, InputMethodSeat},
        output::{Mode, Output, PhysicalProperties},
    },
};

type UdevRenderer<'a> =
    MultiRenderer<'a, 'a, EglGlesBackend<Gles2Renderer>, EglGlesBackend<Gles2Renderer>, Gles2Renderbuffer>;

#[derive(Copy, Clone)]
pub struct SessionFd(RawFd);
impl AsRawFd for SessionFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

#[derive(Debug, PartialEq)]
struct UdevOutputId {
    device_id: DrmNode,
    crtc: crtc::Handle,
}

pub struct UdevData {
    pub session: AutoSession,
    dh: DisplayHandle,
    #[cfg(feature = "egl")]
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    primary_gpu: DrmNode,
    gpus: GpuManager<EglGlesBackend<Gles2Renderer>>,
    backends: HashMap<DrmNode, BackendData>,
    pointer_images: Vec<(xcursor::parser::Image, MultiTexture)>,
    pointer_element: PointerElement<MultiTexture>,
    #[cfg(feature = "debug")]
    fps_texture: MultiTexture,
    signaler: Signaler<SessionSignal>,
    pointer_image: crate::cursor::Cursor,
    logger: slog::Logger,
}

#[cfg(feature = "egl")]
impl DmabufHandler for AnvilState<UdevData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf) -> Result<(), ImportError> {
        self.backend_data
            .gpus
            .renderer::<Gles2Renderbuffer>(&self.backend_data.primary_gpu, &self.backend_data.primary_gpu)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .map(|_| ())
            .map_err(|_| ImportError::Failed)
    }
}
#[cfg(feature = "egl")]
delegate_dmabuf!(AnvilState<UdevData>);

impl Backend for UdevData {
    fn seat_name(&self) -> String {
        self.session.seat()
    }

    fn reset_buffers(&mut self, output: &Output) {
        if let Some(id) = output.user_data().get::<UdevOutputId>() {
            if let Some(gpu) = self.backends.get(&id.device_id) {
                let surfaces = gpu.surfaces.borrow();
                if let Some(surface) = surfaces.get(&id.crtc) {
                    surface.borrow_mut().surface.reset_buffers();
                }
            }
        }
    }

    fn early_import(&mut self, surface: &wl_surface::WlSurface) {
        if let Err(err) = self
            .gpus
            .early_import(Some(self.primary_gpu), self.primary_gpu, surface)
        {
            warn!(self.logger, "Early buffer import failed: {}", err);
        }
    }
}

pub fn run_udev(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let mut display = Display::new().unwrap();

    /*
     * Initialize session
     */
    let (session, notifier) = match AutoSession::new(log.clone()) {
        Some(ret) => ret,
        None => {
            crit!(log, "Could not initialize a session");
            return;
        }
    };
    let session_signal = notifier.signaler();

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
    info!(log, "Using {} as primary gpu.", primary_gpu);

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let mut gpus = GpuManager::new(EglGlesBackend::default(), log.clone()).unwrap();
    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    #[cfg(any(feature = "egl", feature = "debug"))]
    let mut renderer = gpus
        .renderer::<Gles2Renderbuffer>(&primary_gpu, &primary_gpu)
        .unwrap();

    #[cfg(feature = "debug")]
    let fps_image =
        image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
            .decode()
            .unwrap();
    #[cfg(feature = "debug")]
    let fps_texture = renderer
        .import_memory(
            &fps_image.to_rgba8(),
            (fps_image.width() as i32, fps_image.height() as i32).into(),
            false,
        )
        .expect("Unable to upload FPS texture");

    // init dmabuf support with format list from our primary gpu
    // TODO: This does not necessarily depend on egl, but mesa makes no use of it without wl_drm right now
    #[cfg(feature = "egl")]
    let dmabuf_state = {
        info!(
            log,
            "Trying to initialize EGL Hardware Acceleration via {:?}", primary_gpu
        );

        if renderer.bind_wl_display(&display.handle()).is_ok() {
            info!(log, "EGL hardware-acceleration enabled");
            let dmabuf_formats = renderer.dmabuf_formats().cloned().collect::<Vec<_>>();
            let mut state = DmabufState::new();
            let global = state.create_global::<AnvilState<UdevData>, _>(
                &display.handle(),
                dmabuf_formats,
                log.clone(),
            );
            Some((state, global))
        } else {
            None
        }
    };

    let data = UdevData {
        dh: display.handle(),
        #[cfg(feature = "egl")]
        dmabuf_state,
        session,
        primary_gpu,
        gpus,
        backends: HashMap::new(),
        signaler: session_signal.clone(),
        pointer_image: crate::cursor::Cursor::load(&log),
        pointer_images: Vec::new(),
        pointer_element: PointerElement::default(),
        #[cfg(feature = "debug")]
        fps_texture,
        logger: log.clone(),
    };
    let mut state = AnvilState::init(&mut display, event_loop.handle(), data, log.clone(), true);

    /*
     * Initialize the udev backend
     */
    let udev_backend = match UdevBackend::new(&state.seat_name, log.clone()) {
        Ok(ret) => ret,
        Err(err) => {
            crit!(log, "Failed to initialize udev backend"; "error" => err);
            return;
        }
    };

    /*
     * Initialize a fake output (we render one screen to every device in this example)
     */

    /*
     * Initialize libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<AutoSession>>(
        state.backend_data.session.clone().into(),
    );
    libinput_context.udev_assign_seat(&state.seat_name).unwrap();
    let mut libinput_backend = LibinputInputBackend::new(libinput_context, log.clone());
    libinput_backend.link(session_signal);

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
    event_loop
        .handle()
        .insert_source(notifier, |(), &mut (), _data| {})
        .unwrap();
    for (dev, path) in udev_backend.device_list() {
        state.device_added(&mut display, dev, path.into())
    }

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
    state.start_xwayland();

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

pub type RenderSurface = GbmBufferedSurface<Rc<RefCell<GbmDevice<SessionFd>>>, SessionFd>;

struct SurfaceData {
    dh: DisplayHandle,
    device_id: DrmNode,
    render_node: DrmNode,
    surface: RenderSurface,
    global: Option<GlobalId>,
    damage_tracked_renderer: DamageTrackedRenderer,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
    #[cfg(feature = "debug")]
    fps_element: FpsElement<MultiTexture>,
}

impl Drop for SurfaceData {
    fn drop(&mut self) {
        if let Some(global) = self.global.take() {
            self.dh.remove_global::<AnvilState<UdevBackend>>(global);
        }
    }
}

struct BackendData {
    _restart_token: SignalToken,
    surfaces: Rc<RefCell<HashMap<crtc::Handle, Rc<RefCell<SurfaceData>>>>>,
    gbm: Rc<RefCell<GbmDevice<SessionFd>>>,
    registration_token: RegistrationToken,
    event_dispatcher: Dispatcher<'static, DrmDevice<SessionFd>, CalloopData<UdevData>>,
}

#[allow(clippy::too_many_arguments)]
fn scan_connectors(
    device_id: DrmNode,
    device: &DrmDevice<SessionFd>,
    gbm: &Rc<RefCell<GbmDevice<SessionFd>>>,
    display: &mut Display<AnvilState<UdevData>>,
    space: &mut Space<Window>,
    #[cfg(feature = "debug")] fps_texture: &MultiTexture,
    signaler: &Signaler<SessionSignal>,
    logger: &::slog::Logger,
) -> HashMap<crtc::Handle, Rc<RefCell<SurfaceData>>> {
    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = device.resource_handles().unwrap();

    // Find all connected output ports.
    let connector_infos: Vec<ConnectorInfo> = res_handles
        .connectors()
        .iter()
        .map(|conn| device.get_connector(*conn, true).unwrap())
        .filter(|conn| conn.state() == ConnectorState::Connected)
        .inspect(|conn| info!(logger, "Connected: {:?}", conn.interface()))
        .collect();

    let mut backends = HashMap::new();

    let (render_node, formats) = {
        let display = unsafe { EGLDisplay::new(&*gbm.borrow(), logger.clone()).unwrap() };
        let node = match EGLDevice::device_for_display(&display)
            .ok()
            .and_then(|x| x.try_get_render_node().ok().flatten())
        {
            Some(node) => node,
            None => return HashMap::new(),
        };
        let context = EGLContext::new(&display, logger.clone()).unwrap();
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
                logger,
                "Trying to setup connector {:?}-{} with crtc {:?}",
                connector_info.interface(),
                connector_info.interface_id(),
                crtc,
            );

            let mode = connector_info.modes()[0];
            let mut surface = match device.create_surface(crtc, mode, &[connector_info.handle()]) {
                Ok(surface) => surface,
                Err(err) => {
                    warn!(logger, "Failed to create drm surface: {}", err);
                    continue;
                }
            };
            surface.link(signaler.clone());

            let gbm_surface =
                match GbmBufferedSurface::new(surface, gbm.clone(), formats.clone(), logger.clone()) {
                    Ok(renderer) => renderer,
                    Err(err) => {
                        warn!(logger, "Failed to create rendering surface: {}", err);
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
                None,
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

            let damage_tracked_renderer = DamageTrackedRenderer::from_output(&output);
            #[cfg(feature = "debug")]
            let fps_element = FpsElement::new(fps_texture.clone());

            entry.insert(Rc::new(RefCell::new(SurfaceData {
                dh: display.handle(),
                device_id,
                render_node,
                surface: gbm_surface,
                global: Some(global),
                damage_tracked_renderer,
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
            .map(SessionFd)
            .map(|fd| (DrmDevice::new(fd, true, self.log.clone()), GbmDevice::new(fd)));

        // Report device open failures.
        let (mut device, gbm) = match devices {
            Some((Ok(drm), Ok(gbm))) => (drm, gbm),
            Some((Err(err), _)) => {
                warn!(
                    self.log,
                    "Skipping device {:?}, because of drm error: {}", device_id, err
                );
                return;
            }
            Some((_, Err(err))) => {
                // TODO try DumbBuffer allocator in this case
                warn!(
                    self.log,
                    "Skipping device {:?}, because of gbm error: {}", device_id, err
                );
                return;
            }
            None => return,
        };

        let gbm = Rc::new(RefCell::new(gbm));
        let node = match DrmNode::from_dev_id(device_id) {
            Ok(node) => node,
            Err(err) => {
                warn!(self.log, "Failed to access drm node for {}: {}", device_id, err);
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
            &self.backend_data.fps_texture,
            &self.backend_data.signaler,
            &self.log,
        )));

        let handle = self.handle.clone();
        let restart_token = self.backend_data.signaler.register(move |signal| match signal {
            SessionSignal::ActivateSession | SessionSignal::ActivateDevice { .. } => {
                handle.insert_idle(move |data| data.state.render(node, None));
            }
            _ => {}
        });

        device.link(self.backend_data.signaler.clone());
        let event_dispatcher =
            Dispatcher::new(device, move |event, _, data: &mut CalloopData<_>| match event {
                DrmEvent::VBlank(crtc) => data.state.render(node, Some(crtc)),
                DrmEvent::Error(error) => {
                    error!(data.state.log, "{:?}", error);
                }
            });
        let registration_token = self.handle.register_dispatcher(event_dispatcher.clone()).unwrap();

        for backend in backends.borrow_mut().values() {
            // render first frame
            trace!(self.log, "Scheduling frame");
            schedule_initial_render(
                &mut self.backend_data.gpus,
                backend.clone(),
                &self.handle,
                self.log.clone(),
            );
        }

        self.backend_data.backends.insert(
            node,
            BackendData {
                _restart_token: restart_token,
                registration_token,
                event_dispatcher,
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
            let logger = self.log.clone();
            let loop_handle = self.handle.clone();
            let signaler = self.backend_data.signaler.clone();

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
                &self.backend_data.fps_texture,
                &signaler,
                &logger,
            );

            // fixup window coordinates
            crate::shell::fixup_positions(&mut self.space);

            for surface in backends.values() {
                let logger = logger.clone();
                // render first frame
                schedule_initial_render(&mut self.backend_data.gpus, surface.clone(), &loop_handle, logger);
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
            // drop surfaces
            backend_data.surfaces.borrow_mut().clear();
            debug!(self.log, "Surfaces dropped");

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

            debug!(self.log, "Dropping device");
        }
    }

    // If crtc is `Some()`, render it, else render all crtcs
    fn render(&mut self, dev_id: DrmNode, crtc: Option<crtc::Handle>) {
        let device_backend = match self.backend_data.backends.get_mut(&dev_id) {
            Some(backend) => backend,
            None => {
                error!(self.log, "Trying to render on non-existent backend {}", dev_id);
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
                .get_image(1 /*scale*/, self.start_time.elapsed().as_millis() as u32);
            let primary_gpu = self.backend_data.primary_gpu;
            let mut renderer = self
                .backend_data
                .gpus
                .renderer::<Gles2Renderbuffer>(&primary_gpu, &surface.borrow().render_node)
                .unwrap();
            let pointer_images = &mut self.backend_data.pointer_images;
            let pointer_image = pointer_images
                .iter()
                .find_map(|(image, texture)| if image == &frame { Some(texture) } else { None })
                .cloned()
                .unwrap_or_else(|| {
                    let texture = renderer
                        .import_memory(
                            &frame.pixels_rgba,
                            (frame.width as i32, frame.height as i32).into(),
                            false,
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
                &mut *surface.borrow_mut(),
                &mut renderer,
                &mut self.space,
                &output,
                self.seat.input_method().unwrap(),
                self.pointer_location,
                &pointer_image,
                &mut self.backend_data.pointer_element,
                &self.dnd_icon,
                &mut *self.cursor_status.lock().unwrap(),
                &self.log,
            );
            let reschedule = match result {
                Ok(has_rendered) => !has_rendered,
                Err(err) => {
                    warn!(self.log, "Error during rendering: {:?}", err);
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
                let timer = Timer::from_duration(Duration::from_millis(
                    1000 /*a seconds*/ / 60, /*refresh rate*/
                ));
                self.handle
                    .insert_source(timer, move |_, _, data| {
                        data.state.render(dev_id, Some(crtc));
                        TimeoutAction::Drop
                    })
                    .expect("failed to schedule frame timer");
            }

            // Send frame events so that client start drawing their next frame
            let space = &self.space;
            let start_time = &self.start_time;
            space.elements().for_each(|window| {
                if space.outputs_for_element(window).contains(&output) {
                    window.send_frame(start_time.elapsed().as_millis() as u32)
                }
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_surface<'a>(
    surface: &'a mut SurfaceData,
    renderer: &mut UdevRenderer<'a>,
    space: &mut Space<Window>,
    output: &Output,
    input_method: &InputMethodHandle,
    pointer_location: Point<f64, Logical>,
    pointer_image: &MultiTexture,
    pointer_element: &mut PointerElement<MultiTexture>,
    dnd_icon: &Option<wl_surface::WlSurface>,
    cursor_status: &mut CursorImageStatus,
    logger: &slog::Logger,
) -> Result<bool, SwapBuffersError> {
    surface.surface.frame_submitted()?;

    let output_geometry = space.output_geometry(output).unwrap();

    let (dmabuf, age) = surface.surface.next_buffer()?;
    renderer.bind(dmabuf)?;

    let mut elements: Vec<CustomRenderElements<_>> = Vec::new();
    // set cursor
    if output_geometry.to_f64().contains(pointer_location) {
        let (ptr_x, ptr_y) = pointer_location.into();
        let ptr_location = Point::<i32, Logical>::from((ptr_x as i32, ptr_y as i32)); // - output_geometry.loc;
        let scale = Scale::from(output.current_scale().fractional_scale());

        pointer_element.set_position(ptr_location);
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

        elements
            .extend(pointer_element.render_elements(ptr_location.to_physical_precise_round(scale), scale));

        // draw input method surface if any
        let rectangle = input_method.coordinates();
        let position = Point::from((
            rectangle.loc.x + rectangle.size.w,
            rectangle.loc.y + rectangle.size.h,
        ));
        input_method.with_surface(|surface| {
            elements.extend(AsRenderElements::<UdevRenderer<'a>>::render_elements(
                &SurfaceTree::from_surface(surface, position),
                position.to_physical_precise_round(scale),
                scale,
            ));
        });

        // draw the dnd icon if applicable
        {
            if let Some(wl_surface) = dnd_icon.as_ref() {
                if wl_surface.alive() {
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
                    elements.extend(AsRenderElements::<UdevRenderer<'a>>::render_elements(
                        &SurfaceTree::from_surface(wl_surface, ptr_location - cursor_hotspot),
                        ptr_location.to_physical_precise_round(scale),
                        scale,
                    ));
                }
            }
        }

        #[cfg(feature = "debug")]
        {
            surface.fps_element.update_fps(surface.fps.avg().round() as u32);
            surface.fps.tick();
            elements.push(CustomRenderElements::Fps(surface.fps_element.clone()));
        }
    }

    // and draw to our buffer
    // TODO we can pass the damage rectangles inside a AtomicCommitRequest
    #[cfg(feature = "debug")]
    let render_res = render_output(
        output,
        space,
        &elements,
        renderer,
        &mut surface.damage_tracked_renderer,
        age.into(),
        logger,
    )
    .map(|x| x.is_some());

    #[cfg(not(feature = "debug"))]
    let render_res = render_output(
        output,
        space,
        &*elements,
        renderer,
        &mut surface.damage_tracked_renderer,
        age.into(),
        logger,
    )
    .map(|x| x.is_some());

    match render_res.map_err(|err| match err {
        DamageTrackedRendererError::Rendering(err) => err.into(),
        _ => unreachable!(),
    }) {
        Ok(true) => {
            surface
                .surface
                .queue_buffer()
                .map_err(Into::<SwapBuffersError>::into)?;
            Ok(true)
        }
        x => x,
    }
}

fn schedule_initial_render(
    gpus: &mut GpuManager<EglGlesBackend<Gles2Renderer>>,
    surface: Rc<RefCell<SurfaceData>>,
    evt_handle: &LoopHandle<'static, CalloopData<UdevData>>,
    logger: ::slog::Logger,
) {
    let node = surface.borrow().render_node;
    let result = {
        let mut renderer = gpus.renderer::<Gles2Renderbuffer>(&node, &node).unwrap();
        let mut surface = surface.borrow_mut();
        initial_render(&mut surface.surface, &mut renderer)
    };
    if let Err(err) = result {
        match err {
            SwapBuffersError::AlreadySwapped => {}
            SwapBuffersError::TemporaryFailure(err) => {
                // TODO dont reschedule after 3(?) retries
                warn!(logger, "Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |data| {
                    schedule_initial_render(&mut data.state.backend_data.gpus, surface, &handle, logger)
                });
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}

fn initial_render(
    surface: &mut RenderSurface,
    renderer: &mut UdevRenderer<'_>,
) -> Result<(), SwapBuffersError> {
    let (dmabuf, _age) = surface.next_buffer()?;
    renderer.bind(dmabuf)?;
    // Does not matter if we render an empty frame
    renderer
        .render((1, 1).into(), Transform::Normal, |_, frame| {
            frame
                .clear(CLEAR_COLOR, &[Rectangle::from_loc_and_size((0, 0), (1, 1))])
                .map_err(Into::<SwapBuffersError>::into)
        })
        .map_err(Into::<SwapBuffersError>::into)
        .and_then(|x| x.map_err(Into::<SwapBuffersError>::into))?;
    surface.queue_buffer()?;
    surface.reset_buffers();
    Ok(())
}
