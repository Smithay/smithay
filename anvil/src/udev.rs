use std::{
    cell::RefCell,
    collections::hash_map::{Entry, HashMap},
    io::Error as IoError,
    os::unix::io::{AsRawFd, RawFd},
    path::PathBuf,
    rc::Rc,
    sync::atomic::Ordering,
    time::Duration,
};

use image::ImageBuffer;
use slog::Logger;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        drm::{DrmDevice, DrmError, DrmEvent, GbmBufferedSurface},
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            gles2::{Gles2Renderer, Gles2Texture},
            Bind, Frame, Renderer, Transform,
        },
        session::{auto::AutoSession, Session, Signal as SessionSignal},
        udev::{UdevBackend, UdevEvent},
        SwapBuffersError,
    },
    desktop::{
        draw_window,
        space::{DynamicRenderElements, RenderError, Space},
        Window,
    },
    reexports::{
        calloop::{
            timer::{Timer, TimerHandle},
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
        wayland_server::{
            protocol::{wl_output, wl_surface},
            Display, Global,
        },
    },
    utils::{
        signaling::{Linkable, SignalToken, Signaler},
        Logical, Point, Rectangle,
    },
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};
#[cfg(feature = "egl")]
use smithay::{
    backend::{
        drm::DevPath,
        renderer::{ImportDma, ImportEgl},
        udev::primary_gpu,
    },
    wayland::dmabuf::init_dmabuf_global,
};

use crate::{
    drawing::*,
    shell::FullscreenSurface,
    state::{AnvilState, Backend},
};

#[derive(Clone)]
pub struct SessionFd(RawFd);
impl AsRawFd for SessionFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

#[derive(Debug, PartialEq)]
struct UdevOutputId {
    device_id: dev_t,
    crtc: crtc::Handle,
}

pub struct UdevData {
    pub session: AutoSession,
    #[cfg(feature = "egl")]
    primary_gpu: Option<PathBuf>,
    backends: HashMap<dev_t, BackendData>,
    signaler: Signaler<SessionSignal>,
    pointer_image: crate::cursor::Cursor,
    render_timer: TimerHandle<(u64, crtc::Handle)>,
}

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
}

pub fn run_udev(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let name = display
        .borrow_mut()
        .add_socket_auto()
        .unwrap()
        .into_string()
        .unwrap();
    info!(log, "Listening on wayland socket"; "name" => name.clone());
    ::std::env::set_var("WAYLAND_DISPLAY", name);
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
    #[cfg(feature = "egl")]
    let primary_gpu = primary_gpu(&session.seat()).unwrap_or_default();

    // setup the timer
    let timer = Timer::new().unwrap();

    let data = UdevData {
        session,
        #[cfg(feature = "egl")]
        primary_gpu,
        backends: HashMap::new(),
        signaler: session_signal.clone(),
        pointer_image: crate::cursor::Cursor::load(&log),
        render_timer: timer.handle(),
    };
    let mut state = AnvilState::init(display.clone(), event_loop.handle(), data, log.clone(), true);

    // re-render timer
    event_loop
        .handle()
        .insert_source(timer, |(dev_id, crtc), _, anvil_state| {
            anvil_state.render(dev_id, Some(crtc))
        })
        .unwrap();

    /*
     * Initialize the udev backend
     */
    let udev_backend = match UdevBackend::new(state.seat_name.clone(), log.clone()) {
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
    let libinput_event_source = event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, anvil_state| {
            anvil_state.process_input_event(event)
        })
        .unwrap();
    let session_event_source = event_loop
        .handle()
        .insert_source(notifier, |(), &mut (), _anvil_state| {})
        .unwrap();
    for (dev, path) in udev_backend.device_list() {
        state.device_added(dev, path.into())
    }

    // init dmabuf support with format list from all gpus
    // TODO: We need to update this list, when the set of gpus changes
    // TODO2: This does not necessarily depend on egl, but mesa makes no use of it without wl_drm right now
    #[cfg(feature = "egl")]
    {
        let mut formats = Vec::new();
        for backend_data in state.backend_data.backends.values() {
            formats.extend(backend_data.renderer.borrow().dmabuf_formats().cloned());
        }

        init_dmabuf_global(
            &mut *display.borrow_mut(),
            formats,
            |buffer, mut ddata| {
                let anvil_state = ddata.get::<AnvilState<UdevData>>().unwrap();
                for backend_data in anvil_state.backend_data.backends.values() {
                    if backend_data.renderer.borrow_mut().import_dmabuf(buffer).is_ok() {
                        return true;
                    }
                }
                false
            },
            log.clone(),
        );
    }

    let udev_event_source = event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, state| match event {
            UdevEvent::Added { device_id, path } => state.device_added(device_id, path),
            UdevEvent::Changed { device_id } => state.device_changed(device_id),
            UdevEvent::Removed { device_id } => state.device_removed(device_id),
        })
        .map_err(|e| -> IoError { e.into() })
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
        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            display.borrow_mut().flush_clients(&mut state);
            state.space.borrow_mut().refresh();
        }
    }

    // Cleanup stuff
    event_loop.handle().remove(session_event_source);
    event_loop.handle().remove(libinput_event_source);
    event_loop.handle().remove(udev_event_source);
}

pub type RenderSurface = GbmBufferedSurface<SessionFd>;

struct SurfaceData {
    surface: RenderSurface,
    global: Option<Global<wl_output::WlOutput>>,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
}

impl Drop for SurfaceData {
    fn drop(&mut self) {
        if let Some(global) = self.global.take() {
            global.destroy();
        }
    }
}

struct BackendData {
    _restart_token: SignalToken,
    surfaces: Rc<RefCell<HashMap<crtc::Handle, Rc<RefCell<SurfaceData>>>>>,
    pointer_images: Vec<(xcursor::parser::Image, Gles2Texture)>,
    #[cfg(feature = "debug")]
    fps_texture: Gles2Texture,
    renderer: Rc<RefCell<Gles2Renderer>>,
    gbm: GbmDevice<SessionFd>,
    registration_token: RegistrationToken,
    event_dispatcher: Dispatcher<'static, DrmDevice<SessionFd>, AnvilState<UdevData>>,
    dev_id: u64,
}

fn scan_connectors(
    device: &mut DrmDevice<SessionFd>,
    gbm: &GbmDevice<SessionFd>,
    renderer: &mut Gles2Renderer,
    display: &mut Display,
    space: &mut Space,
    signaler: &Signaler<SessionSignal>,
    logger: &::slog::Logger,
) -> HashMap<crtc::Handle, Rc<RefCell<SurfaceData>>> {
    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = device.resource_handles().unwrap();

    // Use first connected connector
    let connector_infos: Vec<ConnectorInfo> = res_handles
        .connectors()
        .iter()
        .map(|conn| device.get_connector(*conn).unwrap())
        .filter(|conn| conn.state() == ConnectorState::Connected)
        .inspect(|conn| info!(logger, "Connected: {:?}", conn.interface()))
        .collect();

    let mut backends = HashMap::new();

    // very naive way of finding good crtc/encoder/connector combinations. This problem is np-complete
    for connector_info in connector_infos {
        let encoder_infos = connector_info
            .encoders()
            .iter()
            .filter_map(|e| *e)
            .flat_map(|encoder_handle| device.get_encoder(encoder_handle))
            .collect::<Vec<EncoderInfo>>();
        'outer: for encoder_info in encoder_infos {
            for crtc in res_handles.filter_crtcs(encoder_info.possible_crtcs()) {
                if let Entry::Vacant(entry) = backends.entry(crtc) {
                    info!(
                        logger,
                        "Trying to setup connector {:?}-{} with crtc {:?}",
                        connector_info.interface(),
                        connector_info.interface_id(),
                        crtc,
                    );
                    let mut surface = match device.create_surface(
                        crtc,
                        connector_info.modes()[0],
                        &[connector_info.handle()],
                    ) {
                        Ok(surface) => surface,
                        Err(err) => {
                            warn!(logger, "Failed to create drm surface: {}", err);
                            continue;
                        }
                    };
                    surface.link(signaler.clone());

                    let renderer_formats =
                        Bind::<Dmabuf>::supported_formats(renderer).expect("Dmabuf renderer without formats");

                    let gbm_surface =
                        match GbmBufferedSurface::new(surface, gbm.clone(), renderer_formats, logger.clone())
                        {
                            Ok(renderer) => renderer,
                            Err(err) => {
                                warn!(logger, "Failed to create rendering surface: {}", err);
                                continue;
                            }
                        };

                    let mode = connector_info.modes()[0];
                    let size = mode.size();
                    let mode = Mode {
                        size: (size.0 as i32, size.1 as i32).into(),
                        refresh: (mode.vrefresh() * 1000) as i32,
                    };

                    let other_short_name;
                    let interface_short_name = match connector_info.interface() {
                        drm::control::connector::Interface::DVII => "DVI-I",
                        drm::control::connector::Interface::DVID => "DVI-D",
                        drm::control::connector::Interface::DVIA => "DVI-A",
                        drm::control::connector::Interface::SVideo => "S-VIDEO",
                        drm::control::connector::Interface::DisplayPort => "DP",
                        drm::control::connector::Interface::HDMIA => "HDMI-A",
                        drm::control::connector::Interface::HDMIB => "HDMI-B",
                        drm::control::connector::Interface::EmbeddedDisplayPort => "eDP",
                        other => {
                            other_short_name = format!("{:?}", other);
                            &other_short_name
                        }
                    };

                    let output_name = format!("{}-{}", interface_short_name, connector_info.interface_id());

                    let (phys_w, phys_h) = connector_info.size().unwrap_or((0, 0));
                    let (output, global) = Output::new(
                        display,
                        output_name,
                        PhysicalProperties {
                            size: (phys_w as i32, phys_h as i32).into(),
                            subpixel: wl_output::Subpixel::Unknown,
                            make: "Smithay".into(),
                            model: "Generic DRM".into(),
                        },
                        None,
                    );
                    let position = (
                        space
                            .outputs()
                            .fold(0, |acc, o| acc + space.output_geometry(o).unwrap().size.w),
                        0,
                    )
                        .into();
                    output.change_current_state(Some(mode), None, None, Some(position));
                    output.set_preferred(mode);
                    space.map_output(&output, 1.0, position);

                    output.user_data().insert_if_missing(|| UdevOutputId {
                        crtc,
                        device_id: device.device_id(),
                    });

                    entry.insert(Rc::new(RefCell::new(SurfaceData {
                        surface: gbm_surface,
                        global: Some(global),
                        #[cfg(feature = "debug")]
                        fps: fps_ticker::Fps::default(),
                    })));
                    break 'outer;
                }
            }
        }
    }

    backends
}

impl AnvilState<UdevData> {
    fn device_added(&mut self, device_id: dev_t, path: PathBuf) {
        // Try to open the device
        if let Some((mut device, gbm)) = self
            .backend_data
            .session
            .open(
                &path,
                OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOCTTY | OFlag::O_NONBLOCK,
            )
            .ok()
            .and_then(|fd| {
                match {
                    let fd = SessionFd(fd);
                    (
                        DrmDevice::new(fd.clone(), true, self.log.clone()),
                        GbmDevice::new(fd),
                    )
                } {
                    (Ok(drm), Ok(gbm)) => Some((drm, gbm)),
                    (Err(err), _) => {
                        warn!(
                            self.log,
                            "Skipping device {:?}, because of drm error: {}", device_id, err
                        );
                        None
                    }
                    (_, Err(err)) => {
                        // TODO try DumbBuffer allocator in this case
                        warn!(
                            self.log,
                            "Skipping device {:?}, because of gbm error: {}", device_id, err
                        );
                        None
                    }
                }
            })
        {
            let egl = match EGLDisplay::new(&gbm, self.log.clone()) {
                Ok(display) => display,
                Err(err) => {
                    warn!(
                        self.log,
                        "Skipping device {:?}, because of egl display error: {}", device_id, err
                    );
                    return;
                }
            };

            let context = match EGLContext::new(&egl, self.log.clone()) {
                Ok(context) => context,
                Err(err) => {
                    warn!(
                        self.log,
                        "Skipping device {:?}, because of egl context error: {}", device_id, err
                    );
                    return;
                }
            };

            let renderer = Rc::new(RefCell::new(unsafe {
                Gles2Renderer::new(context, self.log.clone()).unwrap()
            }));

            #[cfg(feature = "egl")]
            if path.canonicalize().ok() == self.backend_data.primary_gpu {
                info!(self.log, "Initializing EGL Hardware Acceleration via {:?}", path);
                if renderer
                    .borrow_mut()
                    .bind_wl_display(&*self.display.borrow())
                    .is_ok()
                {
                    info!(self.log, "EGL hardware-acceleration enabled");
                }
            }

            let backends = Rc::new(RefCell::new(scan_connectors(
                &mut device,
                &gbm,
                &mut *renderer.borrow_mut(),
                &mut *self.display.borrow_mut(),
                &mut *self.space.borrow_mut(),
                &self.backend_data.signaler,
                &self.log,
            )));

            let dev_id = device.device_id();
            let handle = self.handle.clone();
            let restart_token = self.backend_data.signaler.register(move |signal| match signal {
                SessionSignal::ActivateSession | SessionSignal::ActivateDevice { .. } => {
                    handle.insert_idle(move |anvil_state| anvil_state.render(dev_id, None));
                }
                _ => {}
            });

            device.link(self.backend_data.signaler.clone());
            let event_dispatcher = Dispatcher::new(
                device,
                move |event, _, anvil_state: &mut AnvilState<_>| match event {
                    DrmEvent::VBlank(crtc) => anvil_state.render(dev_id, Some(crtc)),
                    DrmEvent::Error(error) => {
                        error!(anvil_state.log, "{:?}", error);
                    }
                },
            );
            let registration_token = self.handle.register_dispatcher(event_dispatcher.clone()).unwrap();

            trace!(self.log, "Backends: {:?}", backends.borrow().keys());
            for backend in backends.borrow_mut().values() {
                // render first frame
                trace!(self.log, "Scheduling frame");
                schedule_initial_render(backend.clone(), renderer.clone(), &self.handle, self.log.clone());
            }

            #[cfg(feature = "debug")]
            let fps_texture = import_bitmap(
                &mut renderer.borrow_mut(),
                &image::io::Reader::with_format(
                    std::io::Cursor::new(FPS_NUMBERS_PNG),
                    image::ImageFormat::Png,
                )
                .decode()
                .unwrap()
                .to_rgba8(),
            )
            .expect("Unable to upload FPS texture");

            self.backend_data.backends.insert(
                dev_id,
                BackendData {
                    _restart_token: restart_token,
                    registration_token,
                    event_dispatcher,
                    surfaces: backends,
                    renderer,
                    gbm,
                    pointer_images: Vec::new(),
                    #[cfg(feature = "debug")]
                    fps_texture,
                    dev_id,
                },
            );
        }
    }

    fn device_changed(&mut self, device: dev_t) {
        //quick and dirty, just re-init all backends
        if let Some(ref mut backend_data) = self.backend_data.backends.get_mut(&device) {
            let logger = self.log.clone();
            let loop_handle = self.handle.clone();
            let signaler = self.backend_data.signaler.clone();
            let mut space = self.space.borrow_mut();

            let relocate_windows = space
                .outputs()
                .map(|o| {
                    let geo = space.output_geometry(o).unwrap();
                    let windows = space
                        .windows()
                        .filter_map(|w| {
                            let mut w_geo = space.window_geometry(w).unwrap();
                            if geo.contains(w_geo.loc) {
                                w_geo.loc -= geo.loc;
                                Some((
                                    w.clone(),
                                    (
                                        geo.size.w as f64 / w_geo.loc.x as f64,
                                        geo.size.h as f64 / w_geo.loc.y as f64,
                                    ),
                                ))
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    (o.clone(), windows)
                })
                .collect::<Vec<(Output, Vec<(Window, (f64, f64))>)>>();
            for output in space.outputs().cloned().collect::<Vec<_>>().into_iter() {
                space.unmap_output(&output);
            }

            let mut source = backend_data.event_dispatcher.as_source_mut();
            let mut backends = backend_data.surfaces.borrow_mut();
            *backends = scan_connectors(
                &mut *source,
                &backend_data.gbm,
                &mut *backend_data.renderer.borrow_mut(),
                &mut *self.display.borrow_mut(),
                &mut *self.space.borrow_mut(),
                &signaler,
                &logger,
            );

            for (output, window) in relocate_windows.into_iter() {
                let new_output_geo = space
                    .outputs()
                    .find(|o| o.name() == output.name())
                    .or_else(|| space.outputs().next())
                    .map(|new_output| space.output_geometry(new_output).unwrap())
                    .unwrap_or(Rectangle::from_loc_and_size((0, 0), (100, 100)));
                for (window, (x_offset, y_offset)) in window {
                    let offset = Point::from((
                        x_offset * new_output_geo.size.w as f64,
                        y_offset * new_output_geo.size.h as f64,
                    ))
                    .to_i32_round();
                    let new_pos = new_output_geo.loc + offset;
                    space.map_window(&window, new_pos, true);
                }
            }

            for renderer in backends.values() {
                let logger = logger.clone();
                // render first frame
                schedule_initial_render(
                    renderer.clone(),
                    backend_data.renderer.clone(),
                    &loop_handle,
                    logger,
                );
            }
        }
    }

    fn device_removed(&mut self, device: dev_t) {
        // drop the backends on this side
        if let Some(backend_data) = self.backend_data.backends.remove(&device) {
            // drop surfaces
            backend_data.surfaces.borrow_mut().clear();
            debug!(self.log, "Surfaces dropped");
            let mut space = self.space.borrow_mut();

            let i = space
                .outputs()
                .enumerate()
                .find(|(_, output)| {
                    output
                        .user_data()
                        .get::<UdevOutputId>()
                        .map(|id| id.device_id != device)
                        .unwrap_or(true)
                })
                .map(|(i, _)| i)
                .unwrap();

            let relocate_windows = space
                .outputs()
                .skip(i)
                .map(|o| {
                    let geo = space.output_geometry(o).unwrap();
                    let windows = space
                        .windows()
                        .filter_map(|w| {
                            let mut w_geo = space.window_geometry(w).unwrap();
                            if geo.contains(w_geo.loc) {
                                w_geo.loc -= geo.loc;
                                Some((w.clone(), w_geo.loc))
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    (o.clone(), windows)
                })
                .collect::<Vec<(Output, Vec<(Window, Point<i32, Logical>)>)>>();
            for output in relocate_windows.iter().map(|(o, _)| o) {
                space.unmap_output(output);
            }

            for (window, offset) in relocate_windows[0].1.iter() {
                space.map_window(window, *offset, true);
            }
            for (output, windows) in relocate_windows.into_iter().skip(1) {
                let position = (
                    space
                        .outputs()
                        .fold(0, |acc, o| acc + space.output_geometry(o).unwrap().size.w),
                    0,
                );
                space.map_output(&output, 1.0, position);

                let new_output_loc = space
                    .outputs()
                    .find(|o| o.name() == output.name())
                    .or_else(|| space.outputs().next())
                    .map(|new_output| space.output_geometry(new_output).unwrap().loc)
                    .unwrap();
                for (window, offset) in windows {
                    let new_pos = new_output_loc + offset;
                    space.map_window(&window, new_pos, true);
                }
            }

            let _device = self.handle.remove(backend_data.registration_token);
            let _device = backend_data.event_dispatcher.into_source_inner();

            // don't use hardware acceleration anymore, if this was the primary gpu
            #[cfg(feature = "egl")]
            if _device.dev_path().and_then(|path| path.canonicalize().ok()) == self.backend_data.primary_gpu {
                backend_data.renderer.borrow_mut().unbind_wl_display();
            }
            debug!(self.log, "Dropping device");
        }
    }

    // If crtc is `Some()`, render it, else render all crtcs
    fn render(&mut self, dev_id: u64, crtc: Option<crtc::Handle>) {
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
            let renderer = &mut *device_backend.renderer.borrow_mut();
            let pointer_images = &mut device_backend.pointer_images;
            let pointer_image = pointer_images
                .iter()
                .find_map(|(image, texture)| if image == &frame { Some(texture) } else { None })
                .cloned()
                .unwrap_or_else(|| {
                    let image =
                        ImageBuffer::from_raw(frame.width, frame.height, &*frame.pixels_rgba).unwrap();
                    let texture = import_bitmap(renderer, &image).expect("Failed to import cursor bitmap");
                    pointer_images.push((frame, texture.clone()));
                    texture
                });

            let result = render_surface(
                &mut *surface.borrow_mut(),
                renderer,
                device_backend.dev_id,
                crtc,
                &mut *self.space.borrow_mut(),
                self.pointer_location,
                &pointer_image,
                #[cfg(feature = "debug")]
                &device_backend.fps_texture,
                &*self.dnd_icon.lock().unwrap(),
                &mut *self.cursor_status.lock().unwrap(),
                &self.start_time,
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
                self.backend_data.render_timer.add_timeout(
                    Duration::from_millis(1000 /*a seconds*/ / 60 /*refresh rate*/),
                    (device_backend.dev_id, crtc),
                );
            }

            // Send frame events so that client start drawing their next frame
            self.space
                .borrow()
                .send_frames(false, self.start_time.elapsed().as_millis() as u32);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_surface(
    surface: &mut SurfaceData,
    renderer: &mut Gles2Renderer,
    device_id: dev_t,
    crtc: crtc::Handle,
    space: &mut Space,
    pointer_location: Point<f64, Logical>,
    pointer_image: &Gles2Texture,
    #[cfg(feature = "debug")] fps_texture: &Gles2Texture,
    dnd_icon: &Option<wl_surface::WlSurface>,
    cursor_status: &mut CursorImageStatus,
    start_time: &std::time::Instant,
    logger: &slog::Logger,
) -> Result<bool, SwapBuffersError> {
    surface.surface.frame_submitted()?;

    let output = if let Some(output) = space
        .outputs()
        .find(|o| o.user_data().get::<UdevOutputId>() == Some(&UdevOutputId { device_id, crtc }))
    {
        output.clone()
    } else {
        // somehow we got called with an invalid output
        return Ok(true);
    };
    let output_geometry = space.output_geometry(&output).unwrap();

    let (dmabuf, age) = surface.surface.next_buffer()?;
    renderer.bind(dmabuf)?;

    let mut elements: Vec<DynamicRenderElements<Gles2Renderer>> = Vec::new();
    // set cursor
    if output_geometry.to_f64().contains(pointer_location) {
        let (ptr_x, ptr_y) = pointer_location.into();
        let relative_ptr_location =
            Point::<i32, Logical>::from((ptr_x as i32, ptr_y as i32)) - output_geometry.loc;
        // draw the dnd icon if applicable
        {
            if let Some(ref wl_surface) = dnd_icon.as_ref() {
                if wl_surface.as_ref().is_alive() {
                    elements.push(Box::new(draw_dnd_icon(
                        (*wl_surface).clone(),
                        relative_ptr_location,
                        logger,
                    )));
                }
            }
        }

        // draw the cursor as relevant
        {
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Image(ref surface) = *cursor_status {
                reset = !surface.as_ref().is_alive();
            }
            if reset {
                *cursor_status = CursorImageStatus::Default;
            }

            if let CursorImageStatus::Image(ref wl_surface) = *cursor_status {
                elements.push(Box::new(draw_cursor(
                    wl_surface.clone(),
                    relative_ptr_location,
                    logger,
                )));
            } else {
                elements.push(Box::new(PointerElement::new(
                    pointer_image.clone(),
                    relative_ptr_location,
                )));
            }
        }

        #[cfg(feature = "debug")]
        {
            elements.push(Box::new(draw_fps(fps_texture, surface.fps.avg().round() as u32)));
            surface.fps.tick();
        }
    }

    // and draw to our buffer
    let render_res = if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        let transform = output.current_transform().into();
        let mode = output.current_mode().unwrap();
        let scale = space.output_scale(&output).unwrap();
        let res = renderer
            .render(mode.size, transform, |renderer, frame| {
                let damage = Rectangle::from_loc_and_size((0, 0), mode.size);
                frame.clear(CLEAR_COLOR, &[damage])?;
                draw_window(
                    renderer,
                    frame,
                    &window,
                    scale,
                    (0, 0),
                    &[damage.to_f64().to_logical(scale).to_i32_round()],
                    &logger,
                )?;
                for elem in elements {
                    let geo = elem.geometry();
                    let damage = [Rectangle::from_loc_and_size((0, 0), geo.size)];
                    elem.draw(renderer, frame, scale, &damage, &logger)?;
                }
                Ok(())
            })
            .and_then(std::convert::identity)
            .map(|_| true)
            .map_err(RenderError::<Gles2Renderer>::Rendering);
        window.send_frame(start_time.elapsed().as_millis() as u32);
        res
    } else {
        space
            .render_output(renderer, &output, age as usize, CLEAR_COLOR, &*elements)
            .map(|x| x.is_some())
        // TODO we can pass the damage rectangles inside a AtomicCommitRequest
    };

    match render_res.map_err(|err| match err {
        RenderError::Rendering(err) => err.into(),
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

fn schedule_initial_render<Data: 'static>(
    surface: Rc<RefCell<SurfaceData>>,
    renderer: Rc<RefCell<Gles2Renderer>>,
    evt_handle: &LoopHandle<'static, Data>,
    logger: ::slog::Logger,
) {
    let result = {
        let mut surface = surface.borrow_mut();
        let mut renderer = renderer.borrow_mut();
        initial_render(&mut surface.surface, &mut *renderer)
    };
    if let Err(err) = result {
        match err {
            SwapBuffersError::AlreadySwapped => {}
            SwapBuffersError::TemporaryFailure(err) => {
                // TODO dont reschedule after 3(?) retries
                warn!(logger, "Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |_| schedule_initial_render(surface, renderer, &handle, logger));
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}

fn initial_render(surface: &mut RenderSurface, renderer: &mut Gles2Renderer) -> Result<(), SwapBuffersError> {
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
