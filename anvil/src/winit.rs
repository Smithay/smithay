use std::{
    cell::RefCell,
    rc::Rc,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use smithay::{
    backend::{egl::EGLGraphicsBackend, graphics::gl::GLGraphicsBackend, input::InputBackend, winit},
    wayland::{
        data_device::{default_action_chooser, init_data_device, set_data_device_focus, DataDeviceEvent},
        output::{Mode, Output, PhysicalProperties},
        seat::{CursorImageStatus, Seat, XkbConfig},
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

    let dnd_icon = Arc::new(Mutex::new(None));

    let dnd_icon2 = dnd_icon.clone();
    init_data_device(
        display,
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
        compositor_token.clone(),
        log.clone(),
    );

    let (mut seat, _) = Seat::new(display, "winit".into(), compositor_token.clone(), log.clone());

    let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));

    let cursor_status2 = cursor_status.clone();
    let pointer = seat.add_pointer(compositor_token.clone(), move |new_status| {
        // TODO: hide winit system cursor when relevant
        *cursor_status2.lock().unwrap() = new_status
    });

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

    let pointer_location = Rc::new(RefCell::new((0.0, 0.0)));

    input.set_handler(AnvilInputHandler::new(
        log.clone(),
        pointer,
        keyboard,
        window_map.clone(),
        (0, 0),
        running.clone(),
        pointer_location.clone(),
    ));

    info!(log, "Initialization completed, starting the main loop.");

    loop {
        input.dispatch_new_events().unwrap();

        // drawing logic
        {
            use glium::Surface;
            let mut frame = drawer.draw();
            frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);

            // draw the windows
            drawer.draw_windows(&mut frame, &*window_map.borrow(), compositor_token);

            let (x, y) = *pointer_location.borrow();
            // draw the dnd icon if any
            {
                let guard = dnd_icon.lock().unwrap();
                if let Some(ref surface) = *guard {
                    if surface.is_alive() {
                        drawer.draw_dnd_icon(&mut frame, surface, (x as i32, y as i32), compositor_token);
                    }
                }
            }
            // draw the cursor as relevant
            {
                let mut guard = cursor_status.lock().unwrap();
                // reset the cursor if the surface is no longer alive
                let mut reset = false;
                if let CursorImageStatus::Image(ref surface) = *guard {
                    reset = !surface.is_alive();
                }
                if reset {
                    *guard = CursorImageStatus::Default;
                }
                // draw as relevant
                if let CursorImageStatus::Image(ref surface) = *guard {
                    drawer.draw_cursor(&mut frame, surface, (x as i32, y as i32), compositor_token);
                }
            }

            if let Err(err) = frame.finish() {
                error!(log, "Error during rendering: {:?}", err);
            }
        }

        event_loop
            .dispatch(Some(::std::time::Duration::from_millis(16)), &mut ())
            .unwrap();
        display.flush_clients();

        window_map.borrow_mut().refresh();
    }
}
