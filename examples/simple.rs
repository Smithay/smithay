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
use helpers::{shell_implementation, surface_implementation, GliumDrawer};
use slog::{Drain, Logger};
use smithay::backend::graphics::glium::IntoGlium;
use smithay::backend::input::InputBackend;
use smithay::backend::winit;
use smithay::compositor::{compositor_init, SubsurfaceRole, TraversalAction};
use smithay::compositor::roles::Role;
use smithay::shell::shell_init;
use smithay::shm::init_shm_global;

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
     * Initialize the globals
     */

    init_shm_global(&mut event_loop, vec![], log.clone());

    let (compositor_token, _, _) =
        compositor_init(&mut event_loop, surface_implementation(), (), log.clone());

    let (shell_state_token, _, _) = shell_init(
        &mut event_loop,
        compositor_token,
        shell_implementation(),
        compositor_token,
        log.clone(),
    );

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
            for toplevel_surface in state.get(&shell_state_token).toplevel_surfaces() {
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
