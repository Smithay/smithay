//! Attempt to reproduce https://github.com/Smithay/smithay/issues/1894

use smithay_client_toolkit::delegate_subcompositor;
use smithay_client_toolkit::reexports::client::protocol::wl_subsurface::WlSubsurface;
use smithay_client_toolkit::reexports::{calloop, client as wayland_client};

use smithay_client_toolkit::subcompositor::SubcompositorState;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_shell,
    delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
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
use tracing::info;
use wayland_client::{
    protocol::{
        wl_output::{self, WlOutput},
        wl_surface::{self, WlSurface},
    },
    Connection, QueueHandle,
};

fn main() {
    test_clients::init_logging();

    let (mut event_loop, globals, qh) = test_clients::init_connection::<App>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let subcompositor = SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();

    let surface = compositor.create_surface(&qh);
    let subsurface = subcompositor.create_subsurface(surface.clone(), &qh);

    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);

    let shm = Shm::bind(&globals, &qh).unwrap();
    let pool = SlotPool::new(256 * 256 * 4, &shm).unwrap();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,

        should_be_mapped: false,

        first_configure: true,
        pool,
        width: 256,
        height: 256,
        shift: 0,
        buffer: None,
        window,
        subsurface,
        commit_only_once: true,
        loop_signal: event_loop.get_signal(),
    };

    app.map_toggle();

    event_loop.run(None, &mut app, |_| {}).unwrap();
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,

    should_be_mapped: bool,

    first_configure: bool,
    pool: SlotPool,
    width: u32,
    height: u32,
    shift: u32,
    buffer: Option<Buffer>,
    window: Window,
    subsurface: (WlSubsurface, WlSurface),
    commit_only_once: bool,

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
        if self.first_configure {
            info!("Window initial configure");
        } else {
            info!("Window configure");
        }

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
    fn map_toggle(&mut self) {
        if self.should_be_mapped {
            self.unmap()
        } else {
            self.map()
        }
    }

    fn map(&mut self) {
        self.first_configure = true;
        self.should_be_mapped = true;
        self.window.commit();
    }

    fn unmap(&mut self) {
        self.buffer = None;
        self.first_configure = false;
        self.should_be_mapped = false;
        self.window.attach(None, 0, 0);
        self.window.commit();
    }

    fn draw(&mut self, _conn: &Connection, qh: &QueueHandle<Self>) {
        if !self.should_be_mapped {
            return;
        }

        if std::mem::take(&mut self.commit_only_once) {
            test_clients::draw(
                qh,
                &self.subsurface.1,
                &mut self.pool,
                &mut self.buffer,
                self.width,
                self.height,
                &mut self.shift,
            );
        }

        // Test if setting the pos and never calling commit on the subsurface still applies the pos on partent commit
        self.subsurface.0.set_position(self.shift as i32, 20);

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
}

delegate_compositor!(App);
delegate_subcompositor!(App);

delegate_output!(App);
delegate_shm!(App);

delegate_xdg_shell!(App);
delegate_xdg_window!(App);

delegate_registry!(App);

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
