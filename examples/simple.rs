#[macro_use]
extern crate glium;
extern crate rand;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
#[macro_use(define_roles)]
extern crate smithay;
extern crate wayland_protocols;
extern crate wayland_server;

mod helpers;

use glium::Surface;

use helpers::GliumDrawer;
use slog::{Drain, Logger};

use smithay::backend::graphics::glium::IntoGlium;
use smithay::backend::input::InputBackend;
use smithay::backend::winit;
use smithay::compositor::{self, CompositorHandler, CompositorToken, SubsurfaceRole, TraversalAction};
use smithay::compositor::roles::Role;
use smithay::shell::{self, PopupConfigure, PopupSurface, ShellClient, ShellHandler, ShellSurfaceRole,
                     ToplevelConfigure, ToplevelSurface};
use smithay::shm::{ShmGlobal, ShmToken};

use wayland_protocols::unstable::xdg_shell::server::{zxdg_shell_v6, zxdg_toplevel_v6};

use wayland_server::{Client, EventLoopHandle};
use wayland_server::protocol::{wl_compositor, wl_output, wl_seat, wl_shell, wl_shm, wl_subcompositor,
                               wl_surface};

define_roles!(Roles => [ ShellSurface, ShellSurfaceRole ] );

struct SurfaceHandler {
    shm_token: ShmToken,
}

#[derive(Default)]
struct SurfaceData {
    buffer: Option<(Vec<u8>, (u32, u32))>,
    location: Option<(i32, i32)>,
}

impl compositor::Handler<SurfaceData, Roles> for SurfaceHandler {
    fn commit(&mut self, _evlh: &mut EventLoopHandle, _client: &Client, surface: &wl_surface::WlSurface,
              token: CompositorToken<SurfaceData, Roles, SurfaceHandler>) {
        // we retrieve the contents of the associated buffer and copy it
        token.with_surface_data(surface, |attributes| {
            match attributes.buffer.take() {
                Some(Some((buffer, (_x, _y)))) => {
                    // we ignore hotspot coordinates in this simple example
                    self.shm_token
                        .with_buffer_contents(&buffer, |slice, data| {
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
                        })
                        .unwrap();
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

struct ShellSurfaceHandler {
    token: CompositorToken<SurfaceData, Roles, SurfaceHandler>,
}

impl ShellSurfaceHandler {
    fn new(token: CompositorToken<SurfaceData, Roles, SurfaceHandler>) -> ShellSurfaceHandler {
        ShellSurfaceHandler { token }
    }
}

impl shell::Handler<SurfaceData, Roles, SurfaceHandler, ()> for ShellSurfaceHandler {
    fn new_client(&mut self, _evlh: &mut EventLoopHandle, _client: ShellClient<()>) {}
    fn client_pong(&mut self, _evlh: &mut EventLoopHandle, _client: ShellClient<()>) {}
    fn new_toplevel(&mut self, _evlh: &mut EventLoopHandle,
                    surface: ToplevelSurface<SurfaceData, Roles, SurfaceHandler, ()>)
                    -> ToplevelConfigure {
        let wl_surface = surface.get_surface().unwrap();
        self.token.with_surface_data(wl_surface, |data| {
            // place the window at a random location in the [0;300]x[0;300] square
            use rand::distributions::{IndependentSample, Range};
            let range = Range::new(0, 300);
            let mut rng = rand::thread_rng();
            let x = range.ind_sample(&mut rng);
            let y = range.ind_sample(&mut rng);
            data.user_data.location = Some((x, y))
        });
        ToplevelConfigure {
            size: None,
            states: vec![],
            serial: 42,
        }
    }
    fn new_popup(&mut self, _evlh: &mut EventLoopHandle,
                 _surface: PopupSurface<SurfaceData, Roles, SurfaceHandler, ()>)
                 -> PopupConfigure {
        PopupConfigure {
            size: (10, 10),
            position: (10, 10),
            serial: 42,
        }
    }
    fn move_(&mut self, _evlh: &mut EventLoopHandle,
             _surface: ToplevelSurface<SurfaceData, Roles, SurfaceHandler, ()>, _seat: &wl_seat::WlSeat,
             _serial: u32) {
    }
    fn resize(&mut self, _evlh: &mut EventLoopHandle,
              _surface: ToplevelSurface<SurfaceData, Roles, SurfaceHandler, ()>, _seat: &wl_seat::WlSeat,
              _serial: u32, _edges: zxdg_toplevel_v6::ResizeEdge) {
    }
    fn grab(&mut self, _evlh: &mut EventLoopHandle,
            _surface: PopupSurface<SurfaceData, Roles, SurfaceHandler, ()>, _seat: &wl_seat::WlSeat,
            _serial: u32) {
    }
    fn change_display_state(&mut self, _evlh: &mut EventLoopHandle,
                            _surface: ToplevelSurface<SurfaceData, Roles, SurfaceHandler, ()>,
                            _maximized: Option<bool>, _minimized: Option<bool>, _fullscreen: Option<bool>,
                            _output: Option<&wl_output::WlOutput>)
                            -> ToplevelConfigure {
        ToplevelConfigure {
            size: None,
            states: vec![],
            serial: 42,
        }
    }
    fn show_window_menu(&mut self, _evlh: &mut EventLoopHandle,
                        _surface: ToplevelSurface<SurfaceData, Roles, SurfaceHandler, ()>,
                        _seat: &wl_seat::WlSeat, _serial: u32, _x: i32, _y: i32) {
    }
}


type MyCompositorHandler = CompositorHandler<SurfaceData, Roles, SurfaceHandler>;
type MyShellHandler = ShellHandler<SurfaceData, Roles, SurfaceHandler, ShellSurfaceHandler, ()>;

fn main() {
    // A logger facility, here we use the terminal for this example
    let log = Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    // Initialize a simple backend for testing
    let (renderer, mut input) = winit::init(log.clone()).unwrap();

    let (mut display, mut event_loop) = wayland_server::create_display();

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
    let compositor_handler_id = event_loop.add_handler_with_init(MyCompositorHandler::new(
        SurfaceHandler {
            shm_token: shm_token.clone(),
        },
        log.clone(),
    ));
    // register it to handle wl_compositor and wl_subcompositor
    event_loop.register_global::<wl_compositor::WlCompositor, MyCompositorHandler>(compositor_handler_id, 4);
    event_loop
        .register_global::<wl_subcompositor::WlSubcompositor, MyCompositorHandler>(compositor_handler_id, 1);
    // retrieve the tokens
    let compositor_token = {
        let state = event_loop.state();
        state
            .get_handler::<MyCompositorHandler>(compositor_handler_id)
            .get_token()
    };

    /*
     * Initialize the shell global
     */
    let shell_handler_id = event_loop.add_handler_with_init(MyShellHandler::new(
        ShellSurfaceHandler::new(compositor_token),
        compositor_token,
        log.clone(),
    ));
    event_loop.register_global::<wl_shell::WlShell, MyShellHandler>(shell_handler_id, 1);
    event_loop.register_global::<zxdg_shell_v6::ZxdgShellV6, MyShellHandler>(shell_handler_id, 1);

    /*
     * Initialize glium
     */
    let context = renderer.into_glium();

    let drawer = GliumDrawer::new(&context);

    /*
     * Add a listening socket:
     */
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    println!("Listening on socket: {}", name);

    loop {
        input.dispatch_new_events().unwrap();

        let mut frame = context.draw();
        frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, None, None);
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = context.get_framebuffer_dimensions();
            let state = event_loop.state();
            for toplevel_surface in state
                .get_handler::<MyShellHandler>(shell_handler_id)
                .toplevel_surfaces()
            {
                if let Some(wl_surface) = toplevel_surface.get_surface() {
                    // this surface is a root of a subsurface tree that needs to be drawn
                    let initial_place = compositor_token
                        .with_surface_data(wl_surface, |data| data.user_data.location.unwrap_or((0, 0)));
                    compositor_token
                        .with_surface_tree(
                            wl_surface,
                            initial_place,
                            |_surface, attributes, role, &(mut x, mut y)| {
                                if let Some((ref contents, (w, h))) = attributes.user_data.buffer {
                                    // there is actually something to draw !
                                    if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                                        x += subdata.x;
                                        y += subdata.y;
                                    }
                                    drawer.draw(&mut frame, contents, (w, h), (x, y), screen_dimensions);
                                    TraversalAction::DoChildren((x, y))
                                } else {
                                    // we are not display, so our children are neither
                                    TraversalAction::SkipChildren
                                }
                            },
                        )
                        .unwrap();
                }
            }
        }
        frame.finish().unwrap();

        event_loop.dispatch(Some(16)).unwrap();
        display.flush_clients();
    }
}
