//! Test client for the wlr-virtual-pointer protocol.
//!
//! Creates a window and a virtual pointer, then runs a slow sequence of
//! events so cursor movement is plainly visible on screen:
//!
//!  1. Moves the pointer to the top-left corner of the output (absolute)
//!  2. Sweeps diagonally across the output over ~2 seconds (many small relative moves)
//!  3. Pauses for 1 second so the cursor is clearly visible
//!  4. Clicks the left mouse button
//!  5. Scrolls vertically three notches
//!
//! Run against a compositor that implements the protocol and watch the cursor
//! move on screen. Pointer events received by the window are logged.

use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop,
        client as wayland_client,
        protocols_wlr::virtual_pointer::v1::client::{
            zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
            zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};

use wayland_client::{
    Connection, QueueHandle,
    delegate_noop,
    protocol::{
        wl_output::WlOutput,
        wl_pointer::{self, WlPointer},
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
};

use tracing::info;

fn main() {
    test_clients::init_logging();

    let (mut event_loop, globals, qh) = test_clients::init_connection::<App>();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let pool = SlotPool::new(256 * 256 * 4, &shm).unwrap();

    // Bind the seat before committing the surface so wl_seat.capabilities
    // arrives before xdg_surface.configure.
    let seat_state = SeatState::new(&globals, &qh);

    let virtual_pointer_manager = globals
        .bind::<ZwlrVirtualPointerManagerV1, _, _>(&qh, 1..=2, ())
        .expect("compositor does not support zwlr_virtual_pointer_manager_v1");

    let surface = compositor_state.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("test-virtual-pointer");
    window.set_min_size(Some((256, 256)));
    window.commit();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        seat_state,
        shm,
        virtual_pointer_manager,
        virtual_pointer: None,
        seat: None,
        pointer: None,
        window,
        pool,
        buffer: None,
        width: 256,
        height: 256,
        shift: 0,
        first_configure: true,
        step: TestStep::WaitForMap,
        step_due: Instant::now(),
        sweep_remaining: 0,
        loop_signal: event_loop.get_signal(),
    };

    event_loop.run(None, &mut app, |_| {}).unwrap();
}

/// Ordered sequence of actions the test performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestStep {
    WaitForMap,
    /// Jump the cursor to the output top-left corner.
    AbsoluteMotion,
    /// Glide diagonally across the output with many small relative moves.
    Sweep,
    /// Sit still for a moment so the cursor position is plainly visible.
    Pause,
    /// Press and release left mouse button.
    ButtonClick,
    /// Send a wheel scroll event.
    Scroll,
    Done,
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    #[allow(dead_code)]
    compositor_state: CompositorState,
    seat_state: SeatState,
    shm: Shm,
    virtual_pointer_manager: ZwlrVirtualPointerManagerV1,
    virtual_pointer: Option<ZwlrVirtualPointerV1>,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    window: Window,
    pool: SlotPool,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
    shift: u32,
    first_configure: bool,
    step: TestStep,
    /// When the next timed step should fire.
    step_due: Instant,
    /// Frames remaining in the Sweep step.
    sweep_remaining: u32,
    loop_signal: calloop::LoopSignal,
}

impl App {
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        test_clients::draw(
            qh,
            self.window.wl_surface(),
            &mut self.pool,
            &mut self.buffer,
            self.width,
            self.height,
            &mut self.shift,
        );
    }

    fn vp(&mut self, qh: &QueueHandle<Self>) -> ZwlrVirtualPointerV1 {
        if self.virtual_pointer.is_none() {
            let vp = self
                .virtual_pointer_manager
                .create_virtual_pointer(None, qh, ());
            self.virtual_pointer = Some(vp);
        }
        self.virtual_pointer.clone().unwrap()
    }

    fn advance(&mut self, qh: &QueueHandle<Self>) {
        let time: u32 = 0;

        match self.step {
            TestStep::WaitForMap => {
                info!("Window mapped — jumping cursor to top-left corner");
                self.step = TestStep::AbsoluteMotion;
                self.step_due = Instant::now();
                self.advance(qh);
            }

            TestStep::AbsoluteMotion => {
                // (0, 0, 1, 1) maps to the top-left corner of the output.
                let vp = self.vp(qh);
                vp.motion_absolute(time, 0, 0, 1, 1);
                vp.frame();
                self.window.wl_surface().commit();
                info!("Cursor at top-left — sweeping diagonally for ~2 seconds");
                self.step = TestStep::Sweep;
                // ~120 frames × 4px right + 3px down ≈ 480×360 px diagonal sweep
                self.sweep_remaining = 120;
            }

            TestStep::Sweep => {
                let vp = self.vp(qh);
                vp.motion(time, 4.0, 3.0);
                vp.frame();
                self.window.wl_surface().commit();
                self.sweep_remaining -= 1;
                if self.sweep_remaining == 0 {
                    info!("Sweep done — pausing 1 second so cursor is visible");
                    self.step = TestStep::Pause;
                    self.step_due = Instant::now() + Duration::from_secs(1);
                }
            }

            TestStep::Pause => {
                info!("Sending left button click");
                self.step = TestStep::ButtonClick;
                self.step_due = Instant::now() + Duration::from_secs(2);
                self.advance(qh);
            }

            TestStep::ButtonClick => {
                let vp = self.vp(qh);
                const BTN_LEFT: u32 = 0x110;
                vp.button(time, BTN_LEFT, wl_pointer::ButtonState::Pressed);
                vp.frame();
                vp.button(time, BTN_LEFT, wl_pointer::ButtonState::Released);
                vp.frame();
                self.window.wl_surface().commit();
                info!("Click sent — waiting 2 seconds then scrolling");
                self.step = TestStep::Scroll;
                self.step_due = Instant::now() + Duration::from_secs(2);
            }

            TestStep::Scroll => {
                let vp = self.vp(qh);
                vp.axis_source(wl_pointer::AxisSource::Wheel);
                vp.axis_discrete(time, wl_pointer::Axis::VerticalScroll, 30.0, 3);
                vp.frame();
                self.window.wl_surface().commit();
                info!("Scroll sent — done in 2 seconds");
                self.step = TestStep::Done;
                self.step_due = Instant::now() + Duration::from_secs(2);
            }

            TestStep::Done => {
                info!("All virtual pointer events sent — stopping");
                if let Some(vp) = self.virtual_pointer.take() {
                    vp.destroy();
                }
                self.loop_signal.stop();
            }
        }
    }
}

// ─── Protocol implementations ────────────────────────────────────────────────

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wayland_client::protocol::wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &WlSurface, _: u32) {
        match self.step {
            TestStep::WaitForMap => {}
            // Sweep fires every frame to produce smooth motion.
            TestStep::Sweep => self.advance(qh),
            // All other steps wait for their scheduled time.
            _ if Instant::now() >= self.step_due => self.advance(qh),
            _ => {}
        }
        self.draw(qh);
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.loop_signal.stop();
    }

    fn configure(
        &mut self,
        _conn: &Connection,
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
            self.draw(qh);
            self.step = TestStep::WaitForMap;
            self.advance(qh);
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        if self.seat.is_none() {
            self.seat = Some(seat);
        }
    }
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        cap: Capability,
    ) {
        if cap == Capability::Pointer && self.pointer.is_none() {
            self.pointer = Some(self.seat_state.get_pointer(qh, &seat).unwrap());
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        cap: Capability,
    ) {
        if cap == Capability::Pointer {
            if let Some(ptr) = self.pointer.take() {
                ptr.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match &event.kind {
                PointerEventKind::Enter { .. } => {
                    info!("Pointer entered surface @{:?}", event.position)
                }
                PointerEventKind::Leave { .. } => info!("Pointer left surface"),
                PointerEventKind::Motion { .. } => info!("Pointer motion: {:?}", event.position),
                PointerEventKind::Press { button, .. } => info!("Button pressed: {button:#010x}"),
                PointerEventKind::Release { button, .. } => {
                    info!("Button released: {button:#010x}")
                }
                PointerEventKind::Axis { horizontal, vertical, .. } => {
                    info!("Axis: horizontal={:?} vertical={:?}", horizontal, vertical)
                }
            }
        }
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
    registry_handlers![OutputState];
}

// The virtual pointer objects have no events going server→client, so noop impls suffice.
delegate_noop!(App: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(App: ignore ZwlrVirtualPointerV1);

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);
delegate_seat!(App);
delegate_pointer!(App);
