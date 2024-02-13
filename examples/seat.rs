use std::sync::Arc;

use smithay::delegate_seat;
use smithay::input::{keyboard::FilterResult, Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::wl_surface::WlSurface,
    Display, ListeningSocket,
};

struct App {
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
}

impl SeatHandler for App {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();

    let mut seat_state = SeatState::new();
    let seat = seat_state.new_wl_seat(&dh, "Example");

    let mut state = App { seat_state, seat };

    let keyboard = state.seat.add_keyboard(Default::default(), 25, 600)?;

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

        keyboard.input(
            &mut state,
            1,
            smithay::backend::input::KeyState::Pressed,
            0.into(),
            0,
            |_, _, _| {
                if false {
                    FilterResult::Intercept(0)
                } else {
                    FilterResult::Forward
                }
            },
        );

        keyboard.set_focus(&mut state, Option::<WlSurface>::None, 0.into());

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

delegate_seat!(App);
