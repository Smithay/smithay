use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

#[cfg(feature = "debug")]
use image::GenericImageView;
use slog::Logger;
#[cfg(feature = "debug")]
use smithay::backend::renderer::{gles2::Gles2Texture, ImportMem};
#[cfg(feature = "egl")]
use smithay::{
    backend::renderer::{ImportDma, ImportEgl},
    wayland::dmabuf::init_dmabuf_global,
};
use smithay::{
    backend::{
        renderer::gles2::Gles2Renderer,
        winit::{self, WinitEvent},
        SwapBuffersError,
    },
    desktop::space::RenderError,
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

use crate::{
    drawing::*,
    state::{AnvilState, Backend},
};

pub const OUTPUT_NAME: &str = "winit";

pub struct WinitData {
    #[cfg(feature = "debug")]
    fps_texture: Gles2Texture,
    #[cfg(feature = "debug")]
    pub fps: fps_ticker::Fps,
    full_redraw: u8,
}

impl Backend for WinitData {
    fn seat_name(&self) -> String {
        String::from("winit")
    }
    fn reset_buffers(&mut self, _output: &Output) {
        self.full_redraw = 4;
    }
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
}

pub fn run_winit(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let (backend, mut winit) = match winit::init(log.clone()) {
        Ok(ret) => ret,
        Err(err) => {
            slog::crit!(log, "Failed to initialize Winit backend: {}", err);
            return;
        }
    };
    let backend = Rc::new(RefCell::new(backend));

    #[cfg(feature = "egl")]
    if backend
        .borrow_mut()
        .renderer()
        .bind_wl_display(&display.borrow())
        .is_ok()
    {
        info!(log, "EGL hardware-acceleration enabled");
        let dmabuf_formats = backend
            .borrow_mut()
            .renderer()
            .dmabuf_formats()
            .cloned()
            .collect::<Vec<_>>();
        let backend = backend.clone();
        init_dmabuf_global(
            &mut display.borrow_mut(),
            dmabuf_formats,
            move |buffer, _| {
                backend
                    .borrow_mut()
                    .renderer()
                    .import_dmabuf(buffer, None)
                    .is_ok()
            },
            log.clone(),
        );
    };

    let size = backend.borrow().window_size().physical_size;

    /*
     * Initialize the globals
     */

    #[cfg(feature = "debug")]
    let fps_image =
        image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
            .decode()
            .unwrap();
    let data = WinitData {
        #[cfg(feature = "debug")]
        fps_texture: backend
            .borrow_mut()
            .renderer()
            .import_memory(
                &fps_image.to_rgba8(),
                (fps_image.width() as i32, fps_image.height() as i32).into(),
                false,
            )
            .expect("Unable to upload FPS texture"),
        #[cfg(feature = "debug")]
        fps: fps_ticker::Fps::default(),
        full_redraw: 0,
    };
    let mut state = AnvilState::init(display.clone(), event_loop.handle(), data, log.clone(), true);

    let mode = Mode {
        size,
        refresh: 60_000,
    };

    let (output, _global) = Output::new(
        &mut *display.borrow_mut(),
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );
    output.change_current_state(
        Some(mode),
        Some(wl_output::Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.borrow_mut().map_output(&output, 1.0, (0, 0));

    let start_time = std::time::Instant::now();

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        if winit
            .dispatch_new_events(|event| match event {
                WinitEvent::Resized { size, .. } => {
                    let mut space = state.space.borrow_mut();
                    // We only have one output
                    let output = space.outputs().next().unwrap().clone();
                    let current_scale = space.output_scale(&output).unwrap();
                    space.map_output(&output, current_scale, (0, 0));
                    let mode = Mode {
                        size,
                        refresh: 60_000,
                    };
                    output.change_current_state(Some(mode), None, None, None);
                    output.set_preferred(mode);
                    crate::shell::fixup_positions(&mut *space);
                }

                WinitEvent::Input(event) => state.process_input_event_windowed(event, OUTPUT_NAME),

                _ => (),
            })
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
            break;
        }

        // drawing logic
        {
            let mut backend = backend.borrow_mut();
            let cursor_visible: bool;

            let mut elements = Vec::<CustomElem<Gles2Renderer>>::new();
            let dnd_guard = state.dnd_icon.lock().unwrap();
            let mut cursor_guard = state.cursor_status.lock().unwrap();

            // draw the dnd icon if any
            if let Some(ref surface) = *dnd_guard {
                if surface.as_ref().is_alive() {
                    elements.push(
                        draw_dnd_icon(surface.clone(), state.pointer_location.to_i32_round(), &log).into(),
                    );
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
                elements
                    .push(draw_cursor(surface.clone(), state.pointer_location.to_i32_round(), &log).into());
            } else {
                cursor_visible = true;
            }

            // draw FPS
            #[cfg(feature = "debug")]
            {
                let fps = state.backend_data.fps.avg().round() as u32;
                let fps_texture = &state.backend_data.fps_texture;
                elements.push(draw_fps::<Gles2Renderer>(fps_texture, fps).into());
            }

            let full_redraw = &mut state.backend_data.full_redraw;
            *full_redraw = full_redraw.saturating_sub(1);
            let age = if *full_redraw > 0 {
                0
            } else {
                backend.buffer_age().unwrap_or(0)
            };
            let render_res = backend.bind().and_then(|_| {
                let renderer = backend.renderer();
                crate::render::render_output(
                    &output,
                    &mut *state.space.borrow_mut(),
                    renderer,
                    age,
                    &*elements,
                    &log,
                )
                .map_err(|err| match err {
                    RenderError::Rendering(err) => err.into(),
                    _ => unreachable!(),
                })
            });

            match render_res {
                Ok(Some(damage)) => {
                    let scale = state.space.borrow().output_scale(&output).unwrap_or(1.0);
                    if let Err(err) = backend.submit(if age == 0 { None } else { Some(&*damage) }, scale) {
                        warn!(log, "Failed to submit buffer: {}", err);
                    }
                    backend.window().set_cursor_visible(cursor_visible);
                }
                Ok(None) => backend.window().set_cursor_visible(cursor_visible),
                Err(SwapBuffersError::ContextLost(err)) => {
                    error!(log, "Critical Rendering Error: {}", err);
                    state.running.store(false, Ordering::SeqCst);
                }
                Err(err) => warn!(log, "Rendering error: {}", err),
            }
        }

        // Send frame events so that client start drawing their next frame
        state
            .space
            .borrow()
            .send_frames(false, start_time.elapsed().as_millis() as u32);

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

        #[cfg(feature = "debug")]
        state.backend_data.fps.tick();
    }
}
