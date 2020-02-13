use std::{
    cell::RefCell,
    process::Command,
    rc::Rc,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use slog::{info, o, Drain, Logger};
use smithay::{
    backend::input::{InputBackend, MouseButton, MouseButtonState},
    reexports::{
        calloop::EventLoop,
        wayland_server::{protocol::wl_output, Display},
    },
    wayland::{
        data_device::{default_action_chooser, init_data_device, set_data_device_focus, DataDeviceEvent},
        output::{Mode, Output, PhysicalProperties},
        seat::{CursorImageStatus, Seat, XkbConfig},
        shm::init_shm_global,
    },
    utils::Rectangle,
};

use anvil::{
    buffer_utils::BufferUtils,
    input_handler::AnvilInputHandler,
    shell::{init_shell, MyCompositorToken, MyWindowMap, SurfaceData},
};

pub mod input;
use input::*;

pub struct Anvil {
    log: Logger,
    event_loop: EventLoop<()>,
    display: Display,
    socket: String,
    token: MyCompositorToken,
    window_map: Rc<RefCell<MyWindowMap>>,
    input: TestInputBackend,
    event_time_counter: u32,
}

impl Anvil {
    pub fn start() -> Self {
        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::FullFormat::new(decorator).build();
        let log = Logger::root(Mutex::new(drain).fuse(), o!());

        let event_loop = EventLoop::<()>::new().unwrap();
        let mut display = Display::new(event_loop.handle());

        // TODO: headless EGL?

        let buffer_utils = BufferUtils::new(log.clone());

        let name = display.add_socket_auto().unwrap().into_string().unwrap();
        info!(log, "Listening on wayland socket"; "name" => name.clone());

        let running = Arc::new(AtomicBool::new(true));

        init_shm_global(&mut display, vec![], log.clone());

        let (token, _, _, window_map) = init_shell(&mut display, buffer_utils, log.clone());

        let dnd_icon = Arc::new(Mutex::new(None));
        let dnd_icon2 = dnd_icon.clone();
        init_data_device(
            &mut display,
            move |event| match event {
                DataDeviceEvent::DnDStarted { icon, .. } => {
                    *dnd_icon2.lock().unwrap() = icon;
                }
                DataDeviceEvent::DnDDropped => {
                    *dnd_icon2.lock().unwrap() = None;
                }
                _ => {}
            },
            default_action_chooser,
            token,
            log.clone(),
        );

        let (mut seat, _) = Seat::new(&mut display, "test".into(), token, log.clone());

        let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));
        let cursor_status2 = cursor_status.clone();
        let pointer = seat.add_pointer(token.clone(), move |new_status| {
            *cursor_status2.lock().unwrap() = new_status
        });

        let keyboard = seat
            .add_keyboard(XkbConfig::default(), 1000, 500, |seat, focus| {
                set_data_device_focus(seat, focus.and_then(|s| s.as_ref().client()))
            })
            .expect("Failed to initialize the keyboard");

        let (output, _) = Output::new(
            &mut display,
            "Test".into(),
            PhysicalProperties {
                width: 0,
                height: 0,
                subpixel: wl_output::Subpixel::Unknown,
                make: "Smithay".into(),
                model: "Test".into(),
            },
            log.clone(),
        );

        let (w, h) = (1280, 720);
        output.change_current_state(
            Some(Mode {
                width: w as i32,
                height: h as i32,
                refresh: 60_000,
            }),
            None,
            None,
        );
        output.set_preferred(Mode {
            width: w as i32,
            height: h as i32,
            refresh: 60_000,
        });

        let pointer_location = Rc::new(RefCell::new((0.0, 0.0)));

        let mut input = TestInputBackend::new();
        input.set_handler(AnvilInputHandler::new(
            log.clone(),
            pointer,
            keyboard,
            window_map.clone(),
            (0, 0),
            running.clone(),
            pointer_location.clone(),
        ));

        info!(log, "Initialization completed.");

        Self {
            log,
            event_loop,
            display,
            socket: name,
            token,
            window_map,
            input,
            // Certain clients, particularly the Weston test clients, start up with their "last click time"
            // zeroed, so if we start from zero and click once they will assume it's the second click of a
            // double-click sequence, which is not what we want.
            event_time_counter: 100_000,
        }
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    pub fn log(&self) -> Logger {
        self.log.clone()
    }

    pub fn spawn_client(&self, command: &str) {
        info!(self.log, "Spawning client"; "command" => command);

        Command::new(command)
            .env("WAYLAND_DISPLAY", self.socket())
            .spawn()
            .unwrap();
    }

    pub fn wait_for_surface_map(&mut self) {
        info!(self.log, "Waiting for surface map...");

        loop {
            self.display.flush_clients();
            self.event_loop.dispatch(None, &mut ()).unwrap();

            let mut found = false;
            self.window_map
                .borrow()
                .with_windows_from_bottom_to_top(|toplevel, _| {
                    self.token
                        .with_surface_data(toplevel.get_surface().unwrap(), |attributes| {
                            let data = attributes.user_data.get_mut::<SurfaceData>().unwrap();
                            if data.buffer.is_some() {
                                found = true;
                            }
                        });
                });

            if found {
                info!(self.log, "Found a mapped surface, breaking");
                break;
            }
        }
    }

    pub fn wait_for_move(&mut self) {
        info!(self.log, "Waiting for move...");

        // TODO: how to best check this properly?
        // Ideally this should wait until an xdg or wl Move request is received and processed, and
        // then return so as to not depend on any implementation details.
        let mut i = 0;
        loop {
            self.display.flush_clients();
            self.event_loop.dispatch(None, &mut ()).unwrap();

            i += 1;
            if i == 5 {
                break;
            }
        }
    }

    pub fn window_location(&self) -> (i32, i32) {
        let mut location = None;
        self.window_map.borrow().with_windows_from_bottom_to_top(|_, l| {
            if location.is_some() {
                panic!("more than one window");
            }

            location = Some(l);
        });
        location.expect("there are no windows")
    }

    pub fn window_geometry(&self) -> Rectangle {
        let mut toplevel = None;
        self.window_map.borrow().with_windows_from_bottom_to_top(|t, _| {
            if toplevel.is_some() {
                panic!("more than one window");
            }

            toplevel = Some(t.clone());
        });
        let toplevel = toplevel.expect("there are no windows");
        self.window_map.borrow().geometry(&toplevel).unwrap()
    }

    pub fn push_event(&mut self, event: TestEvent) {
        self.input.push_event(event);
    }

    pub fn dispatch_events(&mut self) {
        self.input.dispatch_new_events().unwrap();
    }

    pub fn event_time(&mut self) -> u32 {
        let time = self.event_time_counter;
        self.event_time_counter += 1;
        time
    }

    pub fn pointer_move(&mut self, x: i32, y: i32) {
        let time = self.event_time();
        self.push_event(TestEvent::PointerMotionAbsolute(TestPointerMotionAbsoluteEvent {
            time,
            x: x as f64,
            y: y as f64,
        }));
        self.dispatch_events();
    }

    pub fn pointer_press(&mut self, button: MouseButton) {
        let time = self.event_time();
        self.push_event(TestEvent::PointerButton(TestPointerButtonEvent {
            time,
            button,
            state: MouseButtonState::Pressed,
        }));
        self.dispatch_events();
    }
}
