use std::{sync::atomic::Ordering, time::Duration};

#[cfg(feature = "debug")]
use image::GenericImageView;
use slog::Logger;
#[cfg(feature = "debug")]
use smithay::backend::renderer::{gles2::Gles2Texture, ImportMem};
#[cfg(feature = "egl")]
use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{ImportDma, ImportEgl},
    },
    delegate_dmabuf,
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportError},
};
use smithay::{
    backend::{
        renderer::gles2::Gles2Renderer,
        winit::{self, WinitEvent, WinitGraphicsBackend},
        SwapBuffersError,
    },
    desktop::space::RenderError,
    reexports::{
        calloop::EventLoop,
        wayland_server::{
            protocol::{wl_output, wl_surface},
            Display, DisplayHandle,
        },
    },
    utils::IsAlive,
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use crate::{
    drawing::*,
    state::{AnvilState, Backend, CalloopData},
};

pub const OUTPUT_NAME: &str = "winit";

pub struct WinitData {
    backend: WinitGraphicsBackend,
    #[cfg(feature = "egl")]
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    full_redraw: u8,
    #[cfg(feature = "debug")]
    fps_texture: Gles2Texture,
    #[cfg(feature = "debug")]
    pub fps: fps_ticker::Fps,
}

#[cfg(feature = "egl")]
impl DmabufHandler for AnvilState<WinitData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(
        &mut self,
        _dh: &DisplayHandle,
        global: &DmabufGlobal,
        dmabuf: Dmabuf,
    ) -> Result<(), ImportError> {
        self.backend_data
            .backend
            .renderer()
            .import_dmabuf(&dmabuf, None)
            .map(|_| ())
            .map_err(|_| ImportError::Failed)
    }
}
#[cfg(feature = "egl")]
delegate_dmabuf!(AnvilState<WinitData>);

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
    let mut display = Display::new().unwrap();

    let (mut backend, mut winit) = match winit::init(log.clone()) {
        Ok(ret) => ret,
        Err(err) => {
            slog::crit!(log, "Failed to initialize Winit backend: {}", err);
            return;
        }
    };
    let size = backend.window_size().physical_size;

    let data = {
        #[cfg(feature = "egl")]
        let dmabuf_state = if backend.renderer().bind_wl_display(&display.handle()).is_ok() {
            info!(log, "EGL hardware-acceleration enabled");
            let dmabuf_formats = backend.renderer().dmabuf_formats().cloned().collect::<Vec<_>>();
            let mut state = DmabufState::new();
            let global = state.create_global::<AnvilState<WinitData>, _>(
                &display.handle(),
                dmabuf_formats,
                log.clone(),
            );
            Some((state, global))
        } else {
            None
        };
        #[cfg(feature = "debug")]
        let fps_image =
            image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
                .decode()
                .unwrap();

        WinitData {
            backend,
            #[cfg(feature = "egl")]
            dmabuf_state,
            full_redraw: 0,
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
        }
    };
    let mut state = AnvilState::init(&mut display, event_loop.handle(), data, log.clone(), true);

    let mode = Mode {
        size,
        refresh: 60_000,
    };
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );
    let _global = output.create_global::<AnvilState<WinitData>>(&display.handle());
    output.change_current_state(
        Some(mode),
        Some(wl_output::Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let start_time = std::time::Instant::now();

    #[cfg(feature = "xwayland")]
    state.start_xwayland(&mut display);

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        if winit
            .dispatch_new_events(|event| match event {
                WinitEvent::Resized { size, .. } => {
                    let mut dh = display.handle();
                    // We only have one output
                    let output = state.space.outputs().next().unwrap().clone();
                    state.space.map_output(&output, (0, 0));
                    let mode = Mode {
                        size,
                        refresh: 60_000,
                    };
                    output.change_current_state(Some(mode), None, None, None);
                    output.set_preferred(mode);
                    crate::shell::fixup_positions(&mut dh, &mut state.space);
                }
                WinitEvent::Input(event) => {
                    state.process_input_event_windowed(&mut display.handle(), event, OUTPUT_NAME)
                }
                _ => (),
            })
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
            break;
        }

        // drawing logic
        {
            let mut backend = &mut state.backend_data.backend;
            let cursor_visible: bool;

            let mut elements = Vec::<CustomElem<Gles2Renderer>>::new();
            let mut cursor_guard = state.cursor_status.lock().unwrap();

            // draw the dnd icon if any
            if let Some(surface) = state.dnd_icon.as_ref() {
                if surface.alive() {
                    elements.push(
                        draw_dnd_icon(surface.clone(), state.pointer_location.to_i32_round(), &log).into(),
                    );
                }
            }

            // draw the cursor as relevant
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Image(ref surface) = *cursor_guard {
                reset = !surface.alive();
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
            let space = &mut state.space;
            let render_res = backend.bind().and_then(|_| {
                let renderer = backend.renderer();
                crate::render::render_output(
                    &display.handle(),
                    &output,
                    space,
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
                    let scale = output.current_scale().fractional_scale();
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
        state.space.send_frames(start_time.elapsed().as_millis() as u32);

        let mut calloop_data = CalloopData { state, display };
        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut calloop_data);
        CalloopData { state, display } = calloop_data;

        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.refresh(&display.handle());
            state.popups.cleanup();
            display.flush_clients().unwrap();
        }

        #[cfg(feature = "debug")]
        state.backend_data.fps.tick();
    }
}
