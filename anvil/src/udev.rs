use std::{
    cell::RefCell,
    collections::HashMap,
    io::Error as IoError,
    os::unix::io::{AsRawFd, RawFd},
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use glium::Surface as GliumSurface;
use slog::Logger;

use smithay::{
    backend::{
        drm::{
            dev_t,
            egl::{EglDevice, EglSurface},
            gbm::{egl::Gbm as EglGbmBackend, GbmDevice},
            legacy::LegacyDrmDevice,
            DevPath, Device, DeviceHandler, Surface,
        },
        egl::{EGLDisplay, EGLGraphicsBackend},
        graphics::CursorBackend,
        input::InputBackend,
        libinput::{libinput_bind, LibinputInputBackend, LibinputSessionInterface},
        session::{
            auto::{auto_session_bind, AutoSession},
            OFlag, Session, SessionNotifier,
        },
        udev::{primary_gpu, udev_backend_bind, UdevBackend, UdevHandler},
    },
    drm::control::{
        connector::{Info as ConnectorInfo, State as ConnectorState},
        crtc,
        encoder::Info as EncoderInfo,
        ResourceInfo,
    },
    image::{ImageBuffer, Rgba},
    input::Libinput,
    wayland::{
        compositor::CompositorToken,
        data_device::{default_action_chooser, init_data_device, set_data_device_focus},
        output::{Mode, Output, PhysicalProperties},
        seat::{Seat, XkbConfig},
        shm::init_shm_global,
    },
    wayland_server::{calloop::EventLoop, protocol::wl_output, Display},
};

use glium_drawer::GliumDrawer;
use input_handler::AnvilInputHandler;
use shell::{init_shell, MyWindowMap, Roles, SurfaceData};

pub struct SessionFd(RawFd);
impl AsRawFd for SessionFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

type RenderDevice =
    EglDevice<EglGbmBackend<LegacyDrmDevice<SessionFd>>, GbmDevice<LegacyDrmDevice<SessionFd>>>;
type RenderSurface =
    EglSurface<EglGbmBackend<LegacyDrmDevice<SessionFd>>, GbmDevice<LegacyDrmDevice<SessionFd>>>;

pub fn run_udev(mut display: Display, mut event_loop: EventLoop<()>, log: Logger) -> Result<(), ()> {
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    info!(log, "Listening on wayland socket"; "name" => name.clone());
    ::std::env::set_var("WAYLAND_DISPLAY", name);

    let active_egl_context = Rc::new(RefCell::new(None));

    let display = Rc::new(RefCell::new(display));

    /*
     * Initialize the compositor
     */
    init_shm_global(&mut display.borrow_mut(), vec![], log.clone());

    let (compositor_token, _, _, window_map) = init_shell(&mut display.borrow_mut(), log.clone());

    /*
     * Initialize session
     */
    let (session, mut notifier) = AutoSession::new(log.clone()).ok_or(())?;

    let running = Arc::new(AtomicBool::new(true));

    let pointer_location = Rc::new(RefCell::new((0.0, 0.0)));

    /*
     * Initialize the udev backend
     */
    let context = ::smithay::udev::Context::new().map_err(|_| ())?;
    let seat = session.seat();

    let primary_gpu = primary_gpu(&context, &seat).unwrap_or_default();

    let bytes = include_bytes!("../resources/cursor2.rgba");
    let udev_backend = UdevBackend::new(
        &context,
        UdevHandlerImpl {
            compositor_token,
            active_egl_context,
            session: session.clone(),
            backends: HashMap::new(),
            display: display.clone(),
            primary_gpu,
            window_map: window_map.clone(),
            pointer_location: pointer_location.clone(),
            pointer_image: ImageBuffer::from_raw(64, 64, bytes.to_vec()).unwrap(),
            logger: log.clone(),
        },
        seat.clone(),
        log.clone(),
    ).map_err(|_| ())?;

    init_data_device(
        &mut display.borrow_mut(),
        |_| {},
        default_action_chooser,
        log.clone(),
    );

    let (mut w_seat, _) = Seat::new(&mut display.borrow_mut(), session.seat(), log.clone());

    let pointer = w_seat.add_pointer();
    let keyboard = w_seat
        .add_keyboard(XkbConfig::default(), 1000, 500, |seat, focus| {
            set_data_device_focus(seat, focus.and_then(|s| s.client()))
        }).expect("Failed to initialize the keyboard");

    let (output, _output_global) = Output::new(
        &mut display.borrow_mut(),
        "Drm".into(),
        PhysicalProperties {
            width: 0,
            height: 0,
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Generic DRM".into(),
        },
        log.clone(),
    );

    let (w, h) = (1920, 1080); // Hardcode full-hd res
    output.change_current_state(
        Some(Mode {
            width: w as i32,
            height: h as i32,
            refresh: 60_000,
        }),
        None,
        None,
    );
    output.set_preferred(Mode {
        width: w as i32,
        height: h as i32,
        refresh: 60_000,
    });

    /*
     * Initialize libinput backend
     */
    let mut libinput_context =
        Libinput::new_from_udev::<LibinputSessionInterface<AutoSession>>(session.clone().into(), &context);
    let libinput_session_id = notifier.register(&mut libinput_context);
    libinput_context.udev_assign_seat(&seat).unwrap();
    let mut libinput_backend = LibinputInputBackend::new(libinput_context, log.clone());
    libinput_backend.set_handler(AnvilInputHandler::new_with_session(
        log.clone(),
        pointer,
        keyboard,
        window_map.clone(),
        (w, h),
        running.clone(),
        pointer_location,
        session,
    ));
    let libinput_event_source = libinput_bind(libinput_backend, event_loop.handle())
        .map_err(|e| -> IoError { e.into() })
        .unwrap();
    let session_event_source = auto_session_bind(notifier, &event_loop.handle())
        .map_err(|(e, _)| e)
        .unwrap();
    let udev_event_source = udev_backend_bind(udev_backend, &event_loop.handle())
        .map_err(|e| -> IoError { e.into() })
        .unwrap();

    while running.load(Ordering::SeqCst) {
        if event_loop
            .dispatch(Some(::std::time::Duration::from_millis(16)), &mut ())
            .is_err()
        {
            running.store(false, Ordering::SeqCst);
        } else {
            display.borrow_mut().flush_clients();
            window_map.borrow_mut().refresh();
        }
    }

    let mut notifier = session_event_source.unbind();
    notifier.unregister(libinput_session_id);

    libinput_event_source.remove();
    udev_event_source.remove();

    Ok(())
}

struct UdevHandlerImpl {
    compositor_token: CompositorToken<SurfaceData, Roles>,
    active_egl_context: Rc<RefCell<Option<EGLDisplay>>>,
    session: AutoSession,
    backends: HashMap<
        dev_t,
        (
            RenderDevice,
            Rc<RefCell<HashMap<crtc::Handle, GliumDrawer<RenderSurface>>>>,
        ),
    >,
    display: Rc<RefCell<Display>>,
    primary_gpu: Option<PathBuf>,
    window_map: Rc<RefCell<MyWindowMap>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    pointer_image: ImageBuffer<Rgba<u8>, Vec<u8>>,
    logger: ::slog::Logger,
}

impl UdevHandlerImpl {
    pub fn scan_connectors(
        device: &mut RenderDevice,
        egl_display: Rc<RefCell<Option<EGLDisplay>>>,
        pointer_image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
        logger: &::slog::Logger,
    ) -> HashMap<crtc::Handle, GliumDrawer<RenderSurface>> {
        // Get a set of all modesetting resource handles (excluding planes):
        let res_handles = device.resource_handles().unwrap();

        // Use first connected connector
        let connector_infos: Vec<ConnectorInfo> = res_handles
            .connectors()
            .iter()
            .map(|conn| device.resource_info::<ConnectorInfo>(*conn).unwrap())
            .filter(|conn| conn.connection_state() == ConnectorState::Connected)
            .inspect(|conn| info!(logger, "Connected: {:?}", conn.connector_type()))
            .collect();

        let mut backends = HashMap::new();

        // very naive way of finding good crtc/encoder/connector combinations. This problem is np-complete
        for connector_info in connector_infos {
            let encoder_infos = connector_info
                .encoders()
                .iter()
                .flat_map(|encoder_handle| device.resource_info::<EncoderInfo>(*encoder_handle))
                .collect::<Vec<EncoderInfo>>();
            for encoder_info in encoder_infos {
                for crtc in res_handles.filter_crtcs(encoder_info.possible_crtcs()) {
                    if !backends.contains_key(&crtc) {
                        let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)
                                                              // create a backend
                        let renderer = GliumDrawer::init(
                            device
                                .create_surface(crtc, mode, vec![connector_info.handle()].into_iter())
                                .unwrap(),
                            egl_display.clone(),
                            logger.clone(),
                        );

                        // create cursor
                        renderer
                            .borrow()
                            .set_cursor_representation(pointer_image, (2, 2))
                            .unwrap();

                        // render first frame
                        {
                            let mut frame = renderer.draw();
                            frame.clear_color(0.8, 0.8, 0.9, 1.0);
                            frame.finish().unwrap();
                        }

                        backends.insert(crtc, renderer);
                        break;
                    }
                }
            }
        }

        backends
    }
}

impl UdevHandler for UdevHandlerImpl {
    fn device_added(&mut self, _device: dev_t, path: PathBuf) {
        if let Some(mut device) = self
            .session
            .open(
                &path,
                OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOCTTY | OFlag::O_NONBLOCK,
            ).ok()
            .and_then(|fd| LegacyDrmDevice::new(SessionFd(fd), self.logger.clone()).ok())
            .and_then(|drm| GbmDevice::new(drm, self.logger.clone()).ok())
            .and_then(|gbm| EglDevice::new(gbm, self.logger.clone()).ok())
        {
            // init hardware acceleration on the primary gpu.
            if path.canonicalize().ok() == self.primary_gpu {
                *self.active_egl_context.borrow_mut() = device.bind_wl_display(&*self.display.borrow()).ok();
            }

            let backends = Rc::new(RefCell::new(UdevHandlerImpl::scan_connectors(
                &mut device,
                self.active_egl_context.clone(),
                &self.pointer_image,
                &self.logger,
            )));

            device.set_handler(DrmHandlerImpl {
                compositor_token: self.compositor_token,
                backends: backends.clone(),
                window_map: self.window_map.clone(),
                pointer_location: self.pointer_location.clone(),
                logger: self.logger.clone(),
            });

            self.backends.insert(device.device_id(), (device, backends));
        }
    }

    fn device_changed(&mut self, device: dev_t) {
        //quick and dirty, just re-init all backends
        if let Some((ref mut device, ref backends)) = self.backends.get_mut(&device) {
            *backends.borrow_mut() = UdevHandlerImpl::scan_connectors(
                device,
                self.active_egl_context.clone(),
                &self.pointer_image,
                &self.logger,
            );
        }
    }

    fn device_removed(&mut self, device: dev_t) {
        // drop the backends on this side
        if let Some((dev, _)) = self.backends.remove(&device) {
            // don't use hardware acceleration anymore, if this was the primary gpu
            if dev.dev_path().and_then(|path| path.canonicalize().ok()) == self.primary_gpu {
                *self.active_egl_context.borrow_mut() = None;
            }
        }
    }
}

pub struct DrmHandlerImpl {
    compositor_token: CompositorToken<SurfaceData, Roles>,
    backends: Rc<RefCell<HashMap<crtc::Handle, GliumDrawer<RenderSurface>>>>,
    window_map: Rc<RefCell<MyWindowMap>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    logger: ::slog::Logger,
}

impl DeviceHandler for DrmHandlerImpl {
    type Device = RenderDevice;

    fn vblank(&mut self, crtc: crtc::Handle) {
        if let Some(drawer) = self.backends.borrow().get(&crtc) {
            {
                let (x, y) = *self.pointer_location.borrow();
                let _ = drawer
                    .borrow()
                    .set_cursor_position(x.trunc().abs() as u32, y.trunc().abs() as u32);
            }

            drawer.draw_windows(&*self.window_map.borrow(), self.compositor_token, &self.logger);
        }
    }

    fn error(&mut self, error: <RenderSurface as Surface>::Error) {
        error!(self.logger, "{:?}", error);
    }
}
