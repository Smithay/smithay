use std::{
    cell::RefCell,
    rc::Rc,
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use slog::Logger;
#[cfg(feature = "egl")]
use smithay::{backend::renderer::ImportDma, wayland::dmabuf::init_dmabuf_global};
use smithay::{
    backend::{
        egl::{EGLContext, EGLDisplay},
        renderer::{gles2::Gles2Renderer, Bind, ImportEgl, Renderer, Transform, Unbind},
        x11::{WindowBuilder, X11Backend, X11Event, X11Surface},
        SwapBuffersError,
    },
    reexports::{
        calloop::EventLoop,
        gbm,
        wayland_server::{protocol::wl_output, Display},
    },
    wayland::{
        output::{Mode, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use crate::{
    drawing::{draw_cursor, draw_dnd_icon},
    render::render_layers_and_windows,
    state::Backend,
    AnvilState,
};

#[cfg(feature = "debug")]
use smithay::backend::renderer::gles2::Gles2Texture;

pub const OUTPUT_NAME: &str = "x11";

#[derive(Debug)]
pub struct X11Data {
    render: bool,
    mode: Mode,
    surface: X11Surface,
    #[cfg(feature = "debug")]
    fps_texture: Gles2Texture,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
}

impl Backend for X11Data {
    fn seat_name(&self) -> String {
        "x11".to_owned()
    }
}

pub fn run_x11(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let backend = X11Backend::new(log.clone()).expect("Failed to initilize X11 backend");
    let handle = backend.handle();

    // Obtain the DRM node the X server uses for direct rendering.
    let drm_node = handle
        .drm_node()
        .expect("Could not get DRM node used by X server");

    // Create the gbm device for buffer allocation.
    let device = gbm::Device::new(drm_node).expect("Failed to create gbm device");
    // Initialize EGL using the GBM device.
    let egl = EGLDisplay::new(&device, log.clone()).expect("Failed to create EGLDisplay");
    // Create the OpenGL context
    let context = EGLContext::new(&egl, log.clone()).expect("Failed to create EGLContext");

    let window = WindowBuilder::new()
        .title("Anvil")
        .build(&handle)
        .expect("Failed to create first window");

    let device = Arc::new(Mutex::new(device));

    // Create the surface for the window.
    let surface = handle
        .create_surface(
            &window,
            device,
            context
                .dmabuf_render_formats()
                .iter()
                .map(|format| format.modifier),
        )
        .expect("Failed to create X11 surface");

    let renderer =
        unsafe { Gles2Renderer::new(context, log.clone()) }.expect("Failed to initialize renderer");
    let renderer = Rc::new(RefCell::new(renderer));

    #[cfg(feature = "egl")]
    {
        if renderer.borrow_mut().bind_wl_display(&*display.borrow()).is_ok() {
            info!(log, "EGL hardware-acceleration enabled");
            let dmabuf_formats = renderer
                .borrow_mut()
                .dmabuf_formats()
                .cloned()
                .collect::<Vec<_>>();
            let renderer = renderer.clone();
            init_dmabuf_global(
                &mut *display.borrow_mut(),
                dmabuf_formats,
                move |buffer, _| renderer.borrow_mut().import_dmabuf(buffer).is_ok(),
                log.clone(),
            );
        }
    }

    let size = {
        let s = window.size();

        (s.w as i32, s.h as i32).into()
    };

    let mode = Mode {
        size,
        refresh: 60_000,
    };

    let data = X11Data {
        render: true,
        mode,
        surface,
        #[cfg(feature = "debug")]
        fps_texture: {
            use crate::drawing::{import_bitmap, FPS_NUMBERS_PNG};

            import_bitmap(
                &mut *renderer.borrow_mut(),
                &image::io::Reader::with_format(
                    std::io::Cursor::new(FPS_NUMBERS_PNG),
                    image::ImageFormat::Png,
                )
                .decode()
                .unwrap()
                .to_rgba8(),
            )
            .expect("Unable to upload FPS texture")
        },
        #[cfg(feature = "debug")]
        fps: fps_ticker::Fps::default(),
    };

    let mut state = AnvilState::init(display.clone(), event_loop.handle(), data, log.clone(), true);

    state.output_map.borrow_mut().add(
        OUTPUT_NAME,
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "X11".into(),
        },
        mode,
    );

    event_loop
        .handle()
        .insert_source(backend, |event, _, state| match event {
            X11Event::CloseRequested { .. } => {
                state.running.store(false, Ordering::SeqCst);
            }

            X11Event::Resized { new_size, .. } => {
                let size = { (new_size.w as i32, new_size.h as i32).into() };

                state.backend_data.mode = Mode {
                    size,
                    refresh: 60_000,
                };
                state.output_map.borrow_mut().update_mode_by_name(
                    Mode {
                        size,
                        refresh: 60_000,
                    },
                    OUTPUT_NAME,
                );

                let output_mut = state.output_map.borrow();
                let output = output_mut.find_by_name(OUTPUT_NAME).unwrap();

                state.window_map.borrow_mut().layers.arange_layers(output);
                state.backend_data.render = true;
            }

            X11Event::PresentCompleted { .. } | X11Event::Refresh { .. } => {
                state.backend_data.render = true;
            }

            X11Event::Input(event) => state.process_input_event_windowed(event, OUTPUT_NAME),
        })
        .expect("Failed to insert X11 Backend into event loop");

    let start_time = std::time::Instant::now();
    let mut cursor_visible = true;

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        let (output_geometry, output_scale) = state
            .output_map
            .borrow()
            .find_by_name(OUTPUT_NAME)
            .map(|output| (output.geometry(), output.scale()))
            .unwrap();

        if state.backend_data.render {
            state.backend_data.render = false;
            let backend_data = &mut state.backend_data;
            let mut renderer = renderer.borrow_mut();

            // We need to borrow everything we want to refer to inside the renderer callback otherwise rustc is unhappy.
            let window_map = state.window_map.borrow();
            let (x, y) = state.pointer_location.into();
            let dnd_icon = &state.dnd_icon;
            let cursor_status = &state.cursor_status;
            #[cfg(feature = "debug")]
            let fps = backend_data.fps.avg().round() as u32;
            #[cfg(feature = "debug")]
            let fps_texture = &backend_data.fps_texture;

            let (buffer, _age) = backend_data.surface.buffer().expect("gbm device was destroyed");
            if let Err(err) = renderer.bind(buffer) {
                error!(log, "Error while binding buffer: {}", err);
            }

            // drawing logic
            match renderer
                // TODO: Address this issue in renderer.
                .render(backend_data.mode.size, Transform::Normal, |renderer, frame| {
                    render_layers_and_windows(
                        renderer,
                        frame,
                        &*window_map,
                        output_geometry,
                        output_scale,
                        &log,
                    )?;

                    // draw the dnd icon if any
                    {
                        let guard = dnd_icon.lock().unwrap();
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
                        use crate::drawing::draw_fps;

                        draw_fps(renderer, frame, fps_texture, output_scale as f64, fps)?;
                    }

                    Ok(())
                })
                .map_err(Into::<SwapBuffersError>::into)
                .and_then(|x| x)
                .map_err(Into::<SwapBuffersError>::into)
            {
                Ok(()) => {
                    // Unbind the buffer
                    if let Err(err) = renderer.unbind() {
                        error!(log, "Error while unbinding buffer: {}", err);
                    }

                    // Submit the buffer
                    if let Err(err) = backend_data.surface.submit() {
                        error!(log, "Error submitting buffer for display: {}", err);
                    }
                }

                Err(err) => {
                    if let SwapBuffersError::ContextLost(err) = err {
                        error!(log, "Critical Rendering Error: {}", err);
                        state.running.store(false, Ordering::SeqCst);
                    }
                }
            }

            #[cfg(feature = "debug")]
            state.backend_data.fps.tick();
            window.set_cursor_visible(cursor_visible);

            // Send frame events so that client start drawing their next frame
            window_map.send_frames(start_time.elapsed().as_millis() as u32);
            std::mem::drop(window_map);

            display.borrow_mut().flush_clients(&mut state);
        }

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
    }

    // Cleanup stuff
    state.window_map.borrow_mut().clear();
}
