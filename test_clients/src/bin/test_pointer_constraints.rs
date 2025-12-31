//! Constrain the pointer to surface region

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_pointer, delegate_pointer_constraints, delegate_registry,
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop, client as wayland_client,
        protocols::wp::pointer_constraints::zv1::client::{
            zwp_confined_pointer_v1::ZwpConfinedPointerV1, zwp_locked_pointer_v1::ZwpLockedPointerV1,
            zwp_pointer_constraints_v1,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        pointer_constraints::{PointerConstraintsHandler, PointerConstraintsState},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};

use wayland_client::{
    delegate_noop,
    protocol::{
        wl_output::{self, WlOutput},
        wl_pointer::WlPointer,
        wl_region::WlRegion,
        wl_seat,
        wl_surface::{self, WlSurface},
    },
    Connection, QueueHandle,
};

use tracing::{info, warn};

fn main() {
    test_clients::init_logging();

    let (mut event_loop, globals, qh) = test_clients::init_connection::<App>();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();

    let surface = compositor_state.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);

    let shm = Shm::bind(&globals, &qh).unwrap();
    let pool = SlotPool::new(256 * 256 * 4, &shm).unwrap();

    let mut simple_window = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        seat_state: SeatState::new(&globals, &qh),
        shm,
        pointer: None,
        confined_pointer: None,
        pointer_constraint_state: PointerConstraintsState::bind(&globals, &qh),

        should_be_mapped: false,

        first_configure: true,
        pool,
        width: 256,
        height: 256,
        shift: 0,
        buffer: None,
        window,
        loop_signal: event_loop.get_signal(),
    };

    simple_window.map();

    event_loop.run(None, &mut simple_window, |_| {}).unwrap();
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor_state: CompositorState,
    seat_state: SeatState,
    shm: Shm,

    pointer: Option<WlPointer>,
    confined_pointer: Option<ZwpConfinedPointerV1>,
    pointer_constraint_state: PointerConstraintsState,

    should_be_mapped: bool,

    first_configure: bool,
    pool: SlotPool,
    width: u32,
    height: u32,
    shift: u32,
    buffer: Option<Buffer>,
    window: Window,
    loop_signal: calloop::LoopSignal,
}

impl CompositorHandler for App {
    fn frame(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        self.draw(conn, qh);
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: i32) {}
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wl_output::Transform,
    ) {
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.loop_signal.stop();
    }

    fn configure(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        self.buffer = None;
        self.width = configure.new_size.0.map(|v| v.get()).unwrap_or(256);
        self.height = configure.new_size.1.map(|v| v.get()).unwrap_or(256);

        if self.first_configure {
            self.first_configure = false;
            self.draw(conn, qh);
        }
    }
}

impl App {
    fn map(&mut self) {
        self.first_configure = true;
        self.should_be_mapped = true;
        self.window.commit();
    }

    fn draw(&mut self, _conn: &Connection, qh: &QueueHandle<Self>) {
        if !self.should_be_mapped {
            return;
        }

        test_clients::draw(
            qh,
            &self.window,
            &mut self.pool,
            &mut self.buffer,
            self.width,
            self.height,
            &mut self.shift,
        );
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);

delegate_xdg_shell!(App);
delegate_xdg_window!(App);

delegate_registry!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_pointer_constraints!(App);
delegate_noop!(App: WlRegion);

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        info!("add capability: {capability}");
        if capability == Capability::Pointer && self.pointer.is_none() {
            let pointer = self
                .seat_state
                .get_pointer(qh, &seat)
                .expect("Failed to create pointer");

            let region = self.compositor_state.wl_compositor().create_region(qh, ());
            region.add(0, 0, 64, 64);

            let confined_pointer = self
                .pointer_constraint_state
                .confine_pointer(
                    self.window.wl_surface(),
                    &pointer,
                    Some(&region),
                    zwp_pointer_constraints_v1::Lifetime::Persistent,
                    qh,
                )
                .unwrap();

            self.pointer = Some(pointer);
            self.confined_pointer = Some(confined_pointer);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        warn!("remove capability: {capability}");
        if capability == Capability::Pointer && self.pointer.is_some() {
            self.confined_pointer.take().unwrap().destroy();
            self.pointer.take().unwrap().release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    info!("Pointer entered @{:?}", event.position);
                }
                PointerEventKind::Leave { .. } => {
                    info!("Pointer left");
                }
                PointerEventKind::Motion { .. } => {
                    info!("Pointer motion: {:?}", event.position);
                }
                _ => {}
            }
        }
    }
}

impl PointerConstraintsHandler for App {
    fn confined(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _confined_pointer: &ZwpConfinedPointerV1,
        _surface: &WlSurface,
        _pointer: &WlPointer,
    ) {
        info!("Pointer confined");
    }

    fn unconfined(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _confined_pointer: &ZwpConfinedPointerV1,
        _surface: &WlSurface,
        _pointer: &WlPointer,
    ) {
        warn!("Pointer unconfined");
    }

    fn locked(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _locked_pointer: &ZwpLockedPointerV1,
        _surface: &WlSurface,
        _pointer: &WlPointer,
    ) {
    }

    fn unlocked(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _locked_pointer: &ZwpLockedPointerV1,
        _surface: &WlSurface,
        _pointer: &WlPointer,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState,];
}
