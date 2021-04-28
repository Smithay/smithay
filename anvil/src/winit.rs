use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

//#[cfg(feature = "egl")]
//use smithay::backend::egl::EGLGraphicsBackend;
use smithay::{
    backend::{renderer::Renderer, input::InputBackend, winit, SwapBuffersError},
    reexports::{
        calloop::EventLoop,
        wayland_server::{protocol::wl_output, Display},
    },
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use slog::Logger;

use crate::state::AnvilState;
use crate::buffer_utils::BufferUtils;
use crate::drawing::*;

pub fn run_winit(
    display: Rc<RefCell<Display>>,
    event_loop: &mut EventLoop<AnvilState>,
    log: Logger,
) -> Result<(), ()> {
    let (mut renderer, mut input) = winit::init(log.clone()).map_err(|err| {
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

    let (w, h): (u32, u32) = renderer.window_size().physical_size.into();

    /*
     * Initialize the globals
     */

    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        buffer_utils.clone(),
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

    let (texture_send, texture_receive) = std::sync::mpsc::channel();
    while state.running.load(Ordering::SeqCst) {
        if input
            .dispatch_new_events(|event, _| state.process_input_event(event))
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
            break;
        }

        // Send frame events so that client start drawing their next frame
        state
            .window_map
            .borrow()
            .send_frames(start_time.elapsed().as_millis() as u32);
        display.borrow_mut().flush_clients(&mut state);

        // drawing logic
        {
            renderer.begin().expect("Failed to render frame");
            renderer.clear([0.8, 0.8, 0.9, 1.0]).expect("Failed to clear frame");

            // draw the windows
            draw_windows(&mut renderer, 0, &texture_send, &buffer_utils, &*state.window_map.borrow(), None, state.ctoken, &log).expect("Failed to renderer windows");

            let (x, y) = *state.pointer_location.borrow();
            // draw the dnd icon if any
            {
                let guard = state.dnd_icon.lock().unwrap();
                if let Some(ref surface) = *guard {
                    if surface.as_ref().is_alive() {
                        draw_dnd_icon(&mut renderer, 0, &texture_send, &buffer_utils, surface, (x as i32, y as i32), state.ctoken, &log).expect("Failed to render dnd icon");
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
                    renderer.window().set_cursor_visible(false);
                    draw_cursor(&mut renderer, 0, &texture_send, &buffer_utils, surface, (x as i32, y as i32), state.ctoken, &log).expect("Failed to render cursor");
                } else {
                    renderer.window().set_cursor_visible(true);
                }
            }

            if let Err(SwapBuffersError::ContextLost(err)) = renderer.finish() {
                error!(log, "Critical Rendering Error: {}", err);
                state.running.store(false, Ordering::SeqCst);
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

        while let Ok(texture) = texture_receive.try_recv() {
            let _ = renderer.destroy_texture(texture);
        }
    }

    // Cleanup stuff
    state.window_map.borrow_mut().clear();

    Ok(())
}
