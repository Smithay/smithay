use std::sync::{Arc, Mutex};

use smithay::reexports::wayland_server::Display;

use smithay::wayland::xdg_activation::*;

use wayland_server::backend::ClientData;
use wayland_server::{Client, Dispatch, GlobalDispatch, ListeningSocket};

struct App {
    sub: Mutex<SubType>,
}

struct SubType {
    xdg_activation_state: XdgActivationState,
}

impl XdgActivationHandler for SubType {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        _token_data: XdgActivationTokenData,
        _surface: wayland_server::protocol::wl_surface::WlSurface,
    ) {
        self.xdg_activation_state.remove_token(&token);
    }
}

#[derive(Debug, Default)]
struct ClientState;
impl ClientData for ClientState {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();

    let xdg_activation_state = XdgActivationState::new::<App>(&dh);

    let mut state = App {
        sub: Mutex::new(SubType { xdg_activation_state }),
    };

    let listener = ListeningSocket::bind("wayland-5").unwrap();

    let mut clients = Vec::new();

    loop {
        if let Some(stream) = listener.accept().unwrap() {
            println!("Got a client: {:?}", stream);

            let client = display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
                .unwrap();
            clients.push(client);
        }

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }
}

impl GlobalDispatch<xdg_activation_v1::XdgActivationV1, (), App> for App {
    fn bind(
        state: &mut App,
        handle: &wayland_server::DisplayHandle,
        client: &Client,
        resource: wayland_server::New<xdg_activation_v1::XdgActivationV1>,
        global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, App>,
    ) {
        let mut guard = state.sub.lock().unwrap();
        <SubType as GlobalDispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::bind(
            &mut *guard,
            handle,
            client,
            resource,
            global_data,
            data_init,
        )
    }

    fn can_view(client: Client, global_data: &()) -> bool {
        <SubType as GlobalDispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::can_view(
            client,
            global_data,
        )
    }
}

impl Dispatch<xdg_activation_v1::XdgActivationV1, (), App> for App {
    fn request(
        state: &mut App,
        client: &Client,
        resource: &xdg_activation_v1::XdgActivationV1,
        request: <xdg_activation_v1::XdgActivationV1 as wayland_server::Resource>::Request,
        data: &(),
        dhandle: &wayland_server::DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, App>,
    ) {
        let mut guard = state.sub.lock().unwrap();
        <SubType as Dispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::request(
            &mut *guard,
            client,
            resource,
            request,
            data,
            dhandle,
            data_init,
        )
    }

    fn destroyed(
        state: &mut App,
        client: wayland_server::backend::ClientId,
        resource: &xdg_activation_v1::XdgActivationV1,
        data: &(),
    ) {
        let mut guard = state.sub.lock().unwrap();
        <SubType as Dispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::destroyed(
            &mut *guard,
            client,
            resource,
            data,
        )
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ActivationTokenData, App> for App {
    fn request(
        state: &mut App,
        client: &Client,
        resource: &xdg_activation_token_v1::XdgActivationTokenV1,
        request: <xdg_activation_token_v1::XdgActivationTokenV1 as wayland_server::Resource>::Request,
        data: &ActivationTokenData,
        dhandle: &wayland_server::DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, App>,
    ) {
        let mut guard = state.sub.lock().unwrap();
        <SubType as Dispatch<
            xdg_activation_token_v1::XdgActivationTokenV1,
            ActivationTokenData,
            SubType,
            App,
        >>::request(&mut *guard, client, resource, request, data, dhandle, data_init)
    }

    fn destroyed(
        state: &mut App,
        client: wayland_server::backend::ClientId,
        resource: &xdg_activation_token_v1::XdgActivationTokenV1,
        data: &ActivationTokenData,
    ) {
        let mut guard = state.sub.lock().unwrap();
        <SubType as Dispatch<
            xdg_activation_token_v1::XdgActivationTokenV1,
            ActivationTokenData,
            SubType,
            App,
        >>::destroyed(&mut *guard, client, resource, data)
    }
}

impl GlobalDispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App> for SubType {
    fn bind(
        state: &mut SubType,
        handle: &wayland_server::DisplayHandle,
        client: &Client,
        resource: wayland_server::New<xdg_activation_v1::XdgActivationV1>,
        global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, App>,
    ) {
        <XdgActivationState as GlobalDispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::bind(
            state,
            handle,
            client,
            resource,
            global_data,
            data_init,
        )
    }

    fn can_view(client: Client, global_data: &()) -> bool {
        <XdgActivationState as GlobalDispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::can_view(
            client,
            global_data,
        )
    }
}

impl Dispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App> for SubType {
    fn request(
        state: &mut SubType,
        client: &Client,
        resource: &xdg_activation_v1::XdgActivationV1,
        request: <xdg_activation_v1::XdgActivationV1 as wayland_server::Resource>::Request,
        data: &(),
        dhandle: &wayland_server::DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, App>,
    ) {
        <XdgActivationState as Dispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::request(
            state, client, resource, request, data, dhandle, data_init,
        )
    }

    fn destroyed(
        state: &mut SubType,
        client: wayland_server::backend::ClientId,
        resource: &xdg_activation_v1::XdgActivationV1,
        data: &(),
    ) {
        <XdgActivationState as Dispatch<xdg_activation_v1::XdgActivationV1, (), SubType, App>>::destroyed(
            state, client, resource, data,
        )
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ActivationTokenData, SubType, App> for SubType {
    fn request(
        state: &mut SubType,
        client: &Client,
        resource: &xdg_activation_token_v1::XdgActivationTokenV1,
        request: <xdg_activation_token_v1::XdgActivationTokenV1 as wayland_server::Resource>::Request,
        data: &ActivationTokenData,
        dhandle: &wayland_server::DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, App>,
    ) {
        <XdgActivationState as Dispatch<
            xdg_activation_token_v1::XdgActivationTokenV1,
            ActivationTokenData,
            SubType,
            App,
        >>::request(state, client, resource, request, data, dhandle, data_init)
    }

    fn destroyed(
        state: &mut SubType,
        client: wayland_server::backend::ClientId,
        resource: &xdg_activation_token_v1::XdgActivationTokenV1,
        data: &ActivationTokenData,
    ) {
        <XdgActivationState as Dispatch<
            xdg_activation_token_v1::XdgActivationTokenV1,
            ActivationTokenData,
            SubType,
            App,
        >>::destroyed(state, client, resource, data)
    }
}
