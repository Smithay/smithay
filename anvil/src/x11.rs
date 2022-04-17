use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

use crate::{drawing::*, state::Backend, AnvilState};
#[cfg(feature = "debug")]
use image::GenericImageView;
use slog::Logger;
#[cfg(feature = "debug")]
use smithay::backend::renderer::{gles2::Gles2Texture, ImportMem};
#[cfg(feature = "egl")]
use smithay::{backend::renderer::ImportDma, wayland::dmabuf::init_dmabuf_global};
use smithay::{
    backend::{
        egl::{
            context::GlAttributes, surface::adjust_damage as egl_adjust_damage, EGLContext, EGLDisplay,
            EGLSurface,
        },
        renderer::{gles2::Gles2Renderer, Bind, ImportEgl},
        x11::{WindowBuilder, X11Backend, X11Connection, X11Event},
    },
    reexports::{
        calloop::EventLoop,
        wayland_server::{
            protocol::{wl_output, wl_surface},
            Display,
        },
    },
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

pub const OUTPUT_NAME: &str = "x11";

#[derive(Debug)]
pub struct X11Data {
    mode: Mode,
    surface: Rc<EGLSurface>,
    #[cfg(feature = "debug")]
    fps_texture: Gles2Texture,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
    full_redraw: u8,
}

impl Backend for X11Data {
    fn seat_name(&self) -> String {
        "x11".to_owned()
    }
    fn reset_buffers(&mut self, _output: &Output) {
        self.full_redraw = 4;
    }
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
}

pub fn run_x11(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let connection = X11Connection::new(log.clone()).expect("Failed to initialize X11 connection");

    // Initialize EGL using the X11 connection.
    let egl = EGLDisplay::new(&connection, log.clone()).expect("Failed to create EGLDisplay");
    // Create the OpenGL context
    let context = EGLContext::new_with_config(
        &egl,
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: true,
        },
        Default::default(),
        log.clone(),
    )
    .expect("Failed to create EGLContext");

    let backend = X11Backend::new(connection).expect("Failed to create X11 backend");
    let handle = backend.handle();

    let window = WindowBuilder::new()
        .title("Anvil")
        .visual_from_context(&context)
        .expect("Unable to derive visual id from egl context")
        .build(&handle)
        .expect("Failed to create first window");

    // Create the surface for the window.
    let surface = Rc::new(
        EGLSurface::new(
            &egl,
            context.pixel_format().unwrap(),
            context.config_id(),
            window.clone(),
            log.clone(),
        )
        .expect("Failed to create egl surface"),
    );

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
                move |buffer, _| renderer.borrow_mut().import_dmabuf(buffer, None).is_ok(),
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

    #[cfg(feature = "debug")]
    let fps_image =
        image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
            .decode()
            .unwrap();
    let data = X11Data {
        mode,
        surface,
        #[cfg(feature = "debug")]
        fps_texture: {
            renderer
                .borrow_mut()
                .import_memory(
                    &fps_image.to_rgba8(),
                    (fps_image.width() as i32, fps_image.height() as i32).into(),
                    false,
                )
                .expect("Unable to upload FPS texture")
        },
        #[cfg(feature = "debug")]
        fps: fps_ticker::Fps::default(),
        full_redraw: 2,
    };

    let mut state = AnvilState::init(display.clone(), event_loop.handle(), data, log.clone(), true);
    let (output, _global) = Output::new(
        &mut *display.borrow_mut(),
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "X11".into(),
        },
        log.clone(),
    );
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);
    state.space.borrow_mut().map_output(&output, 1.0, (0, 0));

    let output_clone = output.clone();
    event_loop
        .handle()
        .insert_source(backend, move |event, _, state| match event {
            X11Event::CloseRequested { .. } => {
                state.running.store(false, Ordering::SeqCst);
            }
            X11Event::Resized { new_size, .. } => {
                let output = &output_clone;
                let size = { (new_size.w as i32, new_size.h as i32).into() };

                state.backend_data.mode = Mode {
                    size,
                    refresh: 60_000,
                };
                output.delete_mode(output.current_mode().unwrap());
                output.change_current_state(Some(state.backend_data.mode), None, None, None);
                output.set_preferred(state.backend_data.mode);
                crate::shell::fixup_positions(&mut *state.space.borrow_mut());
            }
            X11Event::Input(event) => state.process_input_event_windowed(event, OUTPUT_NAME),
        })
        .expect("Failed to insert X11 Backend into event loop");

    let start_time = std::time::Instant::now();
    let mut cursor_visible;

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        let mut space = state.space.borrow_mut();

        {
            let backend_data = &mut state.backend_data;
            let mut renderer = renderer.borrow_mut();

            // We need to borrow everything we want to refer to inside the renderer callback otherwise rustc is unhappy.
            let (x, y) = state.pointer_location.into();
            let dnd_icon = &state.dnd_icon;
            let cursor_status = &state.cursor_status;
            #[cfg(feature = "debug")]
            let fps = backend_data.fps.avg().round() as u32;
            #[cfg(feature = "debug")]
            let fps_texture = &backend_data.fps_texture;

            let full_redraw = &mut backend_data.full_redraw;
            *full_redraw = full_redraw.saturating_sub(1);
            let age = if *full_redraw > 0 {
                0
            } else {
                backend_data.surface.buffer_age().unwrap_or(0)
            };
            let mut elements = Vec::<CustomElem<Gles2Renderer>>::new();
            let dnd_guard = dnd_icon.lock().unwrap();
            let mut cursor_guard = cursor_status.lock().unwrap();

            // draw the dnd icon if any
            if let Some(ref surface) = *dnd_guard {
                if surface.as_ref().is_alive() {
                    elements.push(draw_dnd_icon(surface.clone(), (x as i32, y as i32), &log).into());
                }
            }

            // draw the cursor as relevant
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Image(ref surface) = *cursor_guard {
                reset = !surface.as_ref().is_alive();
            }
            if reset {
                *cursor_guard = CursorImageStatus::Default;
            }
            if let CursorImageStatus::Image(ref surface) = *cursor_guard {
                cursor_visible = false;
                elements.push(draw_cursor(surface.clone(), (x as i32, y as i32), &log).into());
            } else {
                cursor_visible = true;
            }

            // draw FPS
            #[cfg(feature = "debug")]
            {
                elements.push(draw_fps::<Gles2Renderer>(fps_texture, fps).into());
            }

            renderer
                .bind(backend_data.surface.clone())
                .expect("Failed to bind EGLSurface");
            let render_res = crate::render::render_output(
                &output,
                &mut *space,
                &mut *renderer,
                age as usize,
                &*elements,
                &log,
            );
            match render_res {
                Ok(damage) => {
                    trace!(log, "Finished rendering");
                    let scale = space.output_scale(&output).unwrap_or(1.0);
                    let size = output.current_mode().unwrap().size;
                    let mut damage = damage.filter(|_| age != 0).map(|damage| {
                        egl_adjust_damage(
                            damage
                                .into_iter()
                                .map(|rect| rect.to_f64().to_physical(scale).to_i32_round()),
                            size,
                        )
                    });
                    // try again
                    let _ = backend_data.surface.swap_buffers(damage.as_deref_mut());
                }
                Err(err) => {
                    backend_data.full_redraw = 4;
                    error!(log, "Rendering error: {}", err);
                    // TODO: convert RenderError into SwapBuffersError and skip temporary (will retry) and panic on ContextLost or recreate
                }
            }

            #[cfg(feature = "debug")]
            state.backend_data.fps.tick();
            window.set_cursor_visible(cursor_visible);
        }

        // Send frame events so that client start drawing their next frame
        space.send_frames(start_time.elapsed().as_millis() as u32);
        std::mem::drop(space);

        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.borrow_mut().refresh();
            state.popups.borrow_mut().cleanup();
            display.borrow_mut().flush_clients(&mut state);
        }
    }
}
