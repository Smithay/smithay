use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

#[cfg(feature = "egl")]
use smithay::{
    backend::{
        egl::display::EGLBufferReader,
        renderer::{ImportDma, ImportEgl},
    },
    wayland::dmabuf::init_dmabuf_global,
};
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
use crate::state::{AnvilState, Backend};

pub struct WinitData(Rc<RefCell<winit::WinitGraphicsBackend>>);

impl Backend for WinitData {
    #[cfg(feature = "egl")]
    fn egl_reader(&self) -> Option<EGLBufferReader> {
        self.0.borrow_mut().renderer().egl_reader().cloned()
    }

    fn seat_name(&self) -> String {
        String::from("winit")
    }
}

pub fn run_winit(
    display: Rc<RefCell<Display>>,
    event_loop: &mut EventLoop<'static, AnvilState<WinitData>>,
    log: Logger,
) -> Result<(), ()> {
    let (renderer, mut input) = winit::init(log.clone()).map_err(|err| {
        slog::crit!(log, "Failed to initialize Winit backend: {}", err);
    })?;
    let renderer = Rc::new(RefCell::new(renderer));

    #[cfg(feature = "egl")]
    if renderer.borrow().bind_wl_display(&display.borrow()).is_ok() {
        info!(log, "EGL hardware-acceleration enabled");
        let dmabuf_formats = renderer
            .borrow_mut()
            .renderer()
            .dmabuf_formats()
            .cloned()
            .collect::<Vec<_>>();
        let renderer = renderer.clone();
        init_dmabuf_global(
            &mut *display.borrow_mut(),
            dmabuf_formats,
            move |buffer, _| renderer.borrow_mut().renderer().import_dmabuf(buffer).is_ok(),
            log.clone(),
        );
    };

    let (w, h): (u32, u32) = renderer.borrow().window_size().physical_size.into();

    /*
     * Initialize the globals
     */

    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        WinitData(renderer.clone()),
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

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

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

            let result = renderer
                .render(|renderer, frame| {
                    frame.clear([0.8, 0.8, 0.9, 1.0])?;

                    // draw the windows
                    draw_windows(
                        renderer,
                        frame,
                        &*state.window_map.borrow(),
                        None,
                        state.ctoken,
                        &log,
                    )?;

                    let (x, y) = state.pointer_location;
                    // draw the dnd icon if any
                    {
                        let guard = state.dnd_icon.lock().unwrap();
                        if let Some(ref surface) = *guard {
                            if surface.as_ref().is_alive() {
                                draw_dnd_icon(
                                    renderer,
                                    frame,
                                    surface,
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
                            draw_cursor(renderer, frame, surface, (x as i32, y as i32), state.ctoken, &log)?;
                        } else {
                            cursor_visible = true;
                        }
                    }

                    Ok(())
                })
                .map_err(Into::<SwapBuffersError>::into)
                .and_then(|x| x);

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
