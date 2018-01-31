extern crate drm;
#[macro_use]
extern crate glium;
extern crate rand;
#[macro_use(define_roles)]
extern crate smithay;
extern crate wayland_server;

#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;

mod helpers;

use drm::Device as BasicDevice;
use drm::control::{Device as ControlDevice, ResourceInfo};
use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
use drm::control::crtc;
use drm::control::encoder::Info as EncoderInfo;
use drm::result::Error as DrmError;
use glium::{Blend, Surface};
use helpers::{init_shell, Buffer, GliumDrawer, MyWindowMap, Roles, SurfaceData};
use slog::{Drain, Logger};
use smithay::backend::drm::{drm_device_bind, DrmBackend, DrmDevice, DrmHandler};
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::graphics::egl::wayland::{EGLDisplay, EGLWaylandExtensions, Format};
use smithay::wayland::compositor::{CompositorToken, SubsurfaceRole, TraversalAction};
use smithay::wayland::compositor::roles::Role;
use smithay::wayland::shm::init_shm_global;
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug)]
pub struct Card(File);

impl AsRawFd for Card {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl BasicDevice for Card {}
impl ControlDevice for Card {}

fn main() {
    // A logger facility, here we use the terminal for this example
    let log = Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    // Initialize the wayland server
    let (mut display, mut event_loop) = wayland_server::create_display();

    /*
     * Initialize the drm backend
     */
    // "Find" a suitable drm device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let mut device = DrmDevice::new(
        Card(options.clone().open("/dev/dri/card0").unwrap()),
        log.clone(),
    ).unwrap();

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
    let crtc = encoder_info.current_crtc()
        // or use the first one that is compatible with the encoder
        .unwrap_or_else(||
            *res_handles.filter_crtcs(encoder_info.possible_crtcs())
            .iter()
            .next()
            .unwrap());

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)

    // Initialize the hardware backend
    let renderer = GliumDrawer::from(
        device
            .create_backend(crtc, mode, vec![connector_info.handle()])
            .unwrap(),
    );
    {
        /*
         * Initialize glium
         */
        let mut frame = renderer.draw();
        frame.clear_color(0.8, 0.8, 0.9, 1.0);
        frame.finish().unwrap();
    }

    let egl_display = Rc::new(RefCell::new(
        if let Ok(egl_display) = renderer.bind_wl_display(&display) {
            info!(log, "EGL hardware-acceleration enabled");
            Some(egl_display)
        } else {
            None
        },
    ));

    /*
     * Initialize the globals
     */

    init_shm_global(&mut event_loop, vec![], log.clone());

    let (compositor_token, _shell_state_token, window_map) =
        init_shell(&mut event_loop, log.clone(), egl_display.clone());

    /*
     * Add a listening socket:
     */
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    println!("Listening on socket: {}", name);

    /*
     * Register the DrmDevice on the EventLoop
     */
    let _source = drm_device_bind(
        &mut event_loop,
        device,
        DrmHandlerImpl {
            compositor_token,
            window_map: window_map.clone(),
            drawer: renderer,
            logger: log,
        },
    ).unwrap();

    loop {
        event_loop.dispatch(Some(16)).unwrap();
        display.flush_clients();

        window_map.borrow_mut().refresh();
    }
}

pub struct DrmHandlerImpl {
    compositor_token: CompositorToken<SurfaceData, Roles, Rc<RefCell<Option<EGLDisplay>>>>,
    window_map: Rc<RefCell<MyWindowMap>>,
    drawer: GliumDrawer<DrmBackend<Card>>,
    logger: ::slog::Logger,
}

impl DrmHandler<Card> for DrmHandlerImpl {
    fn ready(
        &mut self, _device: &mut DrmDevice<Card>, _crtc: crtc::Handle, _frame: u32, _duration: Duration
    ) {
        let mut frame = self.drawer.draw();
        frame.clear_color(0.8, 0.8, 0.9, 1.0);
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = self.drawer.get_framebuffer_dimensions();
            self.window_map
                .borrow()
                .with_windows_from_bottom_to_top(|toplevel_surface, initial_place| {
                    if let Some(wl_surface) = toplevel_surface.get_surface() {
                        // this surface is a root of a subsurface tree that needs to be drawn
                        self.compositor_token
                            .with_surface_tree_upward(
                                wl_surface,
                                initial_place,
                                |_surface, attributes, role, &(mut x, mut y)| {
                                    // there is actually something to draw !
                                    if attributes.user_data.texture.is_none() {
                                        let mut remove = false;
                                        match attributes.user_data.buffer {
                                            Some(Buffer::Egl { ref images }) => {
                                                match images.format {
                                                    Format::RGB | Format::RGBA => {
                                                        attributes.user_data.texture =
                                                            self.drawer.texture_from_egl(&images);
                                                    }
                                                    _ => {
                                                        // we don't handle the more complex formats here.
                                                        attributes.user_data.texture = None;
                                                        remove = true;
                                                    }
                                                };
                                            }
                                            Some(Buffer::Shm { ref data, ref size }) => {
                                                attributes.user_data.texture =
                                                    Some(self.drawer.texture_from_mem(data, *size));
                                            }
                                            _ => {}
                                        }
                                        if remove {
                                            attributes.user_data.buffer = None;
                                        }
                                    }

                                    if let Some(ref texture) = attributes.user_data.texture {
                                        if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                                            x += subdata.x;
                                            y += subdata.y;
                                        }
                                        info!(self.logger, "Render window");
                                        self.drawer.render_texture(
                                            &mut frame,
                                            texture,
                                            match *attributes.user_data.buffer.as_ref().unwrap() {
                                                Buffer::Egl { ref images } => images.y_inverted,
                                                Buffer::Shm { .. } => false,
                                            },
                                            match *attributes.user_data.buffer.as_ref().unwrap() {
                                                Buffer::Egl { ref images } => (images.width, images.height),
                                                Buffer::Shm { ref size, .. } => *size,
                                            },
                                            (x, y),
                                            screen_dimensions,
                                            Blend::alpha_blending(),
                                        );
                                        TraversalAction::DoChildren((x, y))
                                    } else {
                                        // we are not display, so our children are neither
                                        TraversalAction::SkipChildren
                                    }
                                },
                            )
                            .unwrap();
                    }
                });
        }
        frame.finish().unwrap();
    }

    fn error(&mut self, _device: &mut DrmDevice<Card>, error: DrmError) {
        panic!("{:?}", error);
    }
}
