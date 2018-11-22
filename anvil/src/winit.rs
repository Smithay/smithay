use std::{
    cell::RefCell,
    rc::Rc,
    sync::{atomic::AtomicBool, Arc},
};

use smithay::{
    backend::{egl::EGLGraphicsBackend, graphics::gl::GLGraphicsBackend, input::InputBackend, winit},
    wayland::{
        data_device::{default_action_chooser, init_data_device, set_data_device_focus},
        output::{Mode, Output, PhysicalProperties},
        seat::{Seat, XkbConfig},
        shm::init_shm_global,
    },
    wayland_server::{calloop::EventLoop, protocol::wl_output, Display},
};

use slog::Logger;

use glium_drawer::GliumDrawer;
use input_handler::AnvilInputHandler;
use shell::init_shell;

pub fn run_winit(display: &mut Display, event_loop: &mut EventLoop<()>, log: Logger) -> Result<(), ()> {
    let (renderer, mut input) = winit::init(log.clone()).map_err(|_| ())?;

    let egl_display = Rc::new(RefCell::new(
        if let Ok(egl_display) = renderer.bind_wl_display(&display) {
            info!(log, "EGL hardware-acceleration enabled");
            Some(egl_display)
        } else {
            None
        },
    ));

    let (w, h) = renderer.get_framebuffer_dimensions();
    let drawer = GliumDrawer::init(renderer, egl_display, log.clone());

    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    info!(log, "Listening on wayland socket"; "name" => name.clone());
    ::std::env::set_var("WAYLAND_DISPLAY", name);

    let running = Arc::new(AtomicBool::new(true));

    /*
     * Initialize the globals
     */

    init_shm_global(display, vec![], log.clone());

    let (compositor_token, _, _, window_map) = init_shell(display, log.clone());

    init_data_device(
        display,
        |_| {},
        default_action_chooser,
        compositor_token.clone(),
        log.clone(),
    );

    let (mut seat, _) = Seat::new(display, "winit".into(), compositor_token.clone(), log.clone());

    let pointer = seat.add_pointer(compositor_token.clone(), |_| {});

    let keyboard = seat
        .add_keyboard(XkbConfig::default(), 1000, 500, |seat, focus| {
            set_data_device_focus(seat, focus.and_then(|s| s.client()))
        })
        .expect("Failed to initialize the keyboard");

    let (output, _) = Output::new(
        display,
        "Winit".into(),
        PhysicalProperties {
            width: 0,
            height: 0,
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );

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

    input.set_handler(AnvilInputHandler::new(
        log.clone(),
        pointer,
        keyboard,
        window_map.clone(),
        (0, 0),
        running.clone(),
        Rc::new(RefCell::new((0.0, 0.0))),
    ));

    info!(log, "Initialization completed, starting the main loop.");

    loop {
        input.dispatch_new_events().unwrap();

        drawer.draw_windows(&*window_map.borrow(), compositor_token, &log);

        event_loop
            .dispatch(Some(::std::time::Duration::from_millis(16)), &mut ())
            .unwrap();
        display.flush_clients();

        window_map.borrow_mut().refresh();
    }
}
