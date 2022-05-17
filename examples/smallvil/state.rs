use std::{os::unix::prelude::AsRawFd, sync::Arc};

use calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction};
use slog::Logger;
use smithay::{
    desktop::{Space, WindowSurfaceType},
    utils::{Logical, Point},
    wayland::{
        compositor::CompositorState,
        output::OutputManagerState,
        seat::{Seat, SeatState},
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::wl_surface::WlSurface,
    Client, Display,
};

use crate::CalloopData;

pub struct Smallvil {
    pub pointer_location: Point<f64, Logical>,

    pub start_time: std::time::Instant,
    pub clients: Vec<Client>,
    pub socket_name: String,

    pub space: Space,
    pub loop_signal: LoopSignal,
    pub log: slog::Logger,

    // Smithay State
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Smallvil>,
    pub seat: Seat<Self>,
}

impl Smallvil {
    pub fn new(event_loop: &mut EventLoop<CalloopData>, display: &mut Display<Self>, log: Logger) -> Self {
        let start_time = std::time::Instant::now();

        let compositor_state = CompositorState::new(display, log.clone());
        let xdg_shell_state = XdgShellState::new(display, log.clone()).0;
        let shm_state = ShmState::new(display, vec![], log.clone());
        let output_manager_state = OutputManagerState::new_with_xdg_output(display);
        let seat_state = SeatState::new();

        // A seat is a group of keyboards, pointer and touch devices.
        // A seat typically has a pointer and maintains a keyboard focus and a pointer focus.
        let mut seat: Seat<Self> = Seat::new(display, "winit", log.clone());

        // Notify clients that we have a keyboard, for the sake of the example we assume that keyboard is always present.
        // You may want to track keyboard hot-plug in real compositor.
        seat.add_keyboard(&mut display.handle(), Default::default(), 200, 200, |_, _| {})
            .unwrap();

        // Notify clients that we have a pointer (mouse)
        // Here we assume that there is always pointer plugged in
        seat.add_pointer(&mut display.handle(), |_| {});

        // A space represents a two-dimensional plane. Windows and Outputs can be mapped onto it.
        //
        // Windows get a position and stacking order through mapping.
        // Outputs become views of a part of the Space and can be rendered via Space::render_output.
        let space = Space::new(log.clone());

        let socket_name = Self::init_wayland_listener(event_loop, log.clone());

        // Get the loop signal, used to stop the event loop
        let loop_signal = event_loop.get_signal();

        Self {
            pointer_location: Default::default(),

            start_time,
            clients: Vec::new(),

            space,
            loop_signal,
            socket_name,

            log,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            seat,
        }
    }

    fn init_wayland_listener(event_loop: &mut EventLoop<CalloopData>, log: slog::Logger) -> String {
        // Creates a new listening socket, automatically choosing the next available `wayland` socket name.
        let listening_socket = ListeningSocketSource::new_auto(log).unwrap();

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket
            .socket_name()
            .to_os_string()
            .into_string()
            .unwrap();

        let handle = event_loop.handle();

        event_loop
            .handle()
            .insert_source(listening_socket, move |client_stream, _, state| {
                handle
                    .insert_source(
                        Generic::new(client_stream.as_raw_fd(), Interest::READ, Mode::Level),
                        |_, _, state| {
                            state.display.dispatch_clients(&mut state.state).unwrap();
                            Ok(PostAction::Continue)
                        },
                    )
                    .unwrap();

                // Inside the callback, you should insert the client into the display.
                //
                // You may also associate some data with the client when inserting the client.
                state
                    .display
                    .insert_client(client_stream, Arc::new(ClientState))
                    .unwrap();

                state.display.dispatch_clients(&mut state.state).unwrap();
            })
            .expect("Failed to init the wayland event source.");

        socket_name
    }

    pub fn surface_under_pointer(&self) -> Option<(WlSurface, Point<i32, Logical>)> {
        let pos = self.pointer_location;
        self.space
            .surface_under(pos, WindowSurfaceType::all())
            .map(|(_, surface, location)| (surface, location))
    }
}

pub struct ClientState;
impl ClientData<Smallvil> for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
