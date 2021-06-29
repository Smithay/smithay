use std::{
    cell::RefCell,
    collections::hash_map::{Entry, HashMap},
    fs::{File, OpenOptions},
    rc::Rc,
    os::unix::io::{AsRawFd, RawFd},
    sync::atomic::Ordering,
    time::Duration,
};

use image::ImageBuffer;
use slog::Logger;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        drm::{DrmDevice, DrmError, DrmEvent, GbmBufferedSurface},
        egl::{EGLContext, EGLDisplay},
        renderer::{
            gles2::{Gles2Renderer, Gles2Texture},
            Bind, Frame, Renderer, Transform,
        },
        SwapBuffersError,
    },
    reexports::{
        calloop::{
            timer::{Timer, TimerHandle},
            Dispatcher, EventLoop, LoopHandle, RegistrationToken,
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
        gbm::Device as GbmDevice,
        wayland_server::{
            protocol::{wl_output, wl_surface},
            Display, Global,
        },
    },
    utils::Rectangle,
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
    },
};
#[cfg(feature = "egl")]
use smithay::{
    backend::renderer::{ImportDma, ImportEgl},
    wayland::dmabuf::init_dmabuf_global,
};

use crate::state::{AnvilState, Backend};
use crate::{drawing::*, window_map::WindowMap};

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

pub type RenderSurface = GbmBufferedSurface<FileWrapper>;

pub struct RawData {
    pub output_map: Vec<MyOutput>,
    _registration_token: RegistrationToken,
    _event_dispatcher: Dispatcher<'static, DrmDevice<FileWrapper>, AnvilState<RawData>>,
    render_timer: TimerHandle<crtc::Handle>,
    surfaces: Rc<RefCell<HashMap<crtc::Handle, Rc<RefCell<RenderSurface>>>>>,
    pointer_image: Gles2Texture,
    renderer: Rc<RefCell<Gles2Renderer>>,
    _egl: EGLDisplay,
}

impl Backend for RawData {
    fn seat_name(&self) -> String {
        String::from("anvil")
    }
}

pub fn run_raw(
    display: Rc<RefCell<Display>>,
    event_loop: &mut EventLoop<'static, AnvilState<RawData>>,
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

    // No session
    // This is for debugging only

    let mut output_map = Vec::new();
    let timer = Timer::new().unwrap();

    /*
     * Initialize the backend
     */
    // Try to open the device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let data = if let Some((mut device, gbm)) = options.open(path.as_ref())
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
        
        let renderer = Rc::new(RefCell::new(unsafe {
            Gles2Renderer::new(context, log.clone()).unwrap()
        }));

        #[cfg(feature = "egl")]
        {
            info!(
                log,
                "Initializing EGL Hardware Acceleration via {:?}", path.as_ref()
            );
            renderer.borrow_mut().bind_wl_display(&*display.borrow()).ok();
            let mut formats = Vec::new();
            formats.extend(renderer.borrow().dmabuf_formats().cloned());

            init_dmabuf_global(
                &mut *display.borrow_mut(),
                formats,
                |buffer, mut ddata| {
                    let anvil_state = ddata.get::<AnvilState<RawData>>().unwrap();
                    anvil_state.backend_data.renderer.borrow_mut().import_dmabuf(buffer).is_ok()
                },
                log.clone(),
            );
        }

        let backends = Rc::new(RefCell::new(scan_connectors(
            &mut device,
            &gbm,
            &mut *renderer.borrow_mut(),
            &mut *display.borrow_mut(),
            &mut output_map,
            &log,
        )));

        let bytes = include_bytes!("../resources/cursor2.rgba");
        let pointer_image = {
            let image = ImageBuffer::from_raw(64, 64, bytes.to_vec()).unwrap();
            renderer
                .borrow_mut()
                .import_bitmap(&image)
                .expect("Failed to load pointer")
        };

        let event_dispatcher = Dispatcher::new(
            device,
            move |event, _, anvil_state: &mut AnvilState<_>| match event {
                DrmEvent::VBlank(crtc) => anvil_state.render(Some(crtc)),
                DrmEvent::Error(error) => {
                    error!(anvil_state.log, "{:?}", error);
                }
            },
        );
        let registration_token = event_loop.handle().register_dispatcher(event_dispatcher.clone()).unwrap();

        for surface in backends.borrow_mut().values() {
            // render first frame
            trace!(log, "Scheduling frame");
            schedule_initial_render(surface.clone(), renderer.clone(), &event_loop.handle(), log.clone());
        }

        RawData {
            output_map,
            surfaces: backends,
            renderer,
            _egl: egl,
            pointer_image,
            _event_dispatcher: event_dispatcher,
            _registration_token: registration_token,
            render_timer: timer.handle(),
        }
    } else {
        return Err(());
    };


    /*
     * Initialize the compositor
     */
    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        data,
        log.clone(),
    );

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
    renderer: &mut Gles2Renderer,
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
                    
                    let renderer_formats =
                        Bind::<Dmabuf>::supported_formats(renderer).expect("Dmabuf renderer without formats");                    
                    let renderer =
                        match GbmBufferedSurface::new(surface, gbm.clone(), renderer_formats, logger.clone()) {
                            Ok(renderer) => renderer,
                            Err(err) => {
                                warn!(logger, "Failed to create rendering surface: {}", err);
                                continue;
                            }
                        };

                    output_map.push(MyOutput::new(
                        display,
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
    pub crtc: crtc::Handle,
    pub size: (u32, u32),
    _wl: Output,
    global: Option<Global<wl_output::WlOutput>>,
}

impl MyOutput {
    fn new(
        display: &mut Display,
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

impl AnvilState<RawData> {
    fn render(&mut self, crtc: Option<crtc::Handle>) {
        // setup two iterators on the stack, one over all surfaces for this backend, and
        // one containing only the one given as argument.
        // They make a trait-object to dynamically choose between the two
        let surfaces = self.backend_data.surfaces.borrow();
        let mut surfaces_iter = surfaces.iter();
        let mut option_iter = crtc
            .iter()
            .flat_map(|crtc| surfaces.get(&crtc).map(|surface| (crtc, surface)));

        let to_render_iter: &mut dyn Iterator<Item = (&crtc::Handle, &Rc<RefCell<RenderSurface>>)> =
            if crtc.is_some() {
                &mut option_iter
            } else {
                &mut surfaces_iter
            };

        for (&crtc, surface) in to_render_iter {
            let result = render_surface(
                &mut *surface.borrow_mut(),
                &mut *self.backend_data.renderer.borrow_mut(),
                crtc,
                &mut *self.window_map.borrow_mut(),
                &mut self.backend_data.output_map,
                &self.pointer_location,
                &self.backend_data.pointer_image,
                &*self.dnd_icon.lock().unwrap(),
                &mut *self.cursor_status.lock().unwrap(),
                &self.log,
            );
            if let Err(err) = result {
                warn!(self.log, "Error during rendering: {:?}", err);
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
                    debug!(self.log, "Rescheduling");
                    self.backend_data.render_timer.add_timeout(
                        Duration::from_millis(1000 /*a seconds*/ / 60 /*refresh rate*/),
                        crtc,
                    );
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
}

#[allow(clippy::too_many_arguments)]
fn render_surface(
    surface: &mut RenderSurface,
    renderer: &mut Gles2Renderer,
    crtc: crtc::Handle,
    window_map: &mut WindowMap,
    output_map: &mut Vec<MyOutput>,
    pointer_location: &(f64, f64),
    pointer_image: &Gles2Texture,
    dnd_icon: &Option<wl_surface::WlSurface>,
    cursor_status: &mut CursorImageStatus,
    logger: &slog::Logger,
) -> Result<(), SwapBuffersError> {
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

    let dmabuf = surface.next_buffer()?;
    renderer.bind(dmabuf)?;
    // and draw to our buffer
    match renderer
        .render(
            width,
            height,
            Transform::Flipped180, // Scanout is rotated
            |renderer, frame| {
                frame.clear([0.8, 0.8, 0.9, 1.0])?;
                // draw the surfaces
                draw_windows(
                    renderer,
                    frame,
                    window_map,
                    Some(Rectangle {
                        x: x as i32,
                        y: y as i32,
                        width: width as i32,
                        height: height as i32,
                    }),
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
                                draw_dnd_icon(renderer, frame, wl_surface, (ptr_x, ptr_y), logger)?;
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
                            draw_cursor(renderer, frame, wl_surface, (ptr_x, ptr_y), logger)?;
                        } else {
                            frame.render_texture_at(pointer_image, (ptr_x, ptr_y), Transform::Normal, 1.0)?;
                        }
                    }
                }
                Ok(())
            },
        )
        .map_err(Into::<SwapBuffersError>::into)
        .and_then(|x| x)
        .map_err(Into::<SwapBuffersError>::into)
    {
        Ok(()) => surface.queue_buffer().map_err(Into::<SwapBuffersError>::into),
        Err(err) => Err(err),
    }
}

fn schedule_initial_render<Data: 'static>(
    surface: Rc<RefCell<RenderSurface>>,
    renderer: Rc<RefCell<Gles2Renderer>>,
    evt_handle: &LoopHandle<'static, Data>,
    logger: ::slog::Logger,
) {
    let result = {
        let mut surface = surface.borrow_mut();
        let mut renderer = renderer.borrow_mut();
        initial_render(&mut *surface, &mut *renderer)
    };
    if let Err(err) = result {
        match err {
            SwapBuffersError::AlreadySwapped => {}
            SwapBuffersError::TemporaryFailure(err) => {
                // TODO dont reschedule after 3(?) retries
                warn!(logger, "Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |_| schedule_initial_render(surface, renderer, &handle, logger));
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}

fn initial_render(surface: &mut RenderSurface, renderer: &mut Gles2Renderer) -> Result<(), SwapBuffersError> {
    let dmabuf = surface.next_buffer()?;
    renderer.bind(dmabuf)?;
    // Does not matter if we render an empty frame
    renderer
        .render(1, 1, Transform::Normal, |_, frame| {
            frame
                .clear([0.8, 0.8, 0.9, 1.0])
                .map_err(Into::<SwapBuffersError>::into)
        })
        .map_err(Into::<SwapBuffersError>::into)
        .and_then(|x| x.map_err(Into::<SwapBuffersError>::into))?;
    surface.queue_buffer()?;
    Ok(())
}