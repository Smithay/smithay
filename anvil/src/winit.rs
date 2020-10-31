use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

#[cfg(feature = "egl")]
use smithay::backend::egl::EGLGraphicsBackend;
use smithay::{
    backend::{graphics::gl::GLGraphicsBackend, input::InputBackend, winit},
    reexports::{
        calloop::EventLoop,
        wayland_server::{protocol::wl_output, Display},
        winit::window::CursorIcon,
    },
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use slog::Logger;

use crate::buffer_utils::BufferUtils;
use crate::glium_drawer::GliumDrawer;
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

    #[cfg(feature = "egl")]
    let buffer_utils = BufferUtils::new(egl_buffer_reader, log.clone());
    #[cfg(not(feature = "egl"))]
    let buffer_utils = BufferUtils::new(log.clone());

    let (w, h) = renderer.get_framebuffer_dimensions();
    let drawer = GliumDrawer::init(renderer, buffer_utils.clone(), log.clone());

    /*
     * Initialize the globals
     */

    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        buffer_utils,
        None,
        None,
        log.clone(),
    );

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

    let start_time = std::time::Instant::now();

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        input
            .dispatch_new_events(|event, _| state.process_input_event(event))
            .unwrap();

        // Send frame events so that client start drawing their next frame
        state
            .window_map
            .borrow()
            .send_frames(start_time.elapsed().as_millis() as u32);
        display.borrow_mut().flush_clients(&mut state);

        // drawing logic
        {
            use glium::Surface;
            let mut frame = drawer.draw();
            frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);

            // draw the windows
            drawer.draw_windows(&mut frame, &*state.window_map.borrow(), None, state.ctoken);

            let (x, y) = *state.pointer_location.borrow();
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
                let mut guard = state.cursor_status.lock().unwrap();
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
                    drawer.draw_software_cursor(&mut frame, surface, (x as i32, y as i32), state.ctoken);
                } else {
                    drawer.draw_hardware_cursor(&CursorIcon::Default, (0, 0), (x as i32, y as i32));
                }
            }

            if let Err(err) = frame.finish() {
                error!(log, "Error during rendering: {:?}", err);
            }
        }

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
