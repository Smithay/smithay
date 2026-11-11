//! Map a window without xdg_decoration, and do the late `zxdg_toplevel_decoration_v1` creation

use std::time::Duration;

use smithay_client_toolkit::reexports::{calloop, client as wayland_client, protocols};

use protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::{self, Mode, ZxdgToplevelDecorationV1},
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_shell,
    delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
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
use tracing::{debug, info};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_output, wl_surface::WlSurface},
};

fn main() {
    test_clients::init_logging();

    let (mut event_loop, globals, qh) = test_clients::init_connection::<App>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let decoration_manager: ZxdgDecorationManagerV1 = globals.bind(&qh, 2..=2, ()).unwrap();

    // No SCTK decoration, the decoration object is created later
    let window = xdg_shell.create_window(compositor.create_surface(&qh), WindowDecorations::None, &qh);
    window.commit();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        pool: SlotPool::new(256 * 256 * 4, &shm).unwrap(),
        shm,
        buffer: None,
        shift: 0,
        window,
        decoration_manager,
        decoration: None,
        loop_signal: event_loop.get_signal(),
        qh,
    };

    let mut step = 0;
    let delay = Duration::from_secs(2);
    // Just ansii bold + color to make it easier to see the expected mode at glance
    let csd = "\x1b[1;31mCSD\x1b[0m";
    let ssd = "\x1b[1;34mSSD\x1b[0m";

    event_loop
        .handle()
        .insert_source(calloop::timer::Timer::from_duration(delay), move |_, _, app| {
            match step {
                0 => {
                    info!("1. Late init + Request SSD. Expect {ssd}");
                    app.create_decoration(Some(Mode::ServerSide));
                    app.window.commit();
                }
                1 => {
                    info!("2. Decoration destroy + Commit. Expect {csd}");
                    app.destroy_decoration();
                    app.window.commit();
                }
                2 => {
                    info!("3. Decoration recreate + Request SSD. Expect {ssd}");
                    app.create_decoration(Some(Mode::ServerSide));
                    app.window.commit();
                }
                3 => {
                    info!("4. Decoration destroy + No Commit + Recreate. Expect {ssd}");
                    app.destroy_decoration();
                    app.create_decoration(None);
                    app.window.commit();
                }
                4 => {
                    info!("5. Maximize, to make sure we get SSD configure. Expect {ssd}");
                    app.window.set_maximized();
                }
                5 => {
                    info!("6. Unmaximize, to make sure we get SSD configure. Expect {ssd}");
                    app.window.unset_maximized();
                }
                6 => {
                    info!("7. Request CSD. Expect {csd}");
                    app.set_mode(Mode::ClientSide);
                    app.window.commit();
                }
                7 => {
                    info!("8. Decoration destroy + No Commit + Recreate. Expect {csd}");
                    app.destroy_decoration();
                    app.create_decoration(None);
                    app.window.commit();
                }
                8 => {
                    info!("9. Request SSD. Expect {ssd}");
                    app.set_mode(Mode::ServerSide);
                    app.window.commit();
                }
                9 => {
                    info!("10. Decoration destroy + Commit + Recreate + Commit. Expect {csd}");
                    app.destroy_decoration();
                    app.window.commit();
                    app.create_decoration(None);
                    app.window.commit();
                }
                _ => {
                    app.loop_signal.stop();
                    return calloop::timer::TimeoutAction::Drop;
                }
            }

            step += 1;
            calloop::timer::TimeoutAction::ToDuration(delay)
        })
        .unwrap();

    event_loop.run(None, &mut app, |_| {}).unwrap();
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    buffer: Option<Buffer>,
    shift: u32,
    window: Window,
    decoration_manager: ZxdgDecorationManagerV1,
    decoration: Option<ZxdgToplevelDecorationV1>,
    loop_signal: calloop::LoopSignal,
    qh: QueueHandle<Self>,
}

impl App {
    fn create_decoration(&mut self, mode: Option<Mode>) {
        let decoration =
            self.decoration_manager
                .get_toplevel_decoration(self.window.xdg_toplevel(), &self.qh, ());
        if let Some(mode) = mode {
            decoration.set_mode(mode);
        }
        self.decoration = Some(decoration);
    }

    fn destroy_decoration(&mut self) {
        if let Some(decoration) = self.decoration.take() {
            decoration.destroy();
        }
    }

    fn set_mode(&self, mode: Mode) {
        if let Some(decoration) = &self.decoration {
            decoration.set_mode(mode);
        }
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.loop_signal.stop();
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        self.buffer = None;
        test_clients::draw(
            qh,
            self.window.wl_surface(),
            &mut self.pool,
            &mut self.buffer,
            configure.new_size.0.map_or(256, |w| w.get()),
            configure.new_size.1.map_or(256, |h| h.get()),
            &mut self.shift,
        );
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            debug!("Decoration configure: {mode:?}");
        }
    }
}

impl CompositorHandler for App {
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
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

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
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

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);
delegate_noop!(App: ZxdgDecorationManagerV1);
