use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

#[cfg(feature = "debug")]
use smithay::backend::renderer::gles2::Gles2Texture;
#[cfg(feature = "egl")]
use smithay::{
    backend::renderer::{ImportDma, ImportEgl},
    wayland::dmabuf::init_dmabuf_global,
};
use smithay::{
    backend::{
        winit::{self, WinitEvent},
        SwapBuffersError,
    },
    reexports::{
        calloop::EventLoop,
        wayland_server::{protocol::wl_output, Display},
    },
    wayland::{
        output::{Mode, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use slog::Logger;

use crate::state::{AnvilState, Backend};
use crate::{drawing::*, render::render_layers_and_windows};

pub const OUTPUT_NAME: &str = "winit";

pub struct WinitData {
    #[cfg(feature = "debug")]
    fps_texture: Gles2Texture,
    #[cfg(feature = "debug")]
    pub fps: fps_ticker::Fps,
}

impl Backend for WinitData {
    fn seat_name(&self) -> String {
        String::from("winit")
    }
}

pub fn run_winit(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let (renderer, mut winit) = match winit::init(log.clone()) {
        Ok(ret) => ret,
        Err(err) => {
            slog::crit!(log, "Failed to initialize Winit backend: {}", err);
            return;
        }
    };
    let renderer = Rc::new(RefCell::new(renderer));

    #[cfg(feature = "egl")]
    if renderer
        .borrow_mut()
        .renderer()
        .bind_wl_display(&display.borrow())
        .is_ok()
    {
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

    let size = renderer.borrow().window_size().physical_size;

    /*
     * Initialize the globals
     */

    let data = WinitData {
        #[cfg(feature = "debug")]
        fps_texture: import_bitmap(
            &mut renderer.borrow_mut().renderer(),
            &image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
                .decode()
                .unwrap()
                .to_rgba8(),
        )
        .expect("Unable to upload FPS texture"),
        #[cfg(feature = "debug")]
        fps: fps_ticker::Fps::default(),
    };
    let mut state = AnvilState::init(display.clone(), event_loop.handle(), data, log.clone(), true);

    let mode = Mode {
        size,
        refresh: 60_000,
    };

    state.output_map.borrow_mut().add(
        OUTPUT_NAME,
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        mode,
    );

    let start_time = std::time::Instant::now();
    let mut cursor_visible = true;

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        if winit
            .dispatch_new_events(|event| match event {
                WinitEvent::Resized { size, .. } => {
                    state.output_map.borrow_mut().update_mode_by_name(
                        Mode {
                            size,
                            refresh: 60_000,
                        },
                        crate::winit::OUTPUT_NAME,
                    );

                    let output_mut = state.output_map.borrow();
                    let output = output_mut.find_by_name(crate::winit::OUTPUT_NAME).unwrap();

                    state.window_map.borrow_mut().layers.arange_layers(output);
                }

                WinitEvent::Input(event) => state.process_input_event(event),

                _ => (),
            })
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
            break;
        }

        // drawing logic
        {
            let mut renderer = renderer.borrow_mut();
            // This is safe to do as with winit we are guaranteed to have exactly one output
            let (output_geometry, output_scale) = state
                .output_map
                .borrow()
                .find_by_name(OUTPUT_NAME)
                .map(|output| (output.geometry(), output.scale()))
                .unwrap();

            let result = renderer
                .render(|renderer, frame| {
                    render_layers_and_windows(
                        renderer,
                        frame,
                        &*state.window_map.borrow(),
                        output_geometry,
                        output_scale,
                        &log,
                    )?;

                    let (x, y) = state.pointer_location.into();

                    // draw the dnd icon if any
                    {
                        let guard = state.dnd_icon.lock().unwrap();
                        if let Some(ref surface) = *guard {
                            if surface.as_ref().is_alive() {
                                draw_dnd_icon(
                                    renderer,
                                    frame,
                                    surface,
                                    (x as i32, y as i32).into(),
                                    output_scale,
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
                                (x as i32, y as i32).into(),
                                output_scale,
                                &log,
                            )?;
                        } else {
                            cursor_visible = true;
                        }
                    }

                    #[cfg(feature = "debug")]
                    {
                        let fps = state.backend_data.fps.avg().round() as u32;

                        draw_fps(
                            renderer,
                            frame,
                            &state.backend_data.fps_texture,
                            output_scale as f64,
                            fps,
                        )?;
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
            state.output_map.borrow_mut().refresh();
        }

        #[cfg(feature = "debug")]
        state.backend_data.fps.tick();
    }

    // Cleanup stuff
    state.window_map.borrow_mut().clear();
}
