extern crate wayland_server;
extern crate smithay;
extern crate glium;

use glium::Surface;
use smithay::backend::graphics::glium::IntoGlium;
use smithay::backend::input::InputBackend;
use smithay::backend::winit;
use smithay::shm::ShmGlobal;
use wayland_server::protocol::wl_shm;

fn main() {
    // Initialize a simple backend for testing
    let (renderer, mut input) = winit::init().unwrap();

    let (_display, mut event_loop) = wayland_server::create_display();

    // Insert the ShmGlobal as a handler to your event loop
    // Here, we specify tha the standard Argb8888 and Xrgb8888 is the only supported.
    let handler_id =
        event_loop.add_handler_with_init(ShmGlobal::new(vec![],
                                                        None /* we don't provide a logger here */));

    // Register this handler to advertise a wl_shm global of version 1
    let shm_global = event_loop.register_global::<wl_shm::WlShm, ShmGlobal>(handler_id, 1);

    // Retrieve the shm token for later use to access the buffers
    let shm_token = {
        let state = event_loop.state();
        state.get_handler::<ShmGlobal>(handler_id).get_token()
    };

    // Init glium
    let context = renderer.into_glium();


    loop {
        input.dispatch_new_events().unwrap();

        let mut frame = context.draw();
        frame.clear(None, Some((0.0, 0.0, 0.0, 1.0)), false, None, None);
        frame.finish().unwrap();

        event_loop.dispatch(Some(16)).unwrap();
    }
}
