use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    os::unix::io::{AsRawFd, RawFd},
    rc::Rc,
    time::Duration,
};

use smithay::{
    backend::{
        drm::{drm_device_bind, DrmBackend, DrmDevice, DrmHandler},
        graphics::egl::wayland::EGLWaylandExtensions,
    },
    drm::{
        control::{
            connector::{Info as ConnectorInfo, State as ConnectorState},
            crtc,
            encoder::Info as EncoderInfo,
            Device as ControlDevice, ResourceInfo,
        },
        result::Error as DrmError,
        Device as BasicDevice,
    },
    wayland::{compositor::CompositorToken, shm::init_shm_global},
    wayland_server::{calloop::EventLoop, Display},
};

use glium::Surface;
use slog::Logger;

use glium_drawer::GliumDrawer;
use shell::{init_shell, MyWindowMap, Roles, SurfaceData};

#[derive(Debug)]
pub struct Card(File);

impl AsRawFd for Card {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl BasicDevice for Card {}
impl ControlDevice for Card {}

pub fn run_raw_drm(mut display: Display, mut event_loop: EventLoop<()>, log: Logger) -> Result<(), ()> {
    /*
     * Initialize the drm backend
     */
    // "Find" a suitable drm device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let mut device =
        DrmDevice::new(Card(options.clone().open("/dev/dri/card0").unwrap()), log.clone()).unwrap();

    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = device.resource_handles().unwrap();

    // Use first connected connector
    let connector_info = res_handles
        .connectors()
        .iter()
        .map(|conn| ConnectorInfo::load_from_device(&device, *conn).unwrap())
        .find(|conn| conn.connection_state() == ConnectorState::Connected)
        .unwrap();

    // Use the first encoder
    let encoder_info = EncoderInfo::load_from_device(&device, connector_info.encoders()[0]).unwrap();

    // use the connected crtc if any
    let crtc = encoder_info
        .current_crtc()
        // or use the first one that is compatible with the encoder
        .unwrap_or_else(|| {
            *res_handles
                .filter_crtcs(encoder_info.possible_crtcs())
                .iter()
                .next()
                .unwrap()
        });

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)

    // Initialize the hardware backend
    let backend = device
        .create_backend(crtc, mode, vec![connector_info.handle()])
        .unwrap();
    let egl_display = Rc::new(RefCell::new(
        if let Ok(egl_display) = backend.bind_wl_display(&display) {
            info!(log, "EGL hardware-acceleration enabled");
            Some(egl_display)
        } else {
            None
        },
    ));
    let renderer = GliumDrawer::init(backend, egl_display, log.clone());
    {
        /*
         * Initialize glium
         */
        let mut frame = renderer.draw();
        frame.clear_color(0.8, 0.8, 0.9, 1.0);
        frame.finish().unwrap();
    }

    /*
     * Initialize the globals
     */

    init_shm_global(&mut display, vec![], log.clone());

    let (compositor_token, _, _, window_map) = init_shell(&mut display, log.clone());

    /*
     * Add a listening socket:
     */
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    println!("Listening on socket: {}", name);

    /*
     * Register the DrmDevice on the EventLoop
     */
    let _source = drm_device_bind(
        &event_loop.handle(),
        device,
        DrmHandlerImpl {
            compositor_token,
            window_map: window_map.clone(),
            drawer: renderer,
            logger: log,
        },
    ).map_err(|(err, _)| err)
    .unwrap();

    loop {
        event_loop
            .dispatch(Some(::std::time::Duration::from_millis(16)), &mut ())
            .unwrap();
        display.flush_clients();

        window_map.borrow_mut().refresh();
    }
}

pub struct DrmHandlerImpl {
    compositor_token: CompositorToken<SurfaceData, Roles>,
    window_map: Rc<RefCell<MyWindowMap>>,
    drawer: GliumDrawer<DrmBackend<Card>>,
    logger: ::slog::Logger,
}

impl DrmHandler<Card> for DrmHandlerImpl {
    fn ready(
        &mut self,
        _device: &mut DrmDevice<Card>,
        _crtc: crtc::Handle,
        _frame: u32,
        _duration: Duration,
    ) {
        self.drawer
            .draw_windows(&*self.window_map.borrow(), self.compositor_token, &self.logger);
    }

    fn error(&mut self, _device: &mut DrmDevice<Card>, error: DrmError) {
        panic!("{:?}", error);
    }
}
