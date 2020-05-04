use std::{
    cell::RefCell,
    rc::Rc,
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use smithay::{
    backend::{egl::EGLGraphicsBackend, graphics::gl::GLGraphicsBackend, input::InputBackend, winit},
    reexports::{
        calloop::EventLoop,
        wayland_server::{protocol::wl_output, Display},
    },
    wayland::{
        data_device::set_data_device_focus,
        output::{Mode, Output, PhysicalProperties},
        seat::{CursorImageStatus, Seat, XkbConfig},
        SERIAL_COUNTER as SCOUNTER,
    },
};

use slog::Logger;

use crate::buffer_utils::BufferUtils;
use crate::glium_drawer::GliumDrawer;
use crate::input_handler::{AnvilInputHandler, InputInitData};
use crate::state::AnvilState;

pub fn run_winit(
    display: Rc<RefCell<Display>>,
    event_loop: &mut EventLoop<AnvilState>,
    log: Logger,
) -> Result<(), ()> {
    let (renderer, mut input) = winit::init(log.clone()).map_err(|err| {
        slog::crit!(log, "Failed to initialize Winit backend: {}", err);
    })?;

    #[cfg(feature = "egl")]
    let egl_buffer_reader = Rc::new(RefCell::new(
        if let Ok(egl_buffer_reader) = renderer.bind_wl_display(&display.borrow()) {
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

    /*
     * Initialize the globals
     */

    let mut state = AnvilState::init(display.clone(), event_loop.handle(), buffer_utils, log.clone());

    let (mut seat, _) = Seat::new(
        &mut display.borrow_mut(),
        "winit".into(),
        state.ctoken,
        log.clone(),
    );

    let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));

    let cursor_status2 = cursor_status.clone();
    let pointer = seat.add_pointer(state.ctoken.clone(), move |new_status| {
        // TODO: hide winit system cursor when relevant
        *cursor_status2.lock().unwrap() = new_status
    });

    let keyboard = seat
        .add_keyboard(XkbConfig::default(), 200, 25, |seat, focus| {
            set_data_device_focus(seat, focus.and_then(|s| s.as_ref().client()))
        })
        .expect("Failed to initialize the keyboard");

    let (output, _) = Output::new(
        &mut display.borrow_mut(),
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

    let mut input_handler = AnvilInputHandler::new(
        log.clone(),
        InputInitData {
            pointer,
            keyboard,
            window_map: state.window_map.clone(),
            screen_size: (0, 0),
            running: state.running.clone(),
            pointer_location: pointer_location.clone(),
        },
    );

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        input
            .dispatch_new_events(|event, _| input_handler.process_event(event))
            .unwrap();

        // drawing logic
        {
            use glium::Surface;
            let mut frame = drawer.draw();
            frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);

            // draw the windows
            drawer.draw_windows(&mut frame, &*state.window_map.borrow(), state.ctoken);

            let (x, y) = *pointer_location.borrow();
            // draw the dnd icon if any
            {
                let guard = state.dnd_icon.lock().unwrap();
                if let Some(ref surface) = *guard {
                    if surface.as_ref().is_alive() {
                        drawer.draw_dnd_icon(&mut frame, surface, (x as i32, y as i32), state.ctoken);
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
                    drawer.draw_cursor(&mut frame, surface, (x as i32, y as i32), state.ctoken);
                }
            }

            if let Err(err) = frame.finish() {
                error!(log, "Error during rendering: {:?}", err);
            }
        }
        // Send frame events so that client start drawing their next frame
        state.window_map.borrow().send_frames(SCOUNTER.next_serial());

        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            display.borrow_mut().flush_clients(&mut state);
            state.window_map.borrow_mut().refresh();
        }
    }

    // Cleanup stuff
    state.window_map.borrow_mut().clear();

    Ok(())
}
