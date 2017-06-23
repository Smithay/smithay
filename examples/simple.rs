#[macro_use(server_declare_handler)]
extern crate wayland_server;
extern crate smithay;
#[macro_use]
extern crate glium;

#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;

mod helpers;

use glium::Surface;

use helpers::{GliumDrawer, WlShellStubHandler};
use slog::*;

use smithay::backend::graphics::glium::IntoGlium;
use smithay::backend::input::InputBackend;
use smithay::backend::winit;
use smithay::compositor::{self, CompositorHandler, CompositorToken, TraversalAction};
use smithay::shm::{BufferData, ShmGlobal, ShmToken};
use wayland_server::{Client, EventLoopHandle, Liveness, Resource};

use wayland_server::protocol::{wl_compositor, wl_shell, wl_shm, wl_subcompositor, wl_surface};

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
                            new_vec.extend(&slice[(offset + i * stride)..(offset + i * stride + width * 4)]);
                        }
                        attributes.user_data.buffer = Some((new_vec,
                                                            (data.width as u32, data.height as u32)));
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

fn main() {
    // A logger facility, here we use the terminal for this example
    let log = Logger::root(slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
                           o!());

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
    let compositor_handler_id =
        event_loop.add_handler_with_init(CompositorHandler::<SurfaceData, _>::new(SurfaceHandler {
                                                                                      shm_token: shm_token
                                                                                          .clone(),
                                                                                  },
                                                                                  log.clone()));
    // register it to handle wl_compositor and wl_subcompositor
    event_loop.register_global::<wl_compositor::WlCompositor, CompositorHandler<SurfaceData,SurfaceHandler>>(compositor_handler_id, 4);
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
    let shell_handler_id = event_loop
        .add_handler_with_init(WlShellStubHandler::new(compositor_token.clone()));
    event_loop.register_global::<wl_shell::WlShell, WlShellStubHandler<SurfaceData, SurfaceHandler>>(shell_handler_id,
                                                                                            1);

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
            for &(_, ref surface) in
                state
                    .get_handler::<WlShellStubHandler<SurfaceData, SurfaceHandler>>(shell_handler_id)
                    .surfaces() {
                if surface.status() != Liveness::Alive {
                    continue;
                }
                // this surface is a root of a subsurface tree that needs to be drawn
                compositor_token.with_surface_tree(surface, (100, 100), |surface,
                 attributes,
                 &(mut x, mut y)| {
                    if let Some((ref contents, (w, h))) = attributes.user_data.buffer {
                        // there is actually something to draw !
                        if let Some(ref subdata) = attributes.subsurface_attributes {
                            x += subdata.x;
                            y += subdata.y;
                        }
                        drawer.draw(&mut frame, contents, (w, h), (x, y), screen_dimensions);
                        TraversalAction::DoChildren((x, y))
                    } else {
                        // we are not display, so our children are neither
                        TraversalAction::SkipChildren
                    }
                });
            }
        }
        frame.finish().unwrap();

        event_loop.dispatch(Some(16)).unwrap();
        display.flush_clients();
    }
}
