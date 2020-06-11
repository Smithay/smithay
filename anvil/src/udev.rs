use std::{
    cell::RefCell,
    collections::hash_map::{Entry, HashMap},
    io::Error as IoError,
    os::unix::io::{AsRawFd, RawFd},
    path::PathBuf,
    rc::Rc,
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use glium::Surface as GliumSurface;
use slog::Logger;

#[cfg(feature = "egl")]
use smithay::backend::egl::{display::EGLBufferReader, EGLGraphicsBackend};
use smithay::{
    backend::{
        drm::{
            atomic::{AtomicDrmDevice, AtomicDrmSurface},
            common::fallback::{FallbackDevice, FallbackSurface},
            device_bind,
            egl::{EglDevice, EglSurface},
            eglstream::{egl::EglStreamDeviceBackend, EglStreamDevice, EglStreamSurface},
            gbm::{egl::Gbm as EglGbmBackend, GbmDevice, GbmSurface},
            legacy::{LegacyDrmDevice, LegacyDrmSurface},
            DevPath, Device, DeviceHandler, Surface,
        },
        graphics::{CursorBackend, SwapBuffersError},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        session::{auto::AutoSession, Session, Signal as SessionSignal},
        udev::{primary_gpu, UdevBackend, UdevEvent},
    },
    reexports::{
        calloop::{
            generic::Generic,
            timer::{Timer, TimerHandle},
            EventLoop, LoopHandle, Source,
        },
        drm::{
            self,
            control::{
                connector::{Info as ConnectorInfo, State as ConnectorState},
                crtc,
                encoder::Info as EncoderInfo,
            },
        },
        image::{ImageBuffer, Rgba, Pixel},
        input::Libinput,
        nix::{fcntl::OFlag, sys::stat::dev_t},
        wayland_server::{
            protocol::{wl_output, wl_surface},
            Display, Global,
        },
    },
    signaling::{Linkable, SignalToken, Signaler},
    utils::Rectangle,
    wayland::{
        compositor::CompositorToken,
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
        SERIAL_COUNTER as SCOUNTER,
    },
};

use crate::buffer_utils::BufferUtils;
use crate::glium_drawer::{schedule_initial_render, GliumDrawer};
use crate::shell::{MyWindowMap, Roles};
use crate::state::AnvilState;

#[derive(Clone)]
pub struct SessionFd(RawFd);
impl AsRawFd for SessionFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

type RenderDevice = FallbackDevice<
    EglDevice<
        EglGbmBackend<FallbackDevice<AtomicDrmDevice<SessionFd>, LegacyDrmDevice<SessionFd>>>,
        GbmDevice<FallbackDevice<AtomicDrmDevice<SessionFd>, LegacyDrmDevice<SessionFd>>>,
    >,
    EglDevice<
        EglStreamDeviceBackend<FallbackDevice<AtomicDrmDevice<SessionFd>, LegacyDrmDevice<SessionFd>>>,
        EglStreamDevice<FallbackDevice<AtomicDrmDevice<SessionFd>, LegacyDrmDevice<SessionFd>>>,
    >,
>;
type RenderSurface = FallbackSurface<
    EglSurface<GbmSurface<FallbackSurface<AtomicDrmSurface<SessionFd>, LegacyDrmSurface<SessionFd>>>>,
    EglSurface<EglStreamSurface<FallbackSurface<AtomicDrmSurface<SessionFd>, LegacyDrmSurface<SessionFd>>>>,
>;

pub fn run_udev(
    display: Rc<RefCell<Display>>,
    event_loop: &mut EventLoop<AnvilState>,
    log: Logger,
) -> Result<(), ()> {
    let name = display
        .borrow_mut()
        .add_socket_auto()
        .unwrap()
        .into_string()
        .unwrap();
    info!(log, "Listening on wayland socket"; "name" => name.clone());
    ::std::env::set_var("WAYLAND_DISPLAY", name);

    #[cfg(feature = "egl")]
    let egl_buffer_reader = Rc::new(RefCell::new(None));

    #[cfg(feature = "egl")]
    let buffer_utils = BufferUtils::new(egl_buffer_reader.clone(), log.clone());
    #[cfg(not(feature = "egl"))]
    let buffer_utils = BufferUtils::new(log.clone());

    /*
     * Initialize session
     */
    let (session, notifier) = AutoSession::new(log.clone()).ok_or(())?;
    let session_signal = notifier.signaler();

    /*
     * Initialize the compositor
     */
    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        buffer_utils,
        Some(session),
        log.clone(),
    );

    /*
     * Initialize the udev backend
     */
    let primary_gpu = primary_gpu(&state.seat_name).unwrap_or_default();

    let bytes = include_bytes!("../resources/cursor2.rgba");
    let udev_backend = UdevBackend::new(state.seat_name.clone(), log.clone()).map_err(|_| ())?;

    let output_map = Rc::new(RefCell::new(Vec::new()));

    let mut udev_handler = UdevHandlerImpl {
        compositor_token: state.ctoken,
        #[cfg(feature = "egl")]
        egl_buffer_reader,
        session: state.session.clone().unwrap(),
        backends: HashMap::new(),
        output_map,
        display: display.clone(),
        primary_gpu,
        window_map: state.window_map.clone(),
        pointer_location: state.pointer_location.clone(),
        pointer_image: ImageBuffer::from_raw(64, 64, bytes.to_vec()).unwrap(),
        cursor_status: state.cursor_status.clone(),
        dnd_icon: state.dnd_icon.clone(),
        loop_handle: event_loop.handle(),
        signaler: session_signal.clone(),
        logger: log.clone(),
    };

    /*
     * Initialize a fake output (we render one screen to every device in this example)
     */
    

    /*
     * Initialize libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<AutoSession>>(
        state.session.clone().unwrap().into(),
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
        udev_handler.device_added(dev, path.into())
    }

    let udev_event_source = event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, _state| match event {
            UdevEvent::Added { device_id, path } => udev_handler.device_added(device_id, path),
            UdevEvent::Changed { device_id } => udev_handler.device_changed(device_id),
            UdevEvent::Removed { device_id } => udev_handler.device_removed(device_id),
        })
        .map_err(|e| -> IoError { e.into() })
        .unwrap();

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
            state.window_map.borrow_mut().refresh();
        }
    }

    // Cleanup stuff
    state.window_map.borrow_mut().clear();

    event_loop.handle().remove(session_event_source);
    event_loop.handle().remove(libinput_event_source);
    event_loop.handle().remove(udev_event_source);

    Ok(())
}

struct MyOutput {
    device_id: dev_t,
    crtc: crtc::Handle,
    size: (u32, u32),
    _wl: Output,
    global: Option<Global<wl_output::WlOutput>>,
}

impl MyOutput {
    fn new(display: &mut Display, device_id: dev_t, crtc: crtc::Handle, conn: ConnectorInfo, logger: ::slog::Logger) -> MyOutput {
        let (output, global) = Output::new(
            display,
            format!("{:?}", conn.interface()),
            PhysicalProperties {
                width: conn.size().unwrap_or((0, 0)).0 as i32,
                height: conn.size().unwrap_or((0, 0)).1 as i32,
                subpixel: wl_output::Subpixel::Unknown,
                make: "Smithay".into(),
                model: "Generic DRM".into(),
            },
            logger,
        );

        let mode = conn.modes()[0];
        let (w, h) = mode.size();
        output.change_current_state(
            Some(Mode {
                width: w as i32,
                height: h as i32,
                refresh: (mode.vrefresh() * 1000) as i32,
            }),
            None,
            None,
        );
        output.set_preferred(Mode {
            width: w as i32,
            height: h as i32,
            refresh: (mode.vrefresh() * 1000) as i32,
        });

        MyOutput {
            device_id,
            crtc,
            size: (w as u32, h as u32),
            _wl: output,
            global: Some(global),
        }
    }
}

impl Drop for MyOutput {
    fn drop(&mut self) {
        self.global.take().unwrap().destroy();
    }
}

struct BackendData {
    _restart_token: SignalToken,
    event_source: Source<Generic<RenderDevice>>,
    surfaces: Rc<RefCell<HashMap<crtc::Handle, Rc<GliumDrawer<RenderSurface>>>>>,
}

struct UdevHandlerImpl<Data: 'static> {
    compositor_token: CompositorToken<Roles>,
    #[cfg(feature = "egl")]
    egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>,
    session: AutoSession,
    backends: HashMap<dev_t, BackendData>,
    display: Rc<RefCell<Display>>,
    primary_gpu: Option<PathBuf>,
    window_map: Rc<RefCell<MyWindowMap>>,
    output_map: Rc<RefCell<Vec<MyOutput>>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    pointer_image: ImageBuffer<Rgba<u8>, Vec<u8>>,
    cursor_status: Arc<Mutex<CursorImageStatus>>,
    dnd_icon: Arc<Mutex<Option<wl_surface::WlSurface>>>,
    loop_handle: LoopHandle<Data>,
    signaler: Signaler<SessionSignal>,
    logger: ::slog::Logger,
}

impl<Data: 'static> UdevHandlerImpl<Data> {
    #[cfg(feature = "egl")]
    pub fn scan_connectors(
        device: &mut RenderDevice,
        egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>,
        display: &mut Display,
        output_map: &mut Vec<MyOutput>,
        logger: &::slog::Logger,
    ) -> HashMap<crtc::Handle, Rc<GliumDrawer<RenderSurface>>> {
        // Get a set of all modesetting resource handles (excluding planes):
        let res_handles = device.resource_handles().unwrap();

        // Use first connected connector
        let connector_infos: Vec<ConnectorInfo> = res_handles
            .connectors()
            .iter()
            .map(|conn| device.get_connector_info(*conn).unwrap())
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
                .flat_map(|encoder_handle| device.get_encoder_info(encoder_handle))
                .collect::<Vec<EncoderInfo>>();
            'outer: for encoder_info in encoder_infos {
                for crtc in res_handles.filter_crtcs(encoder_info.possible_crtcs()) {
                    if let Entry::Vacant(entry) = backends.entry(crtc) {
                        let renderer = GliumDrawer::init(
                            device
                                .create_surface(crtc, connector_info.modes()[0], &[connector_info.handle()])
                                .unwrap(),
                            egl_buffer_reader.clone(),
                            logger.clone(),
                        );
                        output_map.push(MyOutput::new(display, device.device_id(), crtc, connector_info, logger.clone()));

                        entry.insert(Rc::new(renderer));
                        break 'outer;
                    }
                }
            }
        }

        backends
    }

    #[cfg(not(feature = "egl"))]
    pub fn scan_connectors(
        device: &mut RenderDevice,
        display: &mut Display,
        output_map: &mut Vec<MyOutput>,
        logger: &::slog::Logger,
    ) -> HashMap<crtc::Handle, Rc<GliumDrawer<RenderSurface>>> {
        // Get a set of all modesetting resource handles (excluding planes):
        let res_handles = device.resource_handles().unwrap();

        // Use first connected connector
        let connector_infos: Vec<ConnectorInfo> = res_handles
            .connectors()
            .iter()
            .map(|conn| device.get_connector_info(*conn).unwrap())
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
                .flat_map(|encoder_handle| device.get_encoder_info(encoder_handle))
                .collect::<Vec<EncoderInfo>>();
            'outer: for encoder_info in encoder_infos {
                for crtc in res_handles.filter_crtcs(encoder_info.possible_crtcs()) {
                    if !backends.contains_key(&crtc) {
                        let renderer =
                            GliumDrawer::init(device.create_surface(crtc).unwrap(), logger.clone());
                        output_map.push(MyOutput::new(display, device.device_id(), crtc, connector_info, logger.clone()));

                        backends.insert(crtc, Rc::new(renderer));
                        break 'outer;
                    }
                }
            }
        }

        backends
    }
}

impl<Data: 'static> UdevHandlerImpl<Data> {
    fn device_added(&mut self, device_id: dev_t, path: PathBuf) {
        // Try to open the device
        if let Some(mut device) = self
            .session
            .open(
                &path,
                OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOCTTY | OFlag::O_NONBLOCK,
            )
            .ok()
            .and_then(|fd| {
                match FallbackDevice::<AtomicDrmDevice<_>, LegacyDrmDevice<_>>::new(
                    SessionFd(fd),
                    true,
                    self.logger.clone(),
                ) {
                    Ok(drm) => Some(drm),
                    Err(err) => {
                        warn!(self.logger, "Skipping drm device, because of error: {}", err);
                        None
                    }
                }
            })
            .and_then(|drm| {
                match FallbackDevice::<GbmDevice<_>, EglStreamDevice<_>>::new(drm, self.logger.clone()) {
                    Ok(dev) => Some(dev),
                    Err(err) => {
                        warn!(self.logger, "Skipping device, because of error: {}", err);
                        None
                    }
                }
            })
            .and_then(|dev| match FallbackDevice::new_egl(dev, self.logger.clone()) {
                Ok(egl) => Some(egl),
                Err(err) => {
                    warn!(self.logger, "Skipping egl device, because of error: {}", err);
                    None
                }
            })
        {
            // init hardware acceleration on the primary gpu.
            #[cfg(feature = "egl")]
            {
                if path.canonicalize().ok() == self.primary_gpu {
                    *self.egl_buffer_reader.borrow_mut() =
                        device.bind_wl_display(&*self.display.borrow()).ok();
                }
            }

            #[cfg(feature = "egl")]
            let backends = Rc::new(RefCell::new(UdevHandlerImpl::<Data>::scan_connectors(
                &mut device,
                self.egl_buffer_reader.clone(),
                &mut *self.display.borrow_mut(),
                &mut *self.output_map.borrow_mut(),
                &self.logger,
            )));

            #[cfg(not(feature = "egl"))]
            let backends = Rc::new(RefCell::new(UdevHandlerImpl::<Data>::scan_connectors(
                &mut device,
                &mut *self.display.borrow_mut(),
                &mut *self.output_map.borrow_mut(),
                &self.logger,
            )));

            // Set the handler.
            // Note: if you replicate this (very simple) structure, it is rather easy
            // to introduce reference cycles with Rc. Be sure about your drop order
            let renderer = Rc::new(DrmRenderer {
                device_id,
                compositor_token: self.compositor_token,
                backends: backends.clone(),
                window_map: self.window_map.clone(),
                output_map: self.output_map.clone(),
                pointer_location: self.pointer_location.clone(),
                pointer_image: self.pointer_image.clone(),
                cursor_status: self.cursor_status.clone(),
                dnd_icon: self.dnd_icon.clone(),
                logger: self.logger.clone(),
            });
            let mut listener = DrmRendererSessionListener {
                renderer: renderer.clone(),
                loop_handle: self.loop_handle.clone(),
            };
            let restart_token = self.signaler.register(move |signal| match signal {
                SessionSignal::ActivateSession | SessionSignal::ActivateDevice { .. } => listener.activate(),
                _ => {}
            });
            device.set_handler(DrmHandlerImpl {
                renderer,
                loop_handle: self.loop_handle.clone(),
            });

            device.link(self.signaler.clone());
            let dev_id = device.device_id();
            let event_source = device_bind(&self.loop_handle, device)
                .map_err(|e| -> IoError { e.into() })
                .unwrap();

            for renderer in backends.borrow_mut().values() {
                // render first frame
                schedule_initial_render(renderer.clone(), &self.loop_handle);
            }

            self.backends.insert(
                dev_id,
                BackendData {
                    _restart_token: restart_token,
                    event_source,
                    surfaces: backends,
                },
            );
        }
    }

    fn device_changed(&mut self, device: dev_t) {
        //quick and dirty, just re-init all backends
        if let Some(ref mut backend_data) = self.backends.get_mut(&device) {
            let logger = &self.logger;
            let egl_buffer_reader = self.egl_buffer_reader.clone();
            let loop_handle = self.loop_handle.clone();
            let mut display = self.display.borrow_mut();
            let mut output_map = self.output_map.borrow_mut();
            output_map.retain(|output| output.device_id != device);
            self.loop_handle
                .with_source(&backend_data.event_source, |source| {
                    let mut backends = backend_data.surfaces.borrow_mut();
                    #[cfg(feature = "egl")]
                    let new_backends =
                        UdevHandlerImpl::<Data>::scan_connectors(
                            &mut source.file,
                            egl_buffer_reader,
                            &mut *display,
                            &mut *output_map,
                            logger);
                    #[cfg(not(feature = "egl"))]
                    let new_backends = UdevHandlerImpl::<Data>::scan_connectors(
                        &mut source.file,
                        &mut *display,
                        &mut *output_map,
                        logger);
                    *backends = new_backends;

                    for renderer in backends.values() {
                        // render first frame
                        schedule_initial_render(renderer.clone(), &loop_handle);
                    }
                });
        }
    }

    fn device_removed(&mut self, device: dev_t) {
        // drop the backends on this side
        if let Some(backend_data) = self.backends.remove(&device) {
            // drop surfaces
            backend_data.surfaces.borrow_mut().clear();
            debug!(self.logger, "Surfaces dropped");
            // clear outputs
            self.output_map.borrow_mut().retain(|output| output.device_id != device);

            let device = self.loop_handle.remove(backend_data.event_source).unwrap();

            // don't use hardware acceleration anymore, if this was the primary gpu
            #[cfg(feature = "egl")]
            {
                if device.dev_path().and_then(|path| path.canonicalize().ok()) == self.primary_gpu {
                    *self.egl_buffer_reader.borrow_mut() = None;
                }
            }
            debug!(self.logger, "Dropping device");
        }
    }
}

pub struct DrmHandlerImpl<Data: 'static> {
    renderer: Rc<DrmRenderer>,
    loop_handle: LoopHandle<Data>,
}

impl<Data: 'static> DeviceHandler for DrmHandlerImpl<Data> {
    type Device = RenderDevice;

    fn vblank(&mut self, crtc: crtc::Handle) {
        self.renderer.clone().render(crtc, None, Some(&self.loop_handle))
    }

    fn error(&mut self, error: <RenderSurface as Surface>::Error) {
        error!(self.renderer.logger, "{:?}", error);
    }
}

pub struct DrmRendererSessionListener<Data: 'static> {
    renderer: Rc<DrmRenderer>,
    loop_handle: LoopHandle<Data>,
}

impl<Data: 'static> DrmRendererSessionListener<Data> {
    fn activate(&mut self) {
        // we want to be called, after all session handling is done (TODO this is not so nice)
        let renderer = self.renderer.clone();
        let handle = self.loop_handle.clone();
        self.loop_handle
            .insert_idle(move |_| renderer.render_all(Some(&handle)));
    }
}

pub struct DrmRenderer {
    device_id: dev_t,
    compositor_token: CompositorToken<Roles>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Rc<GliumDrawer<RenderSurface>>>>>,
    window_map: Rc<RefCell<MyWindowMap>>,
    output_map: Rc<RefCell<Vec<MyOutput>>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    pointer_image: ImageBuffer<Rgba<u8>, Vec<u8>>,
    cursor_status: Arc<Mutex<CursorImageStatus>>,
    dnd_icon: Arc<Mutex<Option<wl_surface::WlSurface>>>,
    logger: ::slog::Logger,
}

impl DrmRenderer {
    fn render_all<Data: 'static>(self: Rc<Self>, evt_handle: Option<&LoopHandle<Data>>) {
        for crtc in self.backends.borrow().keys() {
            self.clone().render(*crtc, None, evt_handle);
        }
    }
    fn render<Data: 'static>(
        self: Rc<Self>,
        crtc: crtc::Handle,
        timer: Option<TimerHandle<(std::rc::Weak<DrmRenderer>, crtc::Handle)>>,
        evt_handle: Option<&LoopHandle<Data>>,
    ) {
        if let Some(drawer) = self.backends.borrow().get(&crtc) {
            // get output coordinates
            let (x, y) = self.output_map.borrow()
                .iter()
                .take_while(|output| output.device_id != self.device_id || output.crtc != crtc)
                .fold((0u32, 0u32), |pos, output| (pos.0 + output.size.0, pos.1));
            let (width, height) = self.output_map.borrow()
                .iter()
                .find(|output| output.device_id == self.device_id && output.crtc == crtc)
                .map(|output| output.size)
                .unwrap_or((0, 0)); // in this case the output will be removed.
            
            // and draw in sync with our monitor
            let mut frame = drawer.draw();
            frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);
            // draw the surfaces
            drawer.draw_windows(
                &mut frame,
                &*self.window_map.borrow(),
                Some(Rectangle {
                    x: x as i32, y: y as i32,
                    width: width as i32,
                    height: height as i32,
                }),
                self.compositor_token,
            );

            // get pointer coordinates
            let (ptr_x, ptr_y) = *self.pointer_location.borrow();
            let ptr_x = ptr_x.trunc().abs() as i32 - x as i32;
            let ptr_y = ptr_y.trunc().abs() as i32 - y as i32;

            // set cursor
            // TODO hack, should be possible to clear the cursor
            let empty = ImageBuffer::from_fn(self.pointer_image.width(), self.pointer_image.height(), |_, _| Rgba::from_channels(0, 0, 0, 0));
            if ptr_x > 0 && ptr_x < width as i32 && ptr_y > 0 && ptr_y < height as i32 {
                let _ = drawer
                    .borrow()
                    .set_cursor_position(ptr_x as u32, ptr_y as u32);
                
                let mut software_cursor = false;

                // draw the dnd icon if applicable
                {
                    let guard = self.dnd_icon.lock().unwrap();
                    if let Some(ref surface) = *guard {
                        if surface.as_ref().is_alive() {
                            drawer.draw_dnd_icon(
                                &mut frame,
                                surface,
                                (ptr_x, ptr_y),
                                self.compositor_token,
                            );
                            software_cursor = true;
                        }
                    }
                }
                // draw the cursor as relevant
                {
                    let mut guard = self.cursor_status.lock().unwrap();
                    // reset the cursor if the surface is no longer alive
                    let mut reset = false;
                    if let CursorImageStatus::Image(ref surface) = *guard {
                        reset = !surface.as_ref().is_alive();
                    }
                    if reset {
                        *guard = CursorImageStatus::Default;
                    }
                    if let CursorImageStatus::Image(ref surface) = *guard {
                        drawer.draw_cursor(&mut frame, surface, (ptr_x, ptr_y), self.compositor_token);
                        software_cursor = true;
                    }
                }

                // TODO: this is wasteful, only do this on change (cache previous software cursor state)
                if software_cursor {
                    if let Err(err) = drawer.borrow().set_cursor_representation(&empty, (2, 2)) {
                        error!(self.logger, "Error setting cursor: {}", err);
                    }
                } else {
                    if let Err(err) = drawer.borrow().set_cursor_representation(&self.pointer_image, (2, 2)) {
                        error!(self.logger, "Error setting cursor: {}", err);
                    }
                }
            } else {
                if let Err(err) = drawer.borrow().set_cursor_representation(&empty, (2, 2)) {
                    error!(self.logger, "Error setting cursor: {}", err);
                }
            }

            if let Err(err) = frame.finish() {
                warn!(self.logger, "Error during rendering: {:?}", err);
                let reschedule = match err {
                    SwapBuffersError::AlreadySwapped => false,
                    SwapBuffersError::TemporaryFailure(err) => {
                        match err.downcast_ref::<smithay::backend::drm::common::Error>() {
                            Some(&smithay::backend::drm::common::Error::DeviceInactive) => false,
                            Some(&smithay::backend::drm::common::Error::Access { ref source, .. })
                                if match source.get_ref() {
                                    drm::SystemError::PermissionDenied => true,
                                    _ => false,
                                } =>
                            {
                                false
                            }
                            _ => true,
                        }
                    }
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
                };

                if reschedule {
                    debug!(self.logger, "Rescheduling");
                    match (timer, evt_handle) {
                        (Some(handle), _) => {
                            let _ = handle.add_timeout(
                                Duration::from_millis(1000 /*a seconds*/ / 60 /*refresh rate*/),
                                (Rc::downgrade(&self), crtc),
                            );
                        }
                        (None, Some(evt_handle)) => {
                            let timer = Timer::new().unwrap();
                            let handle = timer.handle();
                            let _ = handle.add_timeout(
                                Duration::from_millis(1000 /*a seconds*/ / 60 /*refresh rate*/),
                                (Rc::downgrade(&self), crtc),
                            );
                            evt_handle
                                .insert_source(timer, |(renderer, crtc), handle, _data| {
                                    if let Some(renderer) = renderer.upgrade() {
                                        renderer.render(
                                            crtc,
                                            Some(handle.clone()),
                                            Option::<&LoopHandle<Data>>::None,
                                        );
                                    }
                                })
                                .unwrap();
                        }
                        _ => unreachable!(),
                    }
                }
            } else {
                // TODO: only send drawn windows the frames callback
                // Send frame events so that client start drawing their next frame
                self.window_map.borrow().send_frames(SCOUNTER.next_serial());
            }
        }
    }
}
