use std::sync::Arc;

use smithay::reexports::wayland_server::Display;
use smithay::wayland::seat2 as seat;

use seat::Seat;
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::socket::ListeningSocket;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<Seat> = Display::new()?;

    let mut state = Seat::new(&mut display, "Example".into(), None);

    let keyboard = state.add_keyboard(&mut display.handle(), Default::default(), 25, 600, |_, _| {})?;

    let listener = ListeningSocket::bind("wayland-5").unwrap();

    let mut clients = Vec::new();

    loop {
        match listener.accept().unwrap() {
            Some(stream) => {
                println!("Got a client: {:?}", stream);

                let client = display.insert_client(stream, Arc::new(ClientState)).unwrap();
                clients.push(client);
            }
            None => {}
        }

        keyboard.input(
            &mut display.handle(),
            1,
            smithay::backend::input::KeyState::Pressed,
            0.into(),
            0,
            |_, _| {
                if false {
                    seat::FilterResult::Intercept(0)
                } else {
                    seat::FilterResult::Forward
                }
            },
        );

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }
}

struct ClientState;
impl ClientData<Seat> for ClientState {
    fn initialized(&self, client_id: ClientId) {
        println!("initialized");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        println!("disconnected");
    }
}
