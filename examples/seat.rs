use std::sync::Arc;

use smithay::reexports::wayland_server::Display;
use smithay::wayland::seat2::{self as seat};

use seat::{DelegateDispatch, DelegateGlobalDispatch, KeyboardUserData, PointerUserData, Seat, SeatUserData};

use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::{
    wl_keyboard::{self, WlKeyboard},
    wl_pointer::{self, WlPointer},
    wl_seat::{self, WlSeat},
};
use wayland_server::{socket::ListeningSocket, Dispatch, DisplayHandle, GlobalDispatch};

struct State {
    seat: Seat,
}

impl Dispatch<WlSeat> for State {
    type UserData = SeatUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSeat,
        request: wl_seat::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        let event = <Seat as DelegateDispatch<WlSeat, Self>>::request(
            &mut self.seat,
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );

        // match event {
        //     SeatEvent::SetCursor { .. } => {}
        // }
    }
}

impl GlobalDispatch<WlSeat> for State {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: &WlSeat,
        global_data: &Self::GlobalData,
    ) -> SeatUserData {
        <Seat as DelegateGlobalDispatch<WlSeat, Self>>::bind(
            &mut self.seat,
            handle,
            client,
            resource,
            global_data,
        )
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<State> = Display::new()?;

    let mut seat = Seat::new(&mut display, "Example".into(), None);

    let keyboard = seat.add_keyboard(&mut display.handle(), Default::default(), 25, 600, |_, _| {})?;

    let mut state = State { seat };

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
impl ClientData<State> for ClientState {
    fn initialized(&self, client_id: ClientId) {
        println!("initialized");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        println!("disconnected");
    }
}

impl Dispatch<WlKeyboard> for State {
    type UserData = KeyboardUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlKeyboard,
        request: wl_keyboard::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WlPointer> for State {
    type UserData = PointerUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlPointer,
        request: wl_pointer::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
    }
}
