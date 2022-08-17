use std::{
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use crate::{
    drawing::*,
    state::{AnvilState, Backend, CalloopData},
};
use slog::Logger;
#[cfg(feature = "debug")]
use smithay::backend::renderer::gles2::Gles2Texture;
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
        egl::{EGLContext, EGLDisplay},
        renderer::{
            gles2::Gles2Renderer,
            output::{
                element::{
                    surface::WaylandSurfaceRenderElement, texture::TextureRenderElement,
                    BuiltinRenderElements, RenderElement,
                },
                OutputRender,
            },
            Bind, ImportAll, ImportMem, Renderer,
        },
        x11::{WindowBuilder, X11Backend, X11Event, X11Surface},
    },
    desktop::space::{SpaceElement, SurfaceTree},
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        gbm,
        wayland_server::{protocol::wl_surface, Display},
    },
    utils::IsAlive,
    wayland::compositor,
};

pub const OUTPUT_NAME: &str = "x11";

#[derive(Debug)]
pub struct X11Data {
    render: bool,
    mode: Mode,
    // FIXME: If Gles2Renderer is dropped before X11Surface, then the MakeCurrent call inside Gles2Renderer will
    // fail because the X11Surface is keeping gbm alive.
    renderer: Gles2Renderer,
    output_render: OutputRender,
    surface: X11Surface,
    #[cfg(feature = "egl")]
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
}

#[cfg(feature = "egl")]
impl DmabufHandler for AnvilState<X11Data> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf) -> Result<(), ImportError> {
        self.backend_data
            .renderer
            .import_dmabuf(&dmabuf, None)
            .map(|_| ())
            .map_err(|_| ImportError::Failed)
    }
}
#[cfg(feature = "egl")]
delegate_dmabuf!(AnvilState<X11Data>);

impl Backend for X11Data {
    fn seat_name(&self) -> String {
        "x11".to_owned()
    }
    fn reset_buffers(&mut self, _output: &Output) {
        self.surface.reset_buffers();
    }
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
}

static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../resources/cursor.rgba");

pub fn run_x11(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let mut display = Display::new().unwrap();

    let backend = X11Backend::new(log.clone()).expect("Failed to initilize X11 backend");
    let handle = backend.handle();

    // Obtain the DRM node the X server uses for direct rendering.
    let (_, fd) = handle
        .drm_node()
        .expect("Could not get DRM node used by X server");

    // Create the gbm device for buffer allocation.
    let device = gbm::Device::new(fd).expect("Failed to create gbm device");
    // Initialize EGL using the GBM device.
    let egl = unsafe { EGLDisplay::new(&device, log.clone()).expect("Failed to create EGLDisplay") };
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

    let mut renderer =
        unsafe { Gles2Renderer::new(context, log.clone()) }.expect("Failed to initialize renderer");

    #[cfg(feature = "egl")]
    let dmabuf_state = if renderer.bind_wl_display(&display.handle()).is_ok() {
        info!(log, "EGL hardware-acceleration enabled");
        let dmabuf_formats = renderer.dmabuf_formats().cloned().collect::<Vec<_>>();
        let mut state = DmabufState::new();
        let global =
            state.create_global::<AnvilState<X11Data>, _>(&display.handle(), dmabuf_formats, log.clone());
        Some((state, global))
    } else {
        None
    };

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
    #[cfg(feature = "debug")]
    let fps_texture = renderer
        .import_memory(
            &fps_image.to_rgba8(),
            (fps_image.width() as i32, fps_image.height() as i32).into(),
            false,
        )
        .expect("Unable to upload FPS texture");
    #[cfg(feature = "debug")]
    let mut fps_element = FpsElement::new(fps_texture);
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "X11".into(),
        },
        log.clone(),
    );
    let _global = output.create_global::<AnvilState<X11Data>>(&display.handle());
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);

    let fallback_cursor = renderer
        .import_memory(FALLBACK_CURSOR_DATA, (64, 64).into(), false)
        .expect("failed to load fallback cursor");

    let output_render = OutputRender::new(&output);

    let data = X11Data {
        render: true,
        mode,
        surface,
        renderer,
        output_render,
        #[cfg(feature = "egl")]
        dmabuf_state,
        #[cfg(feature = "debug")]
        fps: fps_ticker::Fps::default(),
    };

    let mut state = AnvilState::init(&mut display, event_loop.handle(), data, log.clone(), true);

    state.space.map_output(&output, (0, 0));

    let output_clone = output.clone();
    event_loop
        .handle()
        .insert_source(backend, move |event, _, data| match event {
            X11Event::CloseRequested { .. } => {
                data.state.running.store(false, Ordering::SeqCst);
            }
            X11Event::Resized { new_size, .. } => {
                let output = &output_clone;
                let size = { (new_size.w as i32, new_size.h as i32).into() };

                data.state.backend_data.mode = Mode {
                    size,
                    refresh: 60_000,
                };
                output.delete_mode(output.current_mode().unwrap());
                output.change_current_state(Some(data.state.backend_data.mode), None, None, None);
                output.set_preferred(data.state.backend_data.mode);
                crate::shell::fixup_positions(&data.display.handle(), &mut data.state.space);

                data.state.backend_data.render = true;
            }
            X11Event::PresentCompleted { .. } | X11Event::Refresh { .. } => {
                data.state.backend_data.render = true;
            }
            X11Event::Input(event) => {
                data.state
                    .process_input_event_windowed(&data.display.handle(), event, OUTPUT_NAME)
            }
        })
        .expect("Failed to insert X11 Backend into event loop");

    let start_time = std::time::Instant::now();

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    let mut fallback_pointer_element = PointerElement::new(fallback_cursor, (0, 0));

    while state.running.load(Ordering::SeqCst) {
        if state.backend_data.render {
            let backend_data = &mut state.backend_data;
            // We need to borrow everything we want to refer to inside the renderer callback otherwise rustc is unhappy.
            let (x, y) = state.pointer_location.into();
            fallback_pointer_element.set_position((x as i32, y as i32));
            let cursor_status = &state.cursor_status;
            #[cfg(feature = "debug")]
            let fps = backend_data.fps.avg().round() as u32;
            #[cfg(feature = "debug")]
            fps_element.update_fps(fps);

            let (buffer, age) = backend_data.surface.buffer().expect("gbm device was destroyed");
            if let Err(err) = backend_data.renderer.bind(buffer) {
                error!(log, "Error while binding buffer: {}", err);
                continue;
            }

            let mut cursor_guard = cursor_status.lock().unwrap();
            let mut custom_space_elements: Vec<CustomSpaceElements<'_, _>> = Vec::new();

            // // draw the dnd icon if any
            // if let Some(surface) = state.dnd_icon.as_ref() {
            //     if surface.alive() {
            //         elements.push(
            //             draw_dnd_icon(surface.clone(), state.pointer_location.to_i32_round(), &log).into(),
            //         );
            //     }
            // }

            // draw the cursor as relevant
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
                reset = !surface.alive();
            }
            if reset {
                *cursor_guard = CursorImageStatus::Default;
            }

            fallback_pointer_element.set_status(cursor_guard.clone());
            custom_space_elements.push(CustomSpaceElements::Pointer(&fallback_pointer_element));

            // draw the dnd icon if any
            if let Some(surface) = state.dnd_icon.as_ref() {
                if surface.alive() {
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

                    custom_space_elements.push(CustomSpaceElements::SurfaceTree(
                        smithay::desktop::space::SurfaceTree::from_surface(
                            surface,
                            state.pointer_location.to_i32_round() - cursor_hotspot,
                        ),
                    ));
                }
            }

            #[cfg(feature = "debug")]
            let render_res = smithay::desktop::space::render_output::<_, _, CustomRenderElements<'_, _>>(
                &mut backend_data.renderer,
                age.into(),
                &[(&state.space, &*custom_space_elements)],
                vec![CustomRenderElements::Fps(&fps_element)],
                &mut backend_data.output_render,
                &log,
            );

            #[cfg(not(feature = "debug"))]
            let render_res = smithay::desktop::space::render_output::<_, _, BuiltinRenderElements<_>>(
                &mut backend_data.renderer,
                age.into(),
                &[(&state.space, &*custom_space_elements)],
                vec![],
                &mut backend_data.output_render,
                &log,
            );

            match render_res {
                Ok(_) => {
                    trace!(log, "Finished rendering");
                    if let Err(err) = backend_data.surface.submit() {
                        backend_data.surface.reset_buffers();
                        warn!(log, "Failed to submit buffer: {}. Retrying", err);
                    } else {
                        state.backend_data.render = false;
                    };
                }
                Err(err) => {
                    backend_data.surface.reset_buffers();
                    error!(log, "Rendering error: {}", err);
                    // TODO: convert RenderError into SwapBuffersError and skip temporary (will retry) and panic on ContextLost or recreate
                }
            }

            #[cfg(feature = "debug")]
            state.backend_data.fps.tick();
            window.set_cursor_visible(false);
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
    }
}

#[cfg(feature = "debug")]
smithay::backend::renderer::output::element::render_elements! {
    pub CustomRenderElements<'a, R>;
    Surface=smithay::backend::renderer::output::element::surface::WaylandSurfaceRenderElement<R>,
    Texture=smithay::backend::renderer::output::element::texture::TextureRenderElement<R>,
    Fps=&'a FpsElement<<R as Renderer>::TextureId>
}

pub enum CustomSpaceElements<'a, R>
where
    R: Renderer,
{
    Pointer(&'a PointerElement<<R as Renderer>::TextureId>),
    SurfaceTree(SurfaceTree),
}

impl<'a, R, E> SpaceElement<R, E> for CustomSpaceElements<'a, R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Clone + 'static,
    E: RenderElement<R> + From<TextureRenderElement<R>> + From<WaylandSurfaceRenderElement<R>>,
{
    fn z_index(&self, space_id: usize) -> u8 {
        match self {
            CustomSpaceElements::Pointer(p) => SpaceElement::<R, E>::z_index(*p, space_id),
            CustomSpaceElements::SurfaceTree(s) => SpaceElement::<R, E>::z_index(s, space_id),
        }
    }

    fn location(&self, space_id: usize) -> smithay::utils::Point<i32, smithay::utils::Logical> {
        match self {
            CustomSpaceElements::Pointer(p) => SpaceElement::<R, E>::location(*p, space_id),
            CustomSpaceElements::SurfaceTree(s) => SpaceElement::<R, E>::location(s, space_id),
        }
    }

    fn geometry(&self, space_id: usize) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
        match self {
            CustomSpaceElements::Pointer(p) => SpaceElement::<R, E>::geometry(*p, space_id),
            CustomSpaceElements::SurfaceTree(s) => SpaceElement::<R, E>::geometry(s, space_id),
        }
    }

    fn render_elements(
        &self,
        location: smithay::utils::Point<i32, smithay::utils::Physical>,
        scale: smithay::utils::Scale<f64>,
    ) -> Vec<E> {
        match self {
            CustomSpaceElements::Pointer(p) => p.render_elements(location, scale),
            CustomSpaceElements::SurfaceTree(s) => s.render_elements(location, scale),
        }
    }
}

// smithay::backend::renderer::output::element::render_elements! {
//     pub CustomRenderElements<R>;
//     Surface=smithay::backend::renderer::output::element::surface::WaylandSurfaceRenderElement<R>,
//     Texture=smithay::backend::renderer::output::element::texture::TextureRenderElement<R>,
// }

// enum CustomRenderElements<'a, R>
// where
//     R: Renderer + ImportAll,
//     <R as Renderer>::TextureId: Texture + 'static,
// {
//     Surface(smithay::backend::renderer::output::element::surface::WaylandSurfaceRenderElement<R>),
//     Texture(smithay::backend::renderer::output::element::texture::TextureRenderElement<'a, R>),
// }

// impl<'a, R> smithay::backend::renderer::output::element::RenderElement<R> for CustomRenderElements<'a, R>
// where
//     R: Renderer + ImportAll,
// {
//     fn id(&self) -> &smithay::backend::renderer::output::element::Id {
//         match self {
//             CustomRenderElements::Surface(s) => s.id(),
//             CustomRenderElements::Texture(t) => t.id(),
//         }
//     }

//     fn current_commit(&self) -> usize {
//         match self {
//             CustomRenderElements::Surface(s) => s.current_commit(),
//             CustomRenderElements::Texture(t) => t.current_commit(),
//         }
//     }

//     fn location(
//         &self,
//         scale: smithay::utils::Scale<f64>,
//     ) -> smithay::utils::Point<i32, smithay::utils::Physical> {
//         match self {
//             CustomRenderElements::Surface(s) => s.location(scale),
//             CustomRenderElements::Texture(t) => t.location(scale),
//         }
//     }

//     fn geometry(
//         &self,
//         scale: smithay::utils::Scale<f64>,
//     ) -> smithay::utils::Rectangle<i32, smithay::utils::Physical> {
//         match self {
//             CustomRenderElements::Surface(s) => s.geometry(scale),
//             CustomRenderElements::Texture(t) => t.geometry(scale),
//         }
//     }

//     fn damage_since(
//         &self,
//         scale: smithay::utils::Scale<f64>,
//         commit: Option<usize>,
//     ) -> Vec<smithay::utils::Rectangle<i32, smithay::utils::Physical>> {
//         match self {
//             CustomRenderElements::Surface(s) => s.damage_since(scale, commit),
//             CustomRenderElements::Texture(t) => t.damage_since(scale, commit),
//         }
//     }

//     fn opaque_regions(
//         &self,
//         scale: smithay::utils::Scale<f64>,
//     ) -> Vec<smithay::utils::Rectangle<i32, smithay::utils::Physical>> {
//         match self {
//             CustomRenderElements::Surface(s) => s.opaque_regions(scale),
//             CustomRenderElements::Texture(t) => t.opaque_regions(scale),
//         }
//     }

//     fn underlying_storage(
//         &self,
//         renderer: &R,
//     ) -> Option<smithay::backend::renderer::output::element::UnderlyingStorage<'_, R>> {
//         match self {
//             CustomRenderElements::Surface(s) => s.underlying_storage(renderer),
//             CustomRenderElements::Texture(t) => t.underlying_storage(renderer),
//         }
//     }

//     fn draw(
//         &self,
//         renderer: &mut R,
//         frame: &mut <R as Renderer>::Frame,
//         scale: smithay::utils::Scale<f64>,
//         damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
//         log: &slog::Logger,
//     ) {
//         match self {
//             CustomRenderElements::Surface(s) => s.draw(renderer, frame, scale, damage, log),
//             CustomRenderElements::Texture(t) => t.draw(renderer, frame, scale, damage, log),
//         }
//     }
// }
