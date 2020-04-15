use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use smithay::{
    backend::{egl::EGLGraphicsBackend, graphics::gl::GLGraphicsBackend, input::InputBackend, winit},
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
};

use slog::Logger;

use crate::buffer_utils::BufferUtils;
use crate::glium_drawer::GliumDrawer;
use crate::input_handler::AnvilInputHandler;
use crate::shell::init_shell;
use crate::AnvilState;

pub fn run_winit(
    display: &mut Display,
    event_loop: &mut EventLoop<AnvilState>,
    log: Logger,
) -> Result<(), ()> {
    let (renderer, mut input) = winit::init(log.clone()).map_err(|_| ())?;

    #[cfg(feature = "egl")]
    let egl_buffer_reader = Rc::new(RefCell::new(
        if let Ok(egl_buffer_reader) = renderer.bind_wl_display(&display) {
            info!(log, "EGL hardware-acceleration enabled");
            Some(egl_buffer_reader)
        } else {
            None
        },
    ));

    let (w, h) = renderer.get_framebuffer_dimensions();
    #[cfg(feature = "egl")]
    let drawer = GliumDrawer::init(renderer, egl_buffer_reader.clone(), log.clone());
    #[cfg(not(feature = "egl"))]
    let drawer = GliumDrawer::init(renderer, log.clone());

    #[cfg(feature = "egl")]
    let buffer_utils = BufferUtils::new(egl_buffer_reader, log.clone());
    #[cfg(not(feature = "egl"))]
    let buffer_utils = BufferUtils::new(log.clone());

    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    info!(log, "Listening on wayland socket"; "name" => name.clone());
    ::std::env::set_var("WAYLAND_DISPLAY", name);

    let running = Arc::new(AtomicBool::new(true));

    /*
     * Initialize the globals
     */

    init_shm_global(display, vec![], log.clone());

    let (compositor_token, _, _, window_map) = init_shell(display, buffer_utils, log.clone());

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
        compositor_token,
        log.clone(),
    );

    let (mut seat, _) = Seat::new(display, "winit".into(), compositor_token, log.clone());

    let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));

    let cursor_status2 = cursor_status.clone();
    let pointer = seat.add_pointer(compositor_token.clone(), move |new_status| {
        // TODO: hide winit system cursor when relevant
        *cursor_status2.lock().unwrap() = new_status
    });

    let keyboard = seat
        .add_keyboard(XkbConfig::default(), 1000, 500, |seat, focus| {
            set_data_device_focus(seat, focus.and_then(|s| s.as_ref().client()))
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

    while running.load(Ordering::SeqCst) {
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
                    if surface.as_ref().is_alive() {
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
                    reset = !surface.as_ref().is_alive();
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

        let mut state = AnvilState::default();

        if event_loop
            .dispatch(Some(::std::time::Duration::from_millis(16)), &mut state)
            .is_err()
        {
            running.store(false, Ordering::SeqCst);
        } else {
            if state.need_wayland_dispatch {
                display
                    .dispatch(std::time::Duration::from_millis(0), &mut state)
                    .unwrap();
            }
            display.flush_clients(&mut state);
            window_map.borrow_mut().refresh();
        }
    }

    // Cleanup stuff
    window_map.borrow_mut().clear();

    Ok(())
}
