use std::{
    os::unix::io::RawFd,
    sync::{
        atomic::AtomicBool,
        Arc, Mutex,
    },
};

use smithay::{
    desktop::{PopupManager, Space, WindowSurfaceType},
    reexports::{
        calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction},
        wayland_protocols::{
            unstable::{
                xdg_decoration::{self, v1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode},
                xdg_output::v1::server::{
                    zxdg_output_manager_v1::ZxdgOutputManagerV1, zxdg_output_v1::ZxdgOutputV1,
                }
            },
            wlr::unstable::layer_shell::v1::server::{zwlr_layer_shell_v1::ZwlrLayerShellV1, zwlr_layer_surface_v1::ZwlrLayerSurfaceV1},
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{
                wl_data_source::WlDataSource,
                wl_surface::WlSurface,
                wl_output::WlOutput,
            },
            Display, DisplayHandle, Resource,
            delegate_dispatch, delegate_global_dispatch,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::CompositorState,
        data_device::{set_data_device_focus, DataDeviceState, DataDeviceHandler, ClientDndGrabHandler, ServerDndGrabHandler},
        output::{OutputManagerState, Output},
        seat::{CursorImageStatus, KeyboardHandle, PointerHandle, Seat, XkbConfig, SeatState, SeatHandler},
        shell::{
            xdg::{
                decoration::{XdgDecorationHandler, XdgDecorationManager},
                XdgShellState, ToplevelSurface,
            },
            wlr_layer::WlrLayerShellState,
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        xdg_activation::{XdgActivationHandler, XdgActivationToken, XdgActivationTokenData, XdgActivationState},
    },
    delegate_compositor, delegate_data_device, delegate_seat,
    delegate_shm, delegate_xdg_activation, delegate_xdg_decoration,
    delegate_xdg_shell,
};

#[cfg(feature = "xwayland")]
use smithay::xwayland::{XWayland, XWaylandEvent};

pub struct CalloopData<BackendData: 'static> {
    pub state: AnvilState<BackendData>,
    pub display: Display<AnvilState<BackendData>>,
}

struct ClientState;

impl<BackendData> ClientData<AnvilState<BackendData>> for ClientState {
    /// Notification that a client was initialized
    fn initialized(&self, client_id: ClientId) {}
    /// Notification that a client is disconnected
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {}
}

#[derive(Debug)]
pub struct AnvilState<BackendData: 'static> {
    pub backend_data: BackendData,
    pub socket_name: Option<String>,
    pub running: Arc<AtomicBool>,
    pub handle: LoopHandle<'static, CalloopData<BackendData>>,

    // desktop
    pub space: Space,
    pub popups: PopupManager,
   
    // smithay state
    pub compositor_state: CompositorState,
    pub data_device_state: DataDeviceState,
    pub layer_shell_state: WlrLayerShellState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<AnvilState<BackendData>>,
    pub shm_state: ShmState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_decoration_state: XdgDecorationManager,
    pub xdg_shell_state: XdgShellState,

    pub dnd_icon: Option<WlSurface>,
    pub log: slog::Logger,
    
    // input-related fields
    pub suppressed_keys: Vec<u32>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: Arc<Mutex<CursorImageStatus>>,
    pub seat_name: String,
    pub seat: Seat<AnvilState<BackendData>>,
    pub start_time: std::time::Instant,

    // things we must keep alive
    #[cfg(feature = "xwayland")]
    pub xwayland: XWayland<AnvilState<BackendData>>,
}

#[cfg(feature = "winit")]
delegate_compositor!(AnvilState<crate::winit::WinitData>);
#[cfg(feature = "x11")]
delegate_compositor!(AnvilState<crate::x11::X11Data>);
#[cfg(feature = "udev")]
delegate_compositor!(AnvilState<crate::udev::UdevData>);

impl<BackendData> DataDeviceHandler for AnvilState<BackendData> {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
    fn send_selection(&mut self, mime_type: String, fd: RawFd) {
        unreachable!("Anvil doesn't do server-side selections");
    }
}
impl<BackendData> ClientDndGrabHandler for AnvilState<BackendData> {
    fn started(
        &mut self, 
        source: Option<WlDataSource>, 
        icon: Option<WlSurface>, 
        seat: Seat<Self>
    ) {
        self.dnd_icon = icon;
    }
    fn dropped(&mut self, seat: Seat<Self>) {
        self.dnd_icon = None;
    }
}
impl<BackendData> ServerDndGrabHandler for AnvilState<BackendData> {
    fn send(&mut self, mime_type: String, fd: RawFd) {
        unreachable!("Anvil doesn't do server-side grabs");
    }
}
#[cfg(feature = "winit")]
delegate_data_device!(AnvilState<crate::winit::WinitData>);
#[cfg(feature = "x11")]
delegate_data_device!(AnvilState<crate::x11::X11Data>);
#[cfg(feature = "udev")]
delegate_data_device!(AnvilState<crate::udev::UdevData>);

#[cfg(feature = "winit")]
delegate_global_dispatch!(AnvilState<crate::winit::WinitData>: [WlOutput, ZxdgOutputManagerV1] => OutputManagerState);
#[cfg(feature = "winit")]
delegate_dispatch!(AnvilState<crate::winit::WinitData>: [WlOutput, ZxdgOutputManagerV1, ZxdgOutputV1] => OutputManagerState);
#[cfg(feature = "x11")]
delegate_global_dispatch!(AnvilState<crate::x11::X11Data>: [WlOutput, ZxdgOutputManagerV1] => OutputManagerState);
#[cfg(feature = "x11")]
delegate_dispatch!(AnvilState<crate::x11::X11Data>: [WlOutput, ZxdgOutputManagerV1, ZxdgOutputV1] => OutputManagerState);
#[cfg(feature = "udev")]
delegate_global_dispatch!(AnvilState<crate::udev::UdevData>: [WlOutput, ZxdgOutputManagerV1] => OutputManagerState);
#[cfg(feature = "udev")]
delegate_dispatch!(AnvilState<crate::udev::UdevData>: [WlOutput, ZxdgOutputManagerV1, ZxdgOutputV1] => OutputManagerState);

impl<BackendData> AsRef<ShmState> for AnvilState<BackendData> {
    fn as_ref(&self) -> &ShmState {
        &self.shm_state
    }
}
#[cfg(feature = "winit")]
delegate_shm!(AnvilState<crate::winit::WinitData>);
#[cfg(feature = "x11")]
delegate_shm!(AnvilState<crate::x11::X11Data>);
#[cfg(feature = "udev")]
delegate_shm!(AnvilState<crate::udev::UdevData>);

impl<BackendData> SeatHandler for AnvilState<BackendData> {
    fn seat_state(&mut self) -> &mut SeatState<AnvilState<BackendData>> {
        &mut self.seat_state
    }
}
#[cfg(feature = "winit")]
delegate_seat!(AnvilState<crate::winit::WinitData>);
#[cfg(feature = "x11")]
delegate_seat!(AnvilState<crate::x11::X11Data>);
#[cfg(feature = "udev")]
delegate_seat!(AnvilState<crate::udev::UdevData>);

impl<BackendData> XdgActivationHandler for AnvilState<BackendData> {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface
    ) {
        if token_data.timestamp.elapsed().as_secs() < 10 {
            // Just grant the wish
            let w = self.space.window_for_surface(&surface).cloned();
            if let Some(window) = w {
                self.space.raise_window(&window, true);
            }
        } else{
            // Discard the request
            self.xdg_activation_state.remove_request(&token);
        }
    }

    fn destroy_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface
    ) {
        // The request is cancelled
    }
}
#[cfg(feature = "winit")]
delegate_xdg_activation!(AnvilState<crate::winit::WinitData>);
#[cfg(feature = "x11")]
delegate_xdg_activation!(AnvilState<crate::x11::X11Data>);
#[cfg(feature = "udev")]
delegate_xdg_activation!(AnvilState<crate::udev::UdevData>);

impl<BackendData> XdgDecorationHandler for AnvilState<BackendData> {
    fn new_decoration(
        &mut self, 
        dh: &mut DisplayHandle<'_>, 
        toplevel: ToplevelSurface
    ) {
        use xdg_decoration::v1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ClientSide);
        });
        toplevel.send_configure(dh);
    }
    fn request_mode(
        &mut self, 
        _dh: &mut DisplayHandle<'_>, 
        _toplevel: ToplevelSurface, 
        _mode: DecorationMode,
    ) {}
    fn unset_mode(
        &mut self, 
        _dh: &mut DisplayHandle<'_>, 
        _toplevel: ToplevelSurface
    ) {}
}

#[cfg(feature = "winit")]
delegate_xdg_decoration!(AnvilState<crate::winit::WinitData>);
#[cfg(feature = "x11")]
delegate_xdg_decoration!(AnvilState<crate::x11::X11Data>);
#[cfg(feature = "udev")]
delegate_xdg_decoration!(AnvilState<crate::udev::UdevData>);

#[cfg(feature = "winit")]
delegate_xdg_shell!(AnvilState<crate::winit::WinitData>);
#[cfg(feature = "x11")]
delegate_xdg_shell!(AnvilState<crate::x11::X11Data>);
#[cfg(feature = "udev")]
delegate_xdg_shell!(AnvilState<crate::udev::UdevData>);

#[cfg(feature = "winit")]
delegate_global_dispatch!(AnvilState<crate::winit::WinitData>: [ZwlrLayerShellV1] => WlrLayerShellState);
#[cfg(feature = "winit")]
delegate_dispatch!(AnvilState<crate::winit::WinitData>: [ZwlrLayerShellV1, ZwlrLayerSurfaceV1] => WlrLayerShellState);

impl/*<BackendData: Backend + 'static>*/ AnvilState<crate::winit::WinitData>//BackendData>
{
    pub fn init(
        mut display: &mut Display<AnvilState<crate::winit::WinitData>>,//<BackendData>>,
        handle: LoopHandle<'static, CalloopData<crate::winit::WinitData>>,//<BackendData>>,
        backend_data: crate::winit::WinitData,//BackendData,
        log: slog::Logger,
        listen_on_socket: bool,
    ) -> AnvilState<crate::winit::WinitData> {//BackendData> {
        // init wayland clients
        let socket_name = if listen_on_socket {
            let source = ListeningSocketSource::new_auto(log.clone()).unwrap();
            let socket_name = source.socket_name().to_string_lossy().into_owned();
            handle.insert_source(
                source,
                |client_stream, _, data| {
                    use std::os::unix::io::AsRawFd;

                    data.state.handle.insert_source(
                        Generic::new(client_stream.as_raw_fd(), Interest::READ, Mode::Level),
                        |_, _, data| {
                            data.display.dispatch_clients(&mut data.state).unwrap();
                            Ok(PostAction::Continue)
                        },
                    ).unwrap();
                    data.display.insert_client(client_stream, Arc::new(ClientState));
                    data.display.dispatch_clients(&mut data.state).unwrap();
                }
            ).expect("Failed to init wayland socket source");
            info!(log, "Listening on wayland socket"; "name" => socket_name.clone());
            ::std::env::set_var("WAYLAND_DISPLAY", &socket_name);
            Some(socket_name)
        } else {
            None
        };
    
        // init globals
        let compositor_state = CompositorState::new(display, log.clone());
        let data_device_state = DataDeviceState::new(display, log.clone());
        let layer_shell_state = WlrLayerShellState::new(display, log.clone());
        let output_manager_state = OutputManagerState::new();
        let seat_state = SeatState::new();
        let shm_state = ShmState::new(display, vec![], log.clone());
        let xdg_activation_state = XdgActivationState::new(display, log.clone());
        let xdg_decoration_state = XdgDecorationManager::new(display, log.clone()).0;
        let xdg_shell_state = XdgShellState::new(display, log.clone()).0;
        
        // init input
        let seat_name = backend_data.seat_name();
        let mut seat = Seat::new(&mut display, seat_name.clone(), log.clone());
        
        let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));
        let cursor_status2 = cursor_status.clone();
        seat.add_pointer(&mut display.handle(), move |new_status| {
            *cursor_status2.lock().unwrap() = new_status
        });
        
        seat
            .add_keyboard(&mut display.handle(), XkbConfig::default(), 200, 25, |dh, seat, focus| {
                let focus = focus.and_then(|s| dh.get_client(s.id()).ok());
                set_data_device_focus(dh, seat, focus)
            })
            .expect("Failed to initialize the keyboard");

        /*
        init_tablet_manager_global(&mut display.borrow_mut());

        let cursor_status3 = cursor_status.clone();
        seat.tablet_seat().on_cursor_surface(move |_tool, new_status| {
            // TODO: tablet tools should have their own cursors
            *cursor_status3.lock().unwrap() = new_status;
        });
        */

        #[cfg(feature = "xwayland")]
        let xwayland = {
            let (xwayland, channel) = XWayland::new(handle.clone(), display.clone(), log.clone());
            let ret = handle.insert_source(channel, |event, _, anvil_state| match event {
                XWaylandEvent::Ready { connection, client } => anvil_state.xwayland_ready(connection, client),
                XWaylandEvent::Exited => anvil_state.xwayland_exited(),
            });
            if let Err(e) = ret {
                error!(
                    log,
                    "Failed to insert the XWaylandSource into the event loop: {}", e
                );
            }
            xwayland
        };

        AnvilState {
            backend_data,
            socket_name,
            running: Arc::new(AtomicBool::new(true)),
            handle,
            space: Space::new(log.clone()),
            popups: PopupManager::new(log.clone()),
            compositor_state,
            data_device_state,
            layer_shell_state,
            output_manager_state,
            seat_state,
            shm_state,
            xdg_activation_state,
            xdg_decoration_state,
            xdg_shell_state,
            dnd_icon: None,
            log,
            suppressed_keys: Vec::new(),
            pointer_location: (0.0, 0.0).into(),
            cursor_status,
            seat_name,
            seat,
            start_time: std::time::Instant::now(),
            #[cfg(feature = "xwayland")]
            xwayland,
        }
    }
}

pub trait Backend {
    fn seat_name(&self) -> String;
    fn reset_buffers(&mut self, output: &Output);
    fn early_import(&mut self, surface: &WlSurface);
}
