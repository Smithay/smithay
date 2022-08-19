use std::{sync::atomic::Ordering, time::Duration};

use slog::Logger;
#[cfg(feature = "debug")]
use smithay::backend::renderer::ImportMem;
#[cfg(not(feature = "debug"))]
use smithay::desktop::space::SpaceRenderElements;
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
        renderer::{
            output::{
                element::{surface::WaylandSurfaceRenderElement, texture::TextureRenderElement},
                DamageTrackedRenderer,
            },
            Renderer,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
        SwapBuffersError,
    },
    desktop::space::SurfaceTree,
    input::pointer::CursorImageStatus,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::{
            protocol::{wl_output, wl_surface},
            Display,
        },
    },
    utils::IsAlive,
    utils::{IsAlive, Transform},
    wayland::input_method::InputMethodSeat,
    wayland::{compositor, input_method::InputMethodSeat},
};

use crate::state::{AnvilState, Backend, CalloopData};
use crate::{drawing::*, render};

pub const OUTPUT_NAME: &str = "winit";

pub struct WinitData {
    backend: WinitGraphicsBackend,
    damage_tracked_renderer: DamageTrackedRenderer,
    #[cfg(feature = "egl")]
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    full_redraw: u8,
    #[cfg(feature = "debug")]
    pub fps: fps_ticker::Fps,
}

#[cfg(feature = "egl")]
impl DmabufHandler for AnvilState<WinitData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf) -> Result<(), ImportError> {
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

#[cfg(feature = "debug")]
smithay::backend::renderer::output::element::render_elements! {
    pub CustomRenderElements<'a, R>;
    Surface=smithay::backend::renderer::output::element::surface::WaylandSurfaceRenderElement,
    Texture=smithay::backend::renderer::output::element::texture::TextureRenderElement<<R as Renderer>::TextureId>,
    Fps=&'a FpsElement<<R as Renderer>::TextureId>
}

smithay::desktop::space::space_elements! {
    CustomSpaceElements<'a, R>[
        WaylandSurfaceRenderElement,
        TextureRenderElement<<R as Renderer>::TextureId>,
    ];
    Pointer=&'a PointerElement<<R as Renderer>::TextureId>,
    SurfaceTree=SurfaceTree,
}

pub fn run_winit(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let mut display = Display::new().unwrap();

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let (mut backend, mut winit) = match winit::init(log.clone()) {
        Ok(ret) => ret,
        Err(err) => {
            slog::crit!(log, "Failed to initialize Winit backend: {}", err);
            return;
        }
    };
    let size = backend.window_size().physical_size;

    let mode = Mode {
        size,
        refresh: 60_000,
    };
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );
    let _global = output.create_global::<AnvilState<WinitData>>(&display.handle());
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    #[cfg(feature = "debug")]
    let fps_image =
        image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
            .decode()
            .unwrap();
    #[cfg(feature = "debug")]
    let fps_texture = backend
        .renderer()
        .import_memory(
            &fps_image.to_rgba8(),
            (fps_image.width() as i32, fps_image.height() as i32).into(),
            false,
        )
        .expect("Unable to upload FPS texture");
    #[cfg(feature = "debug")]
    let mut fps_element = FpsElement::new(fps_texture);

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

        let damage_tracked_renderer = DamageTrackedRenderer::from_output(&output);

        WinitData {
            backend,
            damage_tracked_renderer,
            #[cfg(feature = "egl")]
            dmabuf_state,
            full_redraw: 0,
            #[cfg(feature = "debug")]
            fps: fps_ticker::Fps::default(),
        }
    };
    let mut state = AnvilState::init(&mut display, event_loop.handle(), data, log.clone(), true);
    state.space.map_output(&output, (0, 0));

    let start_time = std::time::Instant::now();

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    let mut pointer_element = PointerElement::default();

    while state.running.load(Ordering::SeqCst) {
        if winit
            .dispatch_new_events(|event| match event {
                WinitEvent::Resized { size, .. } => {
                    let dh = display.handle();
                    // We only have one output
                    let output = state.space.outputs().next().unwrap().clone();
                    state.space.map_output(&output, (0, 0));
                    let mode = Mode {
                        size,
                        refresh: 60_000,
                    };
                    output.change_current_state(Some(mode), None, None, None);
                    output.set_preferred(mode);
                    crate::shell::fixup_positions(&dh, &mut state.space);
                }
                WinitEvent::Input(event) => {
                    state.process_input_event_windowed(&display.handle(), event, OUTPUT_NAME)
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
            let backend = &mut state.backend_data.backend;
            let cursor_visible: bool;

            let mut elements = Vec::<CustomSpaceElements<'_, _>>::new();
            let mut cursor_guard = state.cursor_status.lock().unwrap();

            // draw the cursor as relevant
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
                reset = !surface.alive();
            }
            if reset {
                *cursor_guard = CursorImageStatus::Default;
            }

            if let CursorImageStatus::Surface(_) = *cursor_guard {
                cursor_visible = false;
            } else {
                cursor_visible = true;
            }

            pointer_element.set_position(state.pointer_location.to_i32_round());
            pointer_element.set_status(cursor_guard.clone());
            elements.push(CustomSpaceElements::Pointer(&pointer_element));

            // draw the dnd icon if any
            if let Some(surface) = state.dnd_icon.as_ref() {
                if surface.alive() {
                    elements.push(CustomSpaceElements::SurfaceTree(
                        smithay::desktop::space::SurfaceTree::from_surface(
                            surface,
                            state.pointer_location.to_i32_round(),
                        ),
                    ));
                }
            }

            // draw input method surface if any
            let input_method = state.seat.input_method().unwrap();
            let rectangle = input_method.coordinates();
            input_method.with_surface(|surface| {
                elements.push(CustomSpaceElements::SurfaceTree(
                    smithay::desktop::space::SurfaceTree::from_surface(
                        surface.clone(),
                        (
                            rectangle.loc.x + rectangle.size.w,
                            (rectangle.loc.y + rectangle.size.h),
                        ),
                    ),
                ));
            });

            #[cfg(feature = "debug")]
            let fps = state.backend_data.fps.avg().round() as u32;
            #[cfg(feature = "debug")]
            fps_element.update_fps(fps);

            let full_redraw = &mut state.backend_data.full_redraw;
            *full_redraw = full_redraw.saturating_sub(1);
            let age = if *full_redraw > 0 {
                0
            } else {
                backend.buffer_age().unwrap_or(0)
            };
            let space = &mut state.space;
            let damage_tracked_renderer = &mut state.backend_data.damage_tracked_renderer;
            let render_res = backend.bind().and_then(|_| {
                let renderer = backend.renderer();

                #[cfg(feature = "debug")]
                let res = render::render_output::<_, _, CustomRenderElements<'_, _>>(
                    &output,
                    damage_tracked_renderer,
                    space,
                    &*elements,
                    &[CustomRenderElements::Fps(&fps_element)],
                    renderer,
                    age,
                    &log,
                )
                .map_err(|err| match err {
                    smithay::backend::renderer::output::OutputRenderError::Rendering(err) => err.into(),
                    _ => unreachable!(),
                });

                #[cfg(not(feature = "debug"))]
                let res = render::render_output::<_, _, SpaceRenderElements<_>>(
                    &output,
                    space,
                    &*elements,
                    &[],
                    renderer,
                    damage_tracked_renderer,
                    age,
                    &log,
                )
                .map_err(|err| match err {
                    smithay::backend::renderer::output::OutputRenderError::Rendering(err) => err.into(),
                    _ => unreachable!(),
                });

                res
            });

            match render_res {
                Ok(Some(damage)) => {
                    if let Err(err) = backend.submit(if age == 0 { None } else { Some(&*damage) }) {
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
