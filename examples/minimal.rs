use std::{borrow::BorrowMut, cell::RefCell, rc::Rc, sync::Arc};

use slog::o;
use smithay::{
    backend::{
        renderer::{
            buffer_dimensions, buffer_type,
            gles2::{Gles2Frame, Gles2Renderer, Gles2Texture},
            utils::{draw_surface_tree, on_commit_buffer_handler},
            BufferType, Frame, ImportAll, Renderer, Transform,
        },
        winit::{self, WinitEvent},
        SwapBuffersError,
    },
    reexports::{calloop::EventLoop, wayland_server::Display},
    utils::{Logical, Physical, Point, Rectangle, Size},
    wayland::{
        compositor::{
            self, is_sync_subsurface, with_surface_tree_upward, BufferAssignment, CompositorDispatch,
            CompositorHandler, CompositorState, Damage, RegionUserData, SubsurfaceCachedState,
            SubsurfaceUserData, SurfaceAttributes, SurfaceUserData, TraversalAction,
        },
        delegate::{DelegateDispatch, DelegateGlobalDispatch},
        shell::xdg::{
            ShellSurfaceUserData, XdgRequest, XdgShellDispatch, XdgShellHandler, XdgShellState,
            XdgSurfaceUserData, XdgWmBaseUserData,
        },
        shm::{ShmBufferUserData, ShmDispatch, ShmPoolUserData, ShmState},
    },
};
use wayland_protocols::xdg_shell::server::{
    xdg_surface,
    xdg_surface::XdgSurface,
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_callback::{self, WlCallback},
        wl_compositor::{self, WlCompositor},
        wl_region::{self, WlRegion},
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
        wl_subcompositor::{self, WlSubcompositor},
        wl_subsurface::{self, WlSubsurface},
        wl_surface::{self, WlSurface},
    },
    socket::ListeningSocket,
    DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

struct InnerApp;

impl XdgShellHandler<App> for InnerApp {
    fn request(&mut self, cx: &mut DisplayHandle<App>, request: XdgRequest) {
        dbg!(&request);

        match request{
            XdgRequest::NewToplevel { surface } => {
                surface.send_configure(cx);
            }
            _ =>{}
            // XdgRequest::NewPopup { surface, positioner } => todo!(),
            // XdgRequest::Move { surface, seat, serial } => todo!(),
            // XdgRequest::Resize { surface, seat, serial, edges } => todo!(),
            // XdgRequest::Grab { surface, seat, serial } => todo!(),
            // XdgRequest::Maximize { surface } => todo!(),
            // XdgRequest::UnMaximize { surface } => todo!(),
            // XdgRequest::Fullscreen { surface, output } => todo!(),
            // XdgRequest::UnFullscreen { surface } => todo!(),
            // XdgRequest::Minimize { surface } => todo!(),
            // XdgRequest::ShowWindowMenu { surface, seat, serial, location } => todo!(),
            // XdgRequest::AckConfigure { surface, configure } => todo!(),
            // XdgRequest::RePosition { surface, positioner, token } => todo!(),
        }
    }
}

impl CompositorHandler<App> for InnerApp {
    fn commit(&mut self, cx: &mut DisplayHandle<App>, surface: &WlSurface) {
        on_commit_buffer_handler(cx, surface);
    }
}

struct App {
    inner: InnerApp,
    compositor_state: CompositorState<Self>,
    xdg_shell_state: XdgShellState<Self>,
    shm_state: ShmState,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_winit()
}

fn log() -> ::slog::Logger {
    use slog::Drain;
    ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), slog::o!())
}

pub fn run_winit() -> Result<(), Box<dyn std::error::Error>> {
    let log = log();

    let mut display: Display<App> = Display::new()?;

    let mut state = {
        let handle = &mut display.handle();
        App {
            inner: InnerApp,
            compositor_state: CompositorState::new(handle, None),
            xdg_shell_state: XdgShellState::new(handle, None).0,
            shm_state: ShmState::new(handle, vec![], None),
        }
    };
    let listener = ListeningSocket::bind("wayland-5").unwrap();
    let mut clients = Vec::new();

    let (mut backend, mut winit) = winit::init(None)?;

    loop {
        winit.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {}
            WinitEvent::Input(event) => {}
            _ => (),
        })?;

        backend.bind().unwrap();

        let size = backend.window_size().physical_size;
        let damage = Rectangle::from_loc_and_size((0, 0), size);

        backend
            .renderer()
            .render(size, Transform::Normal, |renderer, frame| {
                frame.clear([0.1, 0.0, 0.0, 1.0], &[damage]).unwrap();

                state.xdg_shell_state.toplevel_surfaces(|surfaces| {
                    for surface in surfaces {
                        let cx = &mut display.handle();
                        let surface = surface.get_surface(cx).unwrap();
                        draw_surface_tree::<App, _, _, _, _>(
                            renderer,
                            frame,
                            surface,
                            1.0,
                            (0, 0).into(),
                            &[damage.to_logical(1)],
                            &log,
                        )
                        .unwrap();
                    }
                });
            })?;

        backend.submit(Some(&[damage.to_logical(1)]), 1.0).unwrap();

        match listener.accept()? {
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

impl GlobalDispatch<XdgWmBase> for App {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: New<XdgWmBase>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        DelegateGlobalDispatch::<XdgWmBase, _>::bind(
            &mut XdgShellDispatch(&mut self.xdg_shell_state, &mut self.inner),
            handle,
            client,
            resource,
            global_data,
            data_init,
        );
    }
}

impl Dispatch<XdgWmBase> for App {
    type UserData = XdgWmBaseUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &XdgWmBase,
        request: xdg_wm_base::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<XdgWmBase, _>::request(
            &mut XdgShellDispatch(&mut self.xdg_shell_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<XdgSurface> for App {
    type UserData = XdgSurfaceUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &XdgSurface,
        request: xdg_surface::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<XdgSurface, _>::request(
            &mut XdgShellDispatch(&mut self.xdg_shell_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<XdgToplevel> for App {
    type UserData = ShellSurfaceUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &XdgToplevel,
        request: xdg_toplevel::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<XdgToplevel, _>::request(
            &mut XdgShellDispatch(&mut self.xdg_shell_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

/*
 * Compositor
 *
*/

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
    type UserData = SurfaceUserData<Self>;

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
    type UserData = SubsurfaceUserData<Self>;

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

//
// SHM
//

impl GlobalDispatch<WlShm> for App {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: New<WlShm>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        DelegateGlobalDispatch::<WlShm, _>::bind(
            &mut ShmDispatch(&mut self.shm_state),
            handle,
            client,
            resource,
            global_data,
            data_init,
        );
    }
}

impl Dispatch<WlShm> for App {
    type UserData = ();

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlShm,
        request: wl_shm::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlShm, _>::request(
            &mut ShmDispatch(&mut self.shm_state),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlShmPool> for App {
    type UserData = ShmPoolUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlShmPool,
        request: wl_shm_pool::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlShmPool, _>::request(
            &mut ShmDispatch(&mut self.shm_state),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlBuffer> for App {
    type UserData = ShmBufferUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlBuffer,
        request: wl_buffer::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlBuffer, _>::request(
            &mut ShmDispatch(&mut self.shm_state),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}
