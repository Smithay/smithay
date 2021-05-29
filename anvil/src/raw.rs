use std::{
    cell::RefCell,
    collections::hash_map::{Entry, HashMap},
    io::Error as IoError,
    fs::{File, OpenOptions},
    rc::Rc,
    os::unix::io::{AsRawFd, RawFd},
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use image::ImageBuffer;
use slog::Logger;

#[cfg(feature = "egl")]
use smithay::{
    backend::egl::display::EGLBufferReader,
};
use smithay::{
    backend::{
        drm::{device_bind, DeviceHandler, DrmDevice, DrmError, DrmRenderSurface},
        egl::{EGLContext, EGLDisplay},
        renderer::{
            gles2::{Gles2Renderer, Gles2Texture},
            Frame, Renderer, Transform,
        },
        SwapBuffersError,
    },
    reexports::{
        calloop::{
            generic::Generic,
            timer::{Timer, TimerHandle},
            EventLoop, LoopHandle, Source,
        },
        drm::{
            self,
            control::{
                connector::{Info as ConnectorInfo, State as ConnectorState},
                crtc,
                encoder::Info as EncoderInfo,
                Device as ControlDevice,
            },
        },
        gbm::{BufferObject as GbmBuffer, Device as GbmDevice},
        nix::sys::stat::dev_t,
        wayland_server::{
            protocol::{wl_output, wl_surface, wl_buffer},
            Display, Global,
        },
    },
    utils::Rectangle,
    wayland::{
        compositor::CompositorToken,
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use crate::drawing::*;
use crate::shell::{MyWindowMap, Roles};
use crate::state::AnvilState;

pub struct FileWrapper(File);
impl AsRawFd for FileWrapper {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}
impl Clone for FileWrapper {
    fn clone(&self) -> Self {
        FileWrapper(self.0.try_clone().unwrap())
    }
}

pub fn run_raw(
    display: Rc<RefCell<Display>>,
    event_loop: &mut EventLoop<AnvilState>,
    path: impl AsRef<str>,
    log: Logger,
) -> Result<(), ()> {
    let name = display
        .borrow_mut()
        .add_socket_auto()
        .unwrap()
        .into_string()
        .unwrap();
    info!(log, "Listening on wayland socket"; "name" => name.clone());
    ::std::env::set_var("WAYLAND_DISPLAY", name);

    #[cfg(feature = "egl")]
    let egl_buffer_reader = Rc::new(RefCell::new(None));

    let output_map = Rc::new(RefCell::new(Vec::new()));

    /*
     * Initialize the compositor
     */
    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        #[cfg(feature = "egl")]
        egl_buffer_reader.clone(),
        None,
        None,
        log.clone(),
    );

    /*
     * Initialize the backend
     */
    // Try to open the device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let _backend = if let Some((mut device, gbm)) = options.open(path.as_ref())
        .ok()
        .and_then(|fd| {
            let file = FileWrapper(fd);
            match {
                (
                    DrmDevice::new(file.clone(), true, log.clone()),
                    GbmDevice::new(file),
                )
            } {
                (Ok(drm), Ok(gbm)) => Some((drm, gbm)),
                (Err(err), _) => {
                    error!(
                        log,
                        "Aborting initializing {:?}, because of drm error: {}", path.as_ref(), err
                    );
                    None
                }
                (_, Err(err)) => {
                    // TODO try DumbBuffer allocator in this case
                    error!(
                        log,
                        "Aborting initializing {:?}, because of gbm error: {}", path.as_ref(), err
                    );
                    None
                }
            }
        })
    {
        let egl = match EGLDisplay::new(&gbm, log.clone()) {
            Ok(display) => display,
            Err(err) => {
                warn!(
                    log,
                    "Skipping device {:?}, because of egl display error: {}", path.as_ref(), err
                );
                return Err(());
            }
        };

        let context = match EGLContext::new(&egl, log.clone()) {
            Ok(context) => context,
            Err(err) => {
                warn!(
                    log,
                    "Skipping device {:?}, because of egl context error: {}", path.as_ref(), err
                );
                return Err(());
            }
        };

        #[cfg(feature = "egl")]
        {
            info!(
                log,
                "Initializing EGL Hardware Acceleration via {:?}", path.as_ref()
            );
            *egl_buffer_reader.borrow_mut() = egl.bind_wl_display(&*display.borrow()).ok();
        }

        let backends = Rc::new(RefCell::new(scan_connectors(
            &mut device,
            &gbm,
            &egl,
            &context,
            &mut *display.borrow_mut(),
            &mut *output_map.borrow_mut(),
            &log,
        )));

        let bytes = include_bytes!("../resources/cursor2.rgba");
        let pointer_image = {
            let context = EGLContext::new_shared(&egl, &context, log.clone()).unwrap();
            let mut renderer = unsafe { Gles2Renderer::new(context, log.clone()).unwrap() };
            let image = ImageBuffer::from_raw(64, 64, bytes.to_vec()).unwrap();
            renderer
                .import_bitmap(&image)
                .expect("Failed to load pointer")
        };

        // Set the handler.
        // Note: if you replicate this (very simple) structure, it is rather easy
        // to introduce reference cycles with Rc. Be sure about your drop order
        let renderer = Rc::new(DrmRenderer {
            #[cfg(feature = "egl")]
            egl_buffer_reader:egl_buffer_reader.borrow().clone(),
            compositor_token: state.ctoken,
            backends: backends.clone(),
            window_map: state.window_map.clone(),
            output_map: output_map.clone(),
            pointer_location: state.pointer_location.clone(),
            pointer_image,
            cursor_status: state.cursor_status.clone(),
            dnd_icon: state.dnd_icon.clone(),
            logger: log.clone(),
            start_time: std::time::Instant::now(),
        });
        device.set_handler(DrmHandlerImpl {
            renderer,
            loop_handle: event_loop.handle(),
        });

        let event_source = device_bind(&event_loop.handle(), device)
            .map_err(|e| -> IoError { e.into() })
            .unwrap();

        trace!(log, "Backends: {:?}", backends.borrow().keys());
        for renderer in backends.borrow_mut().values() {
            // render first frame
            trace!(log, "Scheduling frame");
            schedule_initial_render(renderer.clone(), &event_loop.handle(), log.clone());
        }

        BackendData {
            event_source,
            surfaces: backends,
            egl,
            context,
        }
    } else {
        return Err(());
    };

    /*
     * And run our loop
     */

    while state.running.load(Ordering::SeqCst) {
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

pub fn scan_connectors(
    device: &mut DrmDevice<FileWrapper>,
    gbm: &GbmDevice<FileWrapper>,
    egl: &EGLDisplay,
    context: &EGLContext,
    display: &mut Display,
    output_map: &mut Vec<MyOutput>,
    logger: &::slog::Logger,
) -> HashMap<crtc::Handle, Rc<RefCell<RenderSurface>>> {
    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = device.resource_handles().unwrap();

    // Use first connected connector
    let connector_infos: Vec<ConnectorInfo> = res_handles
        .connectors()
        .iter()
        .map(|conn| device.get_connector(*conn).unwrap())
        .filter(|conn| conn.state() == ConnectorState::Connected)
        .inspect(|conn| info!(logger, "Connected: {:?}", conn.interface()))
        .collect();

    let mut backends = HashMap::new();

    // very naive way of finding good crtc/encoder/connector combinations. This problem is np-complete
    for connector_info in connector_infos {
        let encoder_infos = connector_info
            .encoders()
            .iter()
            .filter_map(|e| *e)
            .flat_map(|encoder_handle| device.get_encoder(encoder_handle))
            .collect::<Vec<EncoderInfo>>();
        'outer: for encoder_info in encoder_infos {
            for crtc in res_handles.filter_crtcs(encoder_info.possible_crtcs()) {
                if let Entry::Vacant(entry) = backends.entry(crtc) {
                    info!(
                        logger,
                        "Trying to setup connector {:?}-{} with crtc {:?}",
                        connector_info.interface(),
                        connector_info.interface_id(),
                        crtc,
                    );
                    let context = match EGLContext::new_shared(egl, context, logger.clone()) {
                        Ok(context) => context,
                        Err(err) => {
                            warn!(logger, "Failed to create EGLContext: {}", err);
                            continue;
                        }
                    };
                    let renderer = match unsafe { Gles2Renderer::new(context, logger.clone()) } {
                        Ok(renderer) => renderer,
                        Err(err) => {
                            warn!(logger, "Failed to create Gles2 Renderer: {}", err);
                            continue;
                        }
                    };
                    let surface = match device.create_surface(
                        crtc,
                        connector_info.modes()[0],
                        &[connector_info.handle()],
                    ) {
                        Ok(surface) => surface,
                        Err(err) => {
                            warn!(logger, "Failed to create drm surface: {}", err);
                            continue;
                        }
                    };
                    let renderer =
                        match DrmRenderSurface::new(surface, gbm.clone(), renderer, logger.clone()) {
                            Ok(renderer) => renderer,
                            Err(err) => {
                                warn!(logger, "Failed to create rendering surface: {}", err);
                                continue;
                            }
                        };

                    output_map.push(MyOutput::new(
                        display,
                        device.device_id(),
                        crtc,
                        connector_info,
                        logger.clone(),
                    ));

                    entry.insert(Rc::new(RefCell::new(renderer)));
                    break 'outer;
                }
            }
        }
    }

    backends
}

pub struct MyOutput {
    pub device_id: dev_t,
    pub crtc: crtc::Handle,
    pub size: (u32, u32),
    _wl: Output,
    global: Option<Global<wl_output::WlOutput>>,
}

impl MyOutput {
    fn new(
        display: &mut Display,
        device_id: dev_t,
        crtc: crtc::Handle,
        conn: ConnectorInfo,
        logger: ::slog::Logger,
    ) -> MyOutput {
        let (output, global) = Output::new(
            display,
            format!("{:?}", conn.interface()),
            PhysicalProperties {
                width: conn.size().unwrap_or((0, 0)).0 as i32,
                height: conn.size().unwrap_or((0, 0)).1 as i32,
                subpixel: wl_output::Subpixel::Unknown,
                make: "Smithay".into(),
                model: "Generic DRM".into(),
            },
            logger,
        );

        let mode = conn.modes()[0];
        let (w, h) = mode.size();
        output.change_current_state(
            Some(Mode {
                width: w as i32,
                height: h as i32,
                refresh: (mode.vrefresh() * 1000) as i32,
            }),
            None,
            None,
        );
        output.set_preferred(Mode {
            width: w as i32,
            height: h as i32,
            refresh: (mode.vrefresh() * 1000) as i32,
        });

        MyOutput {
            device_id,
            crtc,
            size: (w as u32, h as u32),
            _wl: output,
            global: Some(global),
        }
    }
}

impl Drop for MyOutput {
    fn drop(&mut self) {
        self.global.take().unwrap().destroy();
    }
}

pub type RenderSurface = DrmRenderSurface<FileWrapper, GbmDevice<FileWrapper>, Gles2Renderer, GbmBuffer<()>>;

struct BackendData {
    surfaces: Rc<RefCell<HashMap<crtc::Handle, Rc<RefCell<RenderSurface>>>>>,
    context: EGLContext,
    egl: EGLDisplay,
    event_source: Source<Generic<DrmDevice<FileWrapper>>>,
}

pub struct DrmHandlerImpl<Data: 'static> {
    renderer: Rc<DrmRenderer>,
    loop_handle: LoopHandle<Data>,
}

impl<Data: 'static> DeviceHandler for DrmHandlerImpl<Data> {
    fn vblank(&mut self, crtc: crtc::Handle) {
        self.renderer.clone().render(crtc, None, Some(&self.loop_handle))
    }

    fn error(&mut self, error: DrmError) {
        error!(self.renderer.logger, "{:?}", error);
    }
}

pub struct DrmRenderer {
    #[cfg(feature = "egl")]
    egl_buffer_reader: Option<EGLBufferReader>,
    compositor_token: CompositorToken<Roles>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Rc<RefCell<RenderSurface>>>>>,
    window_map: Rc<RefCell<MyWindowMap>>,
    output_map: Rc<RefCell<Vec<MyOutput>>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    pointer_image: Gles2Texture,
    cursor_status: Arc<Mutex<CursorImageStatus>>,
    dnd_icon: Arc<Mutex<Option<wl_surface::WlSurface>>>,
    logger: ::slog::Logger,
    start_time: std::time::Instant,
}

impl DrmRenderer {
    fn render<Data: 'static>(
        self: Rc<Self>,
        crtc: crtc::Handle,
        timer: Option<TimerHandle<(std::rc::Weak<DrmRenderer>, crtc::Handle)>>,
        evt_handle: Option<&LoopHandle<Data>>,
    ) {
        if let Some(surface) = self.backends.borrow().get(&crtc) {
            let result = DrmRenderer::render_surface(
                &mut *surface.borrow_mut(),
                #[cfg(feature = "egl")]
                self.egl_buffer_reader.as_ref(),
                crtc,
                &mut *self.window_map.borrow_mut(),
                &mut *self.output_map.borrow_mut(),
                &self.compositor_token,
                &*self.pointer_location.borrow(),
                &self.pointer_image,
                &*self.dnd_icon.lock().unwrap(),
                &mut *self.cursor_status.lock().unwrap(),
                &self.logger,
            );
            if let Err(err) = result {
                warn!(self.logger, "Error during rendering: {:?}", err);
                let reschedule = match err {
                    SwapBuffersError::AlreadySwapped => false,
                    SwapBuffersError::TemporaryFailure(err) => !matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(&DrmError::DeviceInactive)
                            | Some(&DrmError::Access {
                                source: drm::SystemError::PermissionDenied,
                                ..
                            })
                    ),
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
                };

                if reschedule {
                    debug!(self.logger, "Rescheduling");
                    match (timer, evt_handle) {
                        (Some(handle), _) => {
                            let _ = handle.add_timeout(
                                Duration::from_millis(1000 /*a seconds*/ / 60 /*refresh rate*/),
                                (Rc::downgrade(&self), crtc),
                            );
                        }
                        (None, Some(evt_handle)) => {
                            let timer = Timer::new().unwrap();
                            let handle = timer.handle();
                            let _ = handle.add_timeout(
                                Duration::from_millis(1000 /*a seconds*/ / 60 /*refresh rate*/),
                                (Rc::downgrade(&self), crtc),
                            );
                            evt_handle
                                .insert_source(timer, |(renderer, crtc), handle, _data| {
                                    if let Some(renderer) = renderer.upgrade() {
                                        renderer.render(
                                            crtc,
                                            Some(handle.clone()),
                                            Option::<&LoopHandle<Data>>::None,
                                        );
                                    }
                                })
                                .unwrap();
                        }
                        _ => unreachable!(),
                    }
                }
            } else {
                // TODO: only send drawn windows the frames callback
                // Send frame events so that client start drawing their next frame
                self.window_map
                    .borrow()
                    .send_frames(self.start_time.elapsed().as_millis() as u32);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_surface(
        surface: &mut RenderSurface,
        #[cfg(feature = "egl")] egl_buffer_reader: Option<&EGLBufferReader>,
        crtc: crtc::Handle,
        window_map: &mut MyWindowMap,
        output_map: &mut Vec<MyOutput>,
        compositor_token: &CompositorToken<Roles>,
        pointer_location: &(f64, f64),
        pointer_image: &Gles2Texture,
        dnd_icon: &Option<wl_surface::WlSurface>,
        cursor_status: &mut CursorImageStatus,
        logger: &slog::Logger,
    ) -> Result<(), SwapBuffersError> {
        #[cfg(not(feature = "egl"))]
        let egl_buffer_reader = None;

        surface.frame_submitted()?;

        // get output coordinates
        let (x, y) = output_map
            .iter()
            .take_while(|output| output.crtc != crtc)
            .fold((0u32, 0u32), |pos, output| (pos.0 + output.size.0, pos.1));
        let (width, height) = output_map
            .iter()
            .find(|output| output.crtc == crtc)
            .map(|output| output.size)
            .unwrap_or((0, 0)); // in this case the output will be removed.

        // and draw in sync with our monitor
        surface.render(|renderer, frame| -> Result<(), SwapBuffersError> {
            frame.clear([0.8, 0.8, 0.9, 1.0])?;
            // draw the surfaces
            draw_windows(
                renderer,
                frame,
                egl_buffer_reader,
                window_map,
                Some(Rectangle {
                    x: x as i32,
                    y: y as i32,
                    width: width as i32,
                    height: height as i32,
                }),
                *compositor_token,
                logger,
            )?;

            // get pointer coordinates
            let (ptr_x, ptr_y) = *pointer_location;
            let ptr_x = ptr_x.trunc().abs() as i32 - x as i32;
            let ptr_y = ptr_y.trunc().abs() as i32 - y as i32;

            // set cursor
            if ptr_x >= 0 && ptr_x < width as i32 && ptr_y >= 0 && ptr_y < height as i32 {
                // draw the dnd icon if applicable
                {
                    if let Some(ref wl_surface) = dnd_icon.as_ref() {
                        if wl_surface.as_ref().is_alive() {
                            draw_dnd_icon(
                                renderer,
                                frame,
                                wl_surface,
                                egl_buffer_reader,
                                (ptr_x, ptr_y),
                                *compositor_token,
                                logger,
                            )?;
                        }
                    }
                }
                // draw the cursor as relevant
                {
                    // reset the cursor if the surface is no longer alive
                    let mut reset = false;
                    if let CursorImageStatus::Image(ref surface) = *cursor_status {
                        reset = !surface.as_ref().is_alive();
                    }
                    if reset {
                        *cursor_status = CursorImageStatus::Default;
                    }

                    if let CursorImageStatus::Image(ref wl_surface) = *cursor_status {
                        draw_cursor(
                            renderer,
                            frame,
                            wl_surface,
                            egl_buffer_reader,
                            (ptr_x, ptr_y),
                            *compositor_token,
                            logger,
                        )?;
                    } else {
                        frame.render_texture_at(pointer_image, (ptr_x, ptr_y), Transform::Normal, 1.0)?;
                    }
                }
            }
            Ok(())
        }).map_err(Into::<SwapBuffersError>::into).and_then(|x| x)
    }
}

fn schedule_initial_render<Data: 'static>(
    renderer: Rc<RefCell<RenderSurface>>,
    evt_handle: &LoopHandle<Data>,
    logger: ::slog::Logger,
) {
    let result = {
        let mut renderer = renderer.borrow_mut();
        // Does not matter if we render an empty frame
        renderer
            .render(|_, frame| {
                frame
                    .clear([0.8, 0.8, 0.9, 1.0])
                    .map_err(Into::<SwapBuffersError>::into)
            })
            .map_err(Into::<SwapBuffersError>::into)
            .and_then(|x| x.map_err(Into::<SwapBuffersError>::into))
    };
    if let Err(err) = result {
        match err {
            SwapBuffersError::AlreadySwapped => {}
            SwapBuffersError::TemporaryFailure(err) => {
                // TODO dont reschedule after 3(?) retries
                warn!(logger, "Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |_| schedule_initial_render(renderer, &handle, logger));
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}