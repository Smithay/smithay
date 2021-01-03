use std::os::unix::net::UnixStream;

use smithay:: {
    reexports::wayland_server::Client,
    xwayland::XWindowManager,
};

pub struct XWm;

impl XWm {
    pub fn new() -> Self {
        Self
    }
}

impl XWindowManager for XWm {
    fn xwayland_ready(&mut self, _connection: UnixStream, _client: Client) {
    }

    fn xwayland_exited(&mut self) {}
}
