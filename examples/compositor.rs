use std::sync::Arc;

use smithay::delegate_compositor;
use smithay::reexports::wayland_server::Display;

use smithay::wayland::compositor::{CompositorHandler, CompositorState};

use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{socket::ListeningSocket, DisplayHandle};

struct App {
    compositor_state: CompositorState,
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn commit(&mut self, _dh: &DisplayHandle, surface: &WlSurface) {
        dbg!("Commit", surface);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();

    let compositor_state = CompositorState::new::<App, _>(&dh, None);

    let mut state = App { compositor_state };

    let listener = ListeningSocket::bind("wayland-5").unwrap();

    let mut clients = Vec::new();

    loop {
        if let Some(stream) = listener.accept().unwrap() {
            println!("Got a client: {:?}", stream);

            let client = display
                .handle()
                .insert_client(stream, Arc::new(ClientState))
                .unwrap();
            clients.push(client);
        }

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }
}

struct ClientState;
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        println!("initialized");
    }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        println!("disconnected");
    }
}

impl AsMut<CompositorState> for App {
    fn as_mut(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
}

delegate_compositor!(App);
