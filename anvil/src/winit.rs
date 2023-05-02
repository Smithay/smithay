#[cfg(feature = "xwayland")]
use std::ffi::OsString;
use std::{
    sync::{atomic::Ordering, Mutex},
    time::Duration,
};

#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;
#[cfg(feature = "debug")]
use smithay::{
    backend::{allocator::Fourcc, renderer::ImportMem},
    reexports::winit::platform::unix::WindowExtUnix,
};

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        color::{lcms::LcmsContext, null::NullCMS, CMS},
        egl::EGLDevice,
        renderer::{
            damage::{Error as OutputDamageTrackerError, OutputDamageTracker},
            element::AsRenderElements,
            gles::{GlesRenderer, GlesTexture},
            ImportDma, ImportMemWl,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
        SwapBuffersError,
    },
    delegate_dmabuf,
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{protocol::wl_surface, Display},
    },
    utils::{IsAlive, Point, Scale, Transform},
    wayland::{
        compositor,
        dmabuf::{
            DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportError,
        },
        input_method::InputMethodSeat,
    },
};
use tracing::{error, info, warn};

use crate::state::{post_repaint, take_presentation_feedback, AnvilState, Backend, CalloopData};
use crate::{drawing::*, render::*};

pub const OUTPUT_NAME: &str = "winit";

pub struct WinitData<C: CMS + 'static> {
    backend: WinitGraphicsBackend<GlesRenderer>,
    cms: C,
    damage_tracker: OutputDamageTracker,
    dmabuf_state: (DmabufState, DmabufGlobal, Option<DmabufFeedback>),
    full_redraw: u8,
    #[cfg(feature = "debug")]
    pub fps: fps_ticker::Fps,
}

impl<C: CMS + 'static> DmabufHandler for AnvilState<WinitData<C>> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.0
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
delegate_dmabuf!(@<C: CMS + 'static> AnvilState<WinitData<C>>);

impl<C: CMS + 'static> Backend for WinitData<C> {
    fn seat_name(&self) -> String {
        String::from("winit")
    }
    fn reset_buffers(&mut self, _output: &Output) {
        self.full_redraw = 4;
    }
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
}

pub fn run_winit(mut backend_args: impl Iterator<Item = String>) {
    let mut color = None;
    loop {
        match (backend_args.next(), backend_args.next()) {
            (Some(arg), Some(value)) => match &*arg {
                "--color" => {
                    color = Some(value);
                }
                x => {
                    error!("Unknown argument: {x}");
                    return;
                }
            },
            (Some(arg), None) => {
                error!("Unmatched argument: {arg}");
                return;
            }
            _ => break,
        }
    }

    match color.as_deref().unwrap_or("lcms") {
        "null" => run_winit_internal(NullCMS),
        "lcms" => run_winit_internal(LcmsContext::new()),
        x => error!("Unknown color argument value: {x}"),
    }
}

fn run_winit_internal<C: CMS + 'static>(cms: C) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let mut display = Display::new().unwrap();

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let (mut backend, mut winit) = match winit::init::<GlesRenderer>() {
        Ok(ret) => ret,
        Err(err) => {
            error!("Failed to initialize Winit backend: {}", err);
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
    );
    let _global = output.create_global::<AnvilState<WinitData<C>>>(&display.handle());
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
            Fourcc::Abgr8888,
            (fps_image.width() as i32, fps_image.height() as i32).into(),
            false,
        )
        .expect("Unable to upload FPS texture");
    #[cfg(feature = "debug")]
    let mut fps_element = FpsElement::new(fps_texture);

    let render_node = EGLDevice::device_for_display(backend.renderer().egl_context().display())
        .and_then(|device| device.try_get_render_node());

    let dmabuf_default_feedback = match render_node {
        Ok(Some(node)) => {
            let dmabuf_formats = backend.renderer().dmabuf_formats().collect::<Vec<_>>();
            let dmabuf_default_feedback = DmabufFeedbackBuilder::new(node.dev_id(), dmabuf_formats)
                .build()
                .unwrap();
            Some(dmabuf_default_feedback)
        }
        Ok(None) => {
            warn!("failed to query render node, dmabuf will use v3");
            None
        }
        Err(err) => {
            warn!(?err, "failed to egl device for display, dmabuf will use v3");
            None
        }
    };

    // if we failed to build dmabuf feedback we fall back to dmabuf v3
    // Note: egl on Mesa requires either v4 or wl_drm (initialized with bind_wl_display)
    let dmabuf_state = if let Some(default_feedback) = dmabuf_default_feedback {
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global = dmabuf_state.create_global_with_default_feedback::<AnvilState<WinitData<C>>>(
            &display.handle(),
            &default_feedback,
        );
        (dmabuf_state, dmabuf_global, Some(default_feedback))
    } else {
        let dmabuf_formats = backend.renderer().dmabuf_formats().collect::<Vec<_>>();
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global =
            dmabuf_state.create_global::<AnvilState<WinitData<C>>>(&display.handle(), dmabuf_formats);
        (dmabuf_state, dmabuf_global, None)
    };

    #[cfg(feature = "egl")]
    if backend.renderer().bind_wl_display(&display.handle()).is_ok() {
        info!("EGL hardware-acceleration enabled");
    };

    let data = {
        let damage_tracker = OutputDamageTracker::from_output(&output);

        WinitData {
            backend,
            cms,
            damage_tracker,
            dmabuf_state,
            full_redraw: 0,
            #[cfg(feature = "debug")]
            fps: fps_ticker::Fps::default(),
        }
    };
    let mut state = AnvilState::init(&mut display, event_loop.handle(), data, true);
    state
        .shm_state
        .update_formats(state.backend_data.backend.renderer().shm_formats());
    state.space.map_output(&output, (0, 0));

    #[cfg(feature = "xwayland")]
    if let Err(e) = state.xwayland.start(
        state.handle.clone(),
        None,
        std::iter::empty::<(OsString, OsString)>(),
        true,
        |_| {},
    ) {
        error!("Failed to start XWayland: {}", e);
    }

    info!("Initialization completed, starting the main loop.");

    let mut pointer_element = PointerElement::<GlesTexture>::default();

    while state.running.load(Ordering::SeqCst) {
        if winit
            .dispatch_new_events(|event| match event {
                WinitEvent::Resized { size, .. } => {
                    // We only have one output
                    let output = state.space.outputs().next().unwrap().clone();
                    state.space.map_output(&output, (0, 0));
                    let mode = Mode {
                        size,
                        refresh: 60_000,
                    };
                    output.change_current_state(Some(mode), None, None, None);
                    output.set_preferred(mode);
                    crate::shell::fixup_positions(&mut state.space);
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
            let cms = &mut state.backend_data.cms;
            let backend = &mut state.backend_data.backend;

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
            let cursor_visible = !matches!(*cursor_guard, CursorImageStatus::Surface(_));

            pointer_element.set_status(cursor_guard.clone());

            #[cfg(feature = "debug")]
            let fps = state.backend_data.fps.avg().round() as u32;
            #[cfg(feature = "debug")]
            fps_element.update_fps(fps);

            let full_redraw = &mut state.backend_data.full_redraw;
            *full_redraw = full_redraw.saturating_sub(1);
            let space = &mut state.space;
            let damage_tracker = &mut state.backend_data.damage_tracker;
            let show_window_preview = state.show_window_preview;

            let input_method = state.seat.input_method().unwrap();
            let dnd_icon = state.dnd_icon.as_ref();

            let scale = Scale::from(output.current_scale().fractional_scale());
            let cursor_hotspot = if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
                compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                })
            } else {
                (0, 0).into()
            };
            let cursor_pos = state.pointer_location - cursor_hotspot.to_f64();
            let cursor_pos_scaled = cursor_pos.to_physical(scale).to_i32_round();

            #[cfg(feature = "debug")]
            let mut renderdoc = state.renderdoc.as_mut();
            let render_res = backend.bind().and_then(|_| {
                #[cfg(feature = "debug")]
                if let Some(renderdoc) = renderdoc.as_mut() {
                    renderdoc.start_frame_capture(
                        backend.renderer().egl_context().get_context_handle(),
                        backend
                            .window()
                            .wayland_surface()
                            .unwrap_or_else(std::ptr::null_mut),
                    );
                }
                let age = if *full_redraw > 0 {
                    0
                } else {
                    backend.buffer_age().unwrap_or(0)
                };

                let renderer = backend.renderer();

                let mut elements = Vec::<CustomRenderElements<GlesRenderer, C>>::new();

                elements.extend(pointer_element.render_elements(renderer, cms, cursor_pos_scaled, scale));

                // draw input method surface if any
                let rectangle = input_method.coordinates();
                let position = Point::from((
                    rectangle.loc.x + rectangle.size.w,
                    rectangle.loc.y + rectangle.size.h,
                ));
                input_method.with_surface(|surface| {
                    elements.extend(AsRenderElements::<GlesRenderer, _>::render_elements(
                        &smithay::desktop::space::SurfaceTree::from_surface(surface),
                        renderer,
                        cms,
                        position.to_physical_precise_round(scale),
                        scale,
                    ));
                });

                // draw the dnd icon if any
                if let Some(surface) = dnd_icon {
                    if surface.alive() {
                        elements.extend(AsRenderElements::<GlesRenderer, _>::render_elements(
                            &smithay::desktop::space::SurfaceTree::from_surface(surface),
                            renderer,
                            cms,
                            cursor_pos_scaled,
                            scale,
                        ));
                    }
                }

                #[cfg(feature = "debug")]
                elements.push(CustomRenderElements::Fps(fps_element.clone()));

                let output_profile = cms.profile_srgb();
                render_output(
                    &output,
                    space,
                    elements,
                    renderer,
                    cms,
                    &output_profile,
                    damage_tracker,
                    age,
                    show_window_preview,
                )
                .map_err(|err| match err {
                    OutputDamageTrackerError::Rendering(err) => err.into(),
                    _ => unreachable!(),
                })
            });

            match render_res {
                Ok((damage, states)) => {
                    let has_rendered = damage.is_some();
                    if let Some(damage) = damage {
                        if let Err(err) = backend.submit(Some(&*damage)) {
                            warn!("Failed to submit buffer: {}", err);
                        }
                    }

                    #[cfg(feature = "debug")]
                    if let Some(renderdoc) = renderdoc.as_mut() {
                        renderdoc.end_frame_capture(
                            backend.renderer().egl_context().get_context_handle(),
                            backend
                                .window()
                                .wayland_surface()
                                .unwrap_or_else(std::ptr::null_mut),
                        );
                    }

                    backend.window().set_cursor_visible(cursor_visible);

                    // Send frame events so that client start drawing their next frame
                    let time = state.clock.now();
                    post_repaint(&output, &states, &state.space, None, time);

                    if has_rendered {
                        let mut output_presentation_feedback =
                            take_presentation_feedback(&output, &state.space, &states);
                        output_presentation_feedback.presented(
                            time,
                            output
                                .current_mode()
                                .map(|mode| mode.refresh as u32)
                                .unwrap_or_default(),
                            0,
                            wp_presentation_feedback::Kind::Vsync,
                        )
                    }
                }
                Err(SwapBuffersError::ContextLost(err)) => {
                    #[cfg(feature = "debug")]
                    if let Some(renderdoc) = renderdoc.as_mut() {
                        renderdoc.discard_frame_capture(
                            backend.renderer().egl_context().get_context_handle(),
                            backend
                                .window()
                                .wayland_surface()
                                .unwrap_or_else(std::ptr::null_mut),
                        );
                    }

                    error!("Critical Rendering Error: {}", err);
                    state.running.store(false, Ordering::SeqCst);
                }
                Err(err) => warn!("Rendering error: {}", err),
            }
        }

        let mut calloop_data = CalloopData { state, display };
        let result = event_loop.dispatch(Some(Duration::from_millis(1)), &mut calloop_data);
        CalloopData { state, display } = calloop_data;

        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.refresh();
            state.popups.cleanup();
            display.flush_clients().unwrap();
        }

        #[cfg(feature = "debug")]
        state.backend_data.fps.tick();
    }
}
