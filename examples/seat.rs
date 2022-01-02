use std::sync::Arc;

use smithay::reexports::wayland_server::Display;
use smithay::wayland::seat::{self as seat};

use seat::{KeyboardUserData, PointerUserData, SeatDispatch, SeatState, SeatUserData};
use smithay::wayland::delegate::{DelegateDispatch, DelegateGlobalDispatch};

use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::{
    wl_keyboard::{self, WlKeyboard},
    wl_pointer::{self, WlPointer},
    wl_seat::{self, WlSeat},
};
use wayland_server::{socket::ListeningSocket, Dispatch, DisplayHandle, GlobalDispatch};
use wayland_server::{DataInit, New};

struct App {
    inner: InnerApp,
    seat_state: SeatState<Self>,
}

struct InnerApp;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<App> = Display::new()?;

    let seat_state = SeatState::new(&mut display.handle(), "Example".into(), None);

    let mut state = App {
        inner: InnerApp,
        seat_state,
    };

    let keyboard =
        state
            .seat_state
            .add_keyboard(&mut display.handle(), Default::default(), 25, 600, |_, _| {})?;

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

        keyboard.set_focus(&mut display.handle(), None, 0.into());

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }
}

struct ClientState;
impl ClientData<App> for ClientState {
    fn initialized(&self, client_id: ClientId) {
        println!("initialized");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        println!("disconnected");
    }
}

impl GlobalDispatch<WlSeat> for App {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: New<WlSeat>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        DelegateGlobalDispatch::<WlSeat, _>::bind(
            &mut SeatDispatch(&mut self.seat_state),
            handle,
            client,
            resource,
            global_data,
            data_init,
        );
    }
}

impl Dispatch<WlSeat> for App {
    type UserData = SeatUserData<Self>;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSeat,
        request: wl_seat::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlSeat, _>::request(
            &mut SeatDispatch(&mut self.seat_state),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlKeyboard> for App {
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
        DelegateDispatch::<WlKeyboard, _>::request(
            &mut SeatDispatch(&mut self.seat_state),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlPointer> for App {
    type UserData = PointerUserData<Self>;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlPointer,
        request: wl_pointer::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlPointer, _>::request(
            &mut SeatDispatch(&mut self.seat_state),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}
