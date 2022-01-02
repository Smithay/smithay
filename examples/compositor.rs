use std::sync::Arc;

use smithay::reexports::wayland_server::Display;

use smithay::wayland::compositor::{
    CompositorDispatch, CompositorHandler, CompositorState, RegionUserData, SubsurfaceUserData,
    SurfaceUserData,
};
use smithay::wayland::delegate::{DelegateDispatch, DelegateGlobalDispatch};

use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_callback::{self, WlCallback};
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_region::{self, WlRegion};
use wayland_server::protocol::wl_subcompositor::{self, WlSubcompositor};
use wayland_server::protocol::wl_subsurface::{self, WlSubsurface};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{socket::ListeningSocket, Dispatch, DisplayHandle, GlobalDispatch};
use wayland_server::{DataInit, New};

struct App {
    inner: InnerApp,
    compositor_state: CompositorState,
}

impl CompositorHandler for InnerApp {
    fn commit(&mut self, surface: &WlSurface) {
        dbg!("Commit", surface);
    }
}

struct InnerApp;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<App> = Display::new()?;

    let compositor_state = CompositorState::new(&mut display.handle(), None);

    let mut state = App {
        inner: InnerApp,
        compositor_state,
    };

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

impl GlobalDispatch<WlCompositor> for App {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: New<WlCompositor>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        DelegateGlobalDispatch::<WlCompositor, _>::bind(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            handle,
            client,
            resource,
            global_data,
            data_init,
        );
    }
}

impl Dispatch<WlCompositor> for App {
    type UserData = ();

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlCompositor,
        request: wl_compositor::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        dbg!(&request);

        DelegateDispatch::<WlCompositor, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlSurface> for App {
    type UserData = SurfaceUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSurface,
        request: wl_surface::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlSurface, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlRegion> for App {
    type UserData = RegionUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlRegion,
        request: wl_region::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlRegion, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlCallback> for App {
    type UserData = ();

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlCallback,
        request: wl_callback::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlSubcompositor> for App {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: New<WlSubcompositor>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        DelegateGlobalDispatch::<WlSubcompositor, _>::bind(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            handle,
            client,
            resource,
            global_data,
            data_init,
        );
    }
}

impl Dispatch<WlSubcompositor> for App {
    type UserData = ();

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSubcompositor,
        request: wl_subcompositor::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlSubcompositor, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlSubsurface> for App {
    type UserData = SubsurfaceUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSubsurface,
        request: wl_subsurface::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlSubsurface, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}
