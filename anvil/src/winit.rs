use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

use smithay::{
    backend::{input::InputBackend, renderer::Frame, winit, SwapBuffersError},
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

use crate::drawing::*;
use crate::state::AnvilState;

pub fn run_winit(
    display: Rc<RefCell<Display>>,
    event_loop: &mut EventLoop<AnvilState>,
    log: Logger,
) -> Result<(), ()> {
    let (renderer, mut input) = winit::init(log.clone()).map_err(|err| {
        slog::crit!(log, "Failed to initialize Winit backend: {}", err);
    })?;
    let renderer = Rc::new(RefCell::new(renderer));

    #[cfg(feature = "egl")]
    let reader = renderer.borrow().bind_wl_display(&display.borrow()).ok();
    #[cfg(not(feature = "egl"))]
    let reader = None;

    #[cfg(feature = "egl")]
    if reader.is_some() {
        info!(log, "EGL hardware-acceleration enabled");
    };

    let (w, h): (u32, u32) = renderer.borrow().window_size().physical_size.into();

    /*
     * Initialize the globals
     */

    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        #[cfg(feature = "egl")]
        Rc::new(RefCell::new(reader.clone())),
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
    let mut cursor_visible = true;

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        if input
            .dispatch_new_events(|event, _| state.process_input_event(event))
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
            break;
        }

        // drawing logic
        {
            let mut renderer = renderer.borrow_mut();


            let result = renderer.render(|renderer, frame| {
                frame.clear([0.8, 0.8, 0.9, 1.0])?;

                // draw the windows
                draw_windows(
                    renderer,
                    frame,
                    reader.as_ref(),
                    &*state.window_map.borrow(),
                    None,
                    state.ctoken,
                    &log,
                )?;

                let (x, y) = *state.pointer_location.borrow();
                // draw the dnd icon if any
                {
                    let guard = state.dnd_icon.lock().unwrap();
                    if let Some(ref surface) = *guard {
                        if surface.as_ref().is_alive() {
                            draw_dnd_icon(
                                renderer,
                                frame,
                                surface,
                                reader.as_ref(),
                                (x as i32, y as i32),
                                state.ctoken,
                                &log,
                            )?;
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
                        cursor_visible = false;
                        draw_cursor(
                            renderer,
                            frame,
                            surface,
                            reader.as_ref(),
                            (x as i32, y as i32),
                            state.ctoken,
                            &log,
                        )?;
                    } else {
                        cursor_visible = true;
                    }
                }

                Ok(())
            }).map_err(Into::<SwapBuffersError>::into)
            .and_then(|x| x.into());
            
            renderer.window().set_cursor_visible(cursor_visible);

            if let Err(SwapBuffersError::ContextLost(err)) = result {
                error!(log, "Critical Rendering Error: {}", err);
                state.running.store(false, Ordering::SeqCst);
            }
        }

        // Send frame events so that client start drawing their next frame
        state
            .window_map
            .borrow()
            .send_frames(start_time.elapsed().as_millis() as u32);
        display.borrow_mut().flush_clients(&mut state);

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
