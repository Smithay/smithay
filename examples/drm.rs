#[macro_use(server_declare_handler)]
extern crate wayland_server;
extern crate smithay;
#[macro_use]
extern crate glium;
extern crate drm;
extern crate gbm;
extern crate libudev;
extern crate input;
extern crate nix;

#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;

mod helpers;

use drm::control::{Device as ControlDevice, ResourceInfo};
use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
use gbm::Device as GbmDevice;

use glium::Surface;

use helpers::{GliumDrawer, WlShellStubHandler};
use slog::*;

use smithay::backend::drm::{DrmDevice, DrmBackend, DrmHandler};
use smithay::backend::graphics::GraphicsBackend;
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::graphics::glium::{IntoGlium, GliumGraphicsBackend};
use smithay::backend::libinput::{LibinputInputBackend, PointerAxisEvent};
use smithay::backend::input::{InputBackend, InputHandler, Seat, PointerMotionEvent, PointerMotionAbsoluteEvent};
use smithay::compositor::{self, CompositorHandler, CompositorToken, TraversalAction};
use smithay::shm::{BufferData, ShmGlobal, ShmToken};

use nix::libc;
use input::{Libinput, LibinputInterface, Device as InputDevice, event};
use libudev::Context as Udev;
use libudev::handle::Handle as UdevHandle;

use wayland_server::{Client, Display, EventLoopHandle, Liveness, Resource};

use wayland_server::protocol::{wl_compositor, wl_shell, wl_shm, wl_subcompositor, wl_surface};

use std::fs::{OpenOptions, File};
use std::io::Error as IoError;
use std::sync::Mutex;
use std::rc::Rc;
use std::os::unix::io::AsRawFd;

struct SurfaceHandler {
    shm_token: ShmToken,
}

#[derive(Default)]
struct SurfaceData {
    buffer: Option<(Vec<u8>, (u32, u32))>,
}

impl compositor::Handler<SurfaceData> for SurfaceHandler {
    fn commit(&mut self, evlh: &mut EventLoopHandle, client: &Client, surface: &wl_surface::WlSurface,
              token: CompositorToken<SurfaceData, SurfaceHandler>) {
        // we retrieve the contents of the associated buffer and copy it
        token.with_surface_data(surface, |attributes| {
            match attributes.buffer.take() {
                Some(Some((buffer, (x, y)))) => {
                    self.shm_token.with_buffer_contents(&buffer, |slice, data| {
                        let offset = data.offset as usize;
                        let stride = data.stride as usize;
                        let width = data.width as usize;
                        let height = data.height as usize;
                        let mut new_vec = Vec::with_capacity(width * height * 4);
                        for i in 0..height {
                            new_vec.extend(
                                &slice[(offset + i * stride)..(offset + i * stride + width * 4)],
                            );
                        }
                        attributes.user_data.buffer =
                            Some((new_vec, (data.width as u32, data.height as u32)));
                    });

                }
                Some(None) => {
                    // erase the contents
                    attributes.user_data.buffer = None;
                }
                None => {}
            }
        });
    }
}

unsafe extern "C" fn open_restricted(path: *const i8, flags: i32, userdata: *mut libc::c_void) -> i32 {
    libc::open(path, flags)
}

unsafe extern "C" fn close_restricted(fd: i32, userdata: *mut libc::c_void) {
    libc::close(fd);
}

fn main() {
    // A logger facility, here we use the terminal for this example
    let log = Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    // Initialize the wayland server
    let (mut display, mut event_loop) = wayland_server::create_display();

    let udev = Udev::new().unwrap();
    let mut input = LibinputInputBackend::new(unsafe {
        Libinput::new_from_udev(LibinputInterface {
            open_restricted: Some(open_restricted),
            close_restricted: Some(close_restricted),
        }, None::<()>, udev.as_ptr() as *mut _)
    }, log.clone());

    // "Find" a suitable drm device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let device = DrmDevice::new_from_file(options.clone().open("/dev/dri/card0").unwrap());

    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = device.resource_handles().unwrap();

    // Use first connected connector
    let connector_info = res_handles.connectors().iter().map(|conn| {
        ConnectorInfo::load_from_device(&device, *conn).unwrap()
    }).find(|conn| conn.connection_state() == ConnectorState::Connected).unwrap();

    // Use the first crtc (should be successful in most cases)
    let crtc = res_handles.crtcs()[0];

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)

    // Also get a gbm device (usually the same)
    let gbm = unsafe { GbmDevice::new_from_fd(device.as_raw_fd()).unwrap() };

    // Initialize the hardware backends
    let renderer = DrmBackend::new(device, crtc, mode, vec![connector_info.handle()], gbm, log.clone()).unwrap();

        /*
     * Initialize wl_shm global
     */
    // Insert the ShmGlobal as a handler to your event loop
    // Here, we specify tha the standard Argb8888 and Xrgb8888 is the only supported.
    let shm_handler_id = event_loop.add_handler_with_init(ShmGlobal::new(vec![], log.clone()));
    // Register this handler to advertise a wl_shm global of version 1
    event_loop.register_global::<wl_shm::WlShm, ShmGlobal>(shm_handler_id, 1);
    // retreive the token
    let shm_token = {
        let state = event_loop.state();
        state.get_handler::<ShmGlobal>(shm_handler_id).get_token()
    };


    /*
     * Initialize the compositor global
     */
    let compositor_handler_id = event_loop.add_handler_with_init(CompositorHandler::<SurfaceData, _>::new(
        SurfaceHandler { shm_token: shm_token.clone() },
        log.clone(),
    ));
    // register it to handle wl_compositor and wl_subcompositor
    event_loop
        .register_global::<wl_compositor::WlCompositor, CompositorHandler<SurfaceData, SurfaceHandler>>(
            compositor_handler_id,
            4,
        );
    event_loop.register_global::<wl_subcompositor::WlSubcompositor, CompositorHandler<SurfaceData,SurfaceHandler>>(compositor_handler_id, 1);
    // retrieve the tokens
    let compositor_token = {
        let state = event_loop.state();
        state
            .get_handler::<CompositorHandler<SurfaceData, SurfaceHandler>>(compositor_handler_id)
            .get_token()
    };

    /*
     * Initialize the shell stub global
     */
    let shell_handler_id =
        event_loop.add_handler_with_init(WlShellStubHandler::new(compositor_token.clone()));
    event_loop.register_global::<wl_shell::WlShell, WlShellStubHandler<SurfaceData, SurfaceHandler>>(
        shell_handler_id,
        1,
    );

    /*
     * Initialize glium
     */
    let drawer = Rc::new(Mutex::new(GliumDrawer::new(renderer.into_glium())));

    /*
     * Add a listening socket:
     */
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    println!("Listening on socket: {}", name);

    let _drm_event_source = drawer.lock().unwrap().register(&mut event_loop, DrmHandlerImpl {
        drawer: drawer.clone(),
        shell_handler_id,
        compositor_token,
        logger: log,
    }).unwrap();

    input.set_handler(InputHandlerImpl {
        drawer: drawer.clone(),
        position: (0, 0),
    });
    let _input_event_source = input.register(&mut event_loop);

    event_loop.run().unwrap();
}

struct InputHandlerImpl {
    drawer: Rc<Mutex<GliumDrawer<GliumGraphicsBackend<DrmBackend>>>>,
    position: (u32, u32),
}

impl InputHandlerImpl {
    fn set_pointer(&mut self) {
        //self.drawer.lock().unwrap().set_cursor_position(self.position.0, self.position.1).unwrap()
    }
}

impl InputHandler<LibinputInputBackend> for InputHandlerImpl {
    fn on_seat_created(&mut self, seat: &Seat) {}
    fn on_seat_destroyed(&mut self, seat: &Seat) {}
    fn on_seat_changed(&mut self, seat: &Seat) {}

    fn on_keyboard_key(&mut self, seat: &Seat, event: event::keyboard::KeyboardKeyEvent) {}

    fn on_pointer_move(&mut self, seat: &Seat, event: event::pointer::PointerMotionEvent) {
        self.position.0 += event.delta_x();
        self.position.1 += event.delta_y();
        self.set_pointer()
    }
    fn on_pointer_move_absolute(&mut self, seat: &Seat, event: event::pointer::PointerMotionAbsoluteEvent) {
        self.position = event.position_transformed(self.drawer.lock().unwrap().get_framebuffer_dimensions());
        self.set_pointer()
    }
    fn on_pointer_button(&mut self, seat: &Seat, event: event::pointer::PointerButtonEvent) {}
    fn on_pointer_axis(&mut self, seat: &Seat, event: PointerAxisEvent) {}

    fn on_touch_down(&mut self, seat: &Seat, event: event::touch::TouchDownEvent) {}
    fn on_touch_motion(&mut self, seat: &Seat, event: event::touch::TouchMotionEvent) {}
    fn on_touch_up(&mut self, seat: &Seat, event: event::touch::TouchUpEvent) {}
    fn on_touch_cancel(&mut self, seat: &Seat, event: event::touch::TouchCancelEvent) {}
    fn on_touch_frame(&mut self, seat: &Seat, event: event::touch::TouchFrameEvent) {}
    fn on_input_config_changed(&mut self, config: &mut [input::Device]) {}
}

pub struct DrmHandlerImpl {
    drawer: Rc<Mutex<GliumDrawer<GliumGraphicsBackend<DrmBackend>>>>,
    shell_handler_id: usize,
    compositor_token: CompositorToken<SurfaceData,SurfaceHandler>,
    logger: ::slog::Logger,
}

impl DrmHandler for DrmHandlerImpl {
    fn ready(&mut self, evlh: &mut EventLoopHandle) {
        let drawer = self.drawer.lock().unwrap();

        drawer.prepare_rendering();

        let mut frame = drawer.draw();
        frame.clear_color(0.8, 0.8, 0.9, 1.0);
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = drawer.get_framebuffer_dimensions();
            for &(_, ref surface) in
                unsafe { evlh
                    .get_handler_unchecked::<WlShellStubHandler<SurfaceData, SurfaceHandler>>(self.shell_handler_id)
                    .surfaces() }
            {
                if surface.status() != Liveness::Alive {
                    continue;
                }
                // this surface is a root of a subsurface tree that needs to be drawn
                self.compositor_token.with_surface_tree(
                    surface,
                    (100, 100),
                    |surface, attributes, &(mut x, mut y)| {
                        if let Some((ref contents, (w, h))) = attributes.user_data.buffer {
                            // there is actually something to draw !
                            if let Some(ref subdata) = attributes.subsurface_attributes {
                                x += subdata.x;
                                y += subdata.y;
                            }
                            drawer.render(&mut frame, contents, (w, h), (x, y), screen_dimensions);
                            TraversalAction::DoChildren((x, y))
                        } else {
                            // we are not display, so our children are neither
                            TraversalAction::SkipChildren
                        }
                    },
                );
            }
        }
        frame.finish().unwrap();

        drawer.finish_rendering();
    }

    fn error(&mut self, _evlh: &mut EventLoopHandle, error: IoError) {
        error!(self.logger, "{:?}", error);
    }
}
