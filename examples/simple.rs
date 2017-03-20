extern crate wayland_server;
extern crate smithay;

use smithay::backend::glutin;
use smithay::backend::input::InputBackend;
use smithay::shm::ShmGlobal;
use wayland_server::protocol::wl_shm;

fn main() {
    let (_, mut event_loop) = wayland_server::create_display();

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

    // Initialize a simple backend for testing
    let (mut renderer, mut input) = glutin::init_windowed().unwrap();

    // TODO render stuff

    // TODO put input handling on the event loop
    input.dispatch_new_events().unwrap();

    event_loop.run().unwrap();
}
