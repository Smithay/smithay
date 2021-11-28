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
        renderer::{gles2::Gles2Renderer, Frame, Renderer},
        winit::{self, WinitEvent},
        SwapBuffersError,
    },
    desktop::{
        draw_window,
        space::{RenderElement, RenderError},
    },
    reexports::{
        calloop::EventLoop,
        wayland_server::{protocol::wl_output, Display},
    },
    utils::Rectangle,
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use slog::Logger;

use crate::{
    drawing::*,
    shell::FullscreenSurface,
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
            &mut *display.borrow_mut(),
            dmabuf_formats,
            move |buffer, _| backend.borrow_mut().renderer().import_dmabuf(buffer).is_ok(),
            log.clone(),
        );
    };

    let size = backend.borrow().window_size().physical_size;

    /*
     * Initialize the globals
     */

    let data = WinitData {
        #[cfg(feature = "debug")]
        fps_texture: import_bitmap(
            backend.borrow_mut().renderer(),
            &image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
                .decode()
                .unwrap()
                .to_rgba8(),
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

            let mut elements = Vec::new();
            let dnd_guard = state.dnd_icon.lock().unwrap();
            let mut cursor_guard = state.cursor_status.lock().unwrap();

            // draw the dnd icon if any
            if let Some(ref surface) = *dnd_guard {
                if surface.as_ref().is_alive() {
                    elements.push(Box::new(draw_dnd_icon(
                        surface.clone(),
                        state.pointer_location.to_i32_round(),
                        &log,
                    )) as Box<dyn RenderElement<_, _, _, _>>);
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
                elements.push(Box::new(draw_cursor(
                    surface.clone(),
                    state.pointer_location.to_i32_round(),
                    &log,
                )));
            } else {
                cursor_visible = true;
            }

            // draw FPS
            #[cfg(feature = "debug")]
            {
                let fps = state.backend_data.fps.avg().round() as u32;
                let fps_texture = &state.backend_data.fps_texture;
                elements.push(Box::new(draw_fps(fps_texture, fps)));
            }

            let full_redraw = &mut state.backend_data.full_redraw;
            *full_redraw = full_redraw.saturating_sub(1);
            let age = if *full_redraw > 0 { 0 } else { backend.buffer_age() };
            let render_res = backend.bind().and_then(|_| {
                let renderer = backend.renderer();
                if let Some(window) = output
                    .user_data()
                    .get::<FullscreenSurface>()
                    .and_then(|f| f.get())
                {
                    let transform = output.current_transform().into();
                    let mode = output.current_mode().unwrap();
                    let scale = state.space.borrow().output_scale(&output).unwrap();
                    let res = renderer
                        .render(mode.size, transform, |renderer, frame| {
                            let mut damage = Vec::from(window.accumulated_damage(None));
                            frame.clear(CLEAR_COLOR, &[Rectangle::from_loc_and_size((0, 0), mode.size)])?;
                            draw_window(
                                renderer,
                                frame,
                                &window,
                                scale,
                                (0, 0),
                                &[Rectangle::from_loc_and_size(
                                    (0, 0),
                                    mode.size.to_f64().to_logical(scale).to_i32_round(),
                                )],
                                &log,
                            )?;
                            for elem in elements {
                                let geo = elem.geometry();
                                let elem_damage = elem.accumulated_damage(None);
                                elem.draw(
                                    renderer,
                                    frame,
                                    scale,
                                    &[Rectangle::from_loc_and_size((0, 0), geo.size)],
                                    &log,
                                )?;
                                damage.extend(elem_damage.into_iter().map(|mut rect| {
                                    rect.loc += geo.loc;
                                    rect
                                }))
                            }
                            Ok(Some(damage))
                        })
                        .and_then(std::convert::identity)
                        .map_err(RenderError::<Gles2Renderer>::Rendering);
                    window.send_frame(start_time.elapsed().as_millis() as u32);
                    res
                } else {
                    state
                        .space
                        .borrow_mut()
                        .render_output(renderer, &output, age, CLEAR_COLOR, &*elements)
                }
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
                Ok(None) => {}
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
        display.borrow_mut().flush_clients(&mut state);

        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.borrow_mut().refresh();
            display.borrow_mut().flush_clients(&mut state);
        }

        #[cfg(feature = "debug")]
        state.backend_data.fps.tick();
    }
}
