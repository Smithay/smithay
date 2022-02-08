use std::sync::Arc;

use smithay::reexports::wayland_server::Display;
use smithay::wayland::seat::{self as seat, SeatHandler};

use seat::SeatState;

use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::{wl_keyboard::WlKeyboard, wl_pointer::WlPointer, wl_seat::WlSeat};
use wayland_server::socket::ListeningSocket;
use wayland_server::{delegate_dispatch, delegate_global_dispatch};

struct App {
    seat_state: SeatState<Self>,
}

impl SeatHandler<Self> for App {
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<App> = Display::new()?;

    let seat_state = SeatState::new(&mut display, "Example".into(), None);

    let mut state = App { seat_state };

    let keyboard =
        state
            .seat_state
            .add_keyboard(&mut display.handle(), Default::default(), 25, 600, |_, _| {})?;

    let listener = ListeningSocket::bind("wayland-5").unwrap();

    let mut clients = Vec::new();

    loop {
        if let Some(stream) = listener.accept().unwrap() {
            println!("Got a client: {:?}", stream);

            let client = display.insert_client(stream, Arc::new(ClientState)).unwrap();
            clients.push(client);
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

        keyboard.set_focus(&mut display.handle(), None, 0.into());

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }
}

struct ClientState;
impl ClientData<App> for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        println!("initialized");
    }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        println!("disconnected");
    }
}

delegate_global_dispatch!(App: [WlSeat] => SeatState<App>);
delegate_dispatch!(App: [WlSeat, WlPointer, WlKeyboard] => SeatState<App>);
