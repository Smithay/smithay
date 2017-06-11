extern crate wayland_server;
extern crate smithay;
extern crate glium;

#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;


use glium::Surface;
use slog::*;

use smithay::backend::graphics::glium::IntoGlium;
use smithay::backend::input::InputBackend;
use smithay::backend::winit;
use smithay::compositor::{self, CompositorHandler};
use smithay::shm::ShmGlobal;

use wayland_server::protocol::{wl_compositor, wl_shm, wl_subcompositor};

struct SurfaceHandler;

impl compositor::Handler for SurfaceHandler {}

fn main() {
    // A logger facility, here we use the terminal for this example
    let log = Logger::root(slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
                           o!());

    // Initialize a simple backend for testing
    let (renderer, mut input) = winit::init(log.clone()).unwrap();

    let (_display, mut event_loop) = wayland_server::create_display();

    /*
     * Initialize wl_shm global
     */
    // Insert the ShmGlobal as a handler to your event loop
    // Here, we specify tha the standard Argb8888 and Xrgb8888 is the only supported.
    let shm_handler_id = event_loop.add_handler_with_init(ShmGlobal::new(vec![], log.clone()));
    // Register this handler to advertise a wl_shm global of version 1
    event_loop.register_global::<wl_shm::WlShm, ShmGlobal>(shm_handler_id, 1);

    /*
     * Initialize the compositor global
     */
    let compositor_handler_id =
        event_loop.add_handler_with_init(CompositorHandler::<(), _>::new(SurfaceHandler, log.clone()));
    // register it to handle wl_compositor and wl_subcompositor
    event_loop.register_global::<wl_compositor::WlCompositor, CompositorHandler<(),SurfaceHandler>>(compositor_handler_id, 4);
    event_loop.register_global::<wl_subcompositor::WlSubcompositor, CompositorHandler<(),SurfaceHandler>>(compositor_handler_id, 1);

    /*
     * retrieve the tokens
     */
    let (shm_token, compositor_token) = {
        let state = event_loop.state();
        (state.get_handler::<ShmGlobal>(shm_handler_id).get_token(),
         state
             .get_handler::<CompositorHandler<(), SurfaceHandler>>(compositor_handler_id)
             .get_token())
    };

    /*
     * Initialize glium
     */
    let context = renderer.into_glium();


    loop {
        input.dispatch_new_events().unwrap();

        let mut frame = context.draw();
        frame.clear(None, Some((0.0, 0.0, 0.0, 1.0)), false, None, None);
        frame.finish().unwrap();

        event_loop.dispatch(Some(16)).unwrap();
    }
}
