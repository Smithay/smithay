extern crate drm;
#[macro_use]
extern crate glium;
extern crate image;
extern crate input as libinput;
extern crate rand;
#[macro_use(define_roles)]
extern crate smithay;
extern crate udev;
extern crate wayland_server;
extern crate xkbcommon;

#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;

mod helpers;

use drm::control::{Device as ControlDevice, ResourceInfo};
use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
use drm::control::crtc;
use drm::control::encoder::Info as EncoderInfo;
use drm::result::Error as DrmError;
use glium::{Blend, Surface};
use helpers::{init_shell, Buffer, GliumDrawer, MyWindowMap, Roles, SurfaceData};
use image::{ImageBuffer, Rgba};
use libinput::{event, Device as LibinputDevice, Libinput};
use libinput::event::keyboard::KeyboardEventTrait;
use slog::{Drain, Logger};
use smithay::backend::drm::{DevPath, DrmBackend, DrmDevice, DrmHandler};
use smithay::backend::graphics::GraphicsBackend;
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::graphics::egl::wayland::{EGLDisplay, EGLWaylandExtensions, Format};
use smithay::backend::input::{self, Event, InputBackend, InputHandler, KeyState, KeyboardKeyEvent,
                              PointerAxisEvent, PointerButtonEvent};
use smithay::backend::libinput::{libinput_bind, LibinputInputBackend, LibinputSessionInterface,
                                 PointerAxisEvent as LibinputPointerAxisEvent};
use smithay::backend::session::{Session, SessionNotifier};
use smithay::backend::session::auto::{auto_session_bind, AutoSession};
use smithay::backend::udev::{primary_gpu, udev_backend_bind, SessionFdDrmDevice, UdevBackend, UdevHandler};
use smithay::wayland::compositor::{CompositorToken, SubsurfaceRole, TraversalAction};
use smithay::wayland::compositor::roles::Role;
use smithay::wayland::output::{Mode, Output, PhysicalProperties};
use smithay::wayland::seat::{KeyboardHandle, PointerHandle, Seat};
use smithay::wayland::shm::init_shm_global;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::io::Error as IoError;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use wayland_server::{Display, EventLoopHandle};
use wayland_server::protocol::{wl_output, wl_pointer};
use wayland_server::sources::EventSource;
use xkbcommon::xkb::keysyms as xkb;

struct LibinputInputHandler {
    log: Logger,
    pointer: PointerHandle,
    keyboard: KeyboardHandle,
    window_map: Rc<RefCell<MyWindowMap>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    screen_size: (u32, u32),
    serial: u32,
    session: AutoSession,
    running: Arc<AtomicBool>,
}

impl LibinputInputHandler {
    fn next_serial(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }
}

impl InputHandler<LibinputInputBackend> for LibinputInputHandler {
    fn on_seat_created(&mut self, _evlh: &mut EventLoopHandle, _: &input::Seat) {
        /* we just create a single static one */
    }
    fn on_seat_destroyed(&mut self, _evlh: &mut EventLoopHandle, _: &input::Seat) {
        /* we just create a single static one */
    }
    fn on_seat_changed(&mut self, _evlh: &mut EventLoopHandle, _: &input::Seat) {
        /* we just create a single static one */
    }
    fn on_keyboard_key(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, evt: event::keyboard::KeyboardKeyEvent
    ) {
        let keycode = evt.key();
        let state = evt.state();
        debug!(self.log, "key"; "keycode" => keycode, "state" => format!("{:?}", state));
        let serial = self.next_serial();

        // we cannot borrow `self` into the closure, because we need self.keyboard.
        // but rust does not borrow all fields separately, so we need to do that manually...
        let running = &self.running;
        let mut session = &mut self.session;
        let log = &self.log;
        self.keyboard
            .input(keycode, state, serial, move |modifiers, keysym| {
                debug!(log, "keysym"; "state" => format!("{:?}", state), "mods" => format!("{:?}", modifiers), "keysym" => xkbcommon::xkb::keysym_get_name(keysym));
                if modifiers.ctrl && modifiers.alt && keysym == xkb::KEY_BackSpace
                    && state == KeyState::Pressed
                {
                    info!(log, "Stopping example using Ctrl+Alt+Backspace");
                    running.store(false, Ordering::SeqCst);
                    false
                } else if modifiers.logo && keysym == xkb::KEY_q {
                    info!(log, "Stopping example using Logo+Q");
                    running.store(false, Ordering::SeqCst);
                    false
                } else if modifiers.ctrl && modifiers.alt && keysym >= xkb::KEY_XF86Switch_VT_1
                    && keysym <= xkb::KEY_XF86Switch_VT_12
                    && state == KeyState::Pressed
                {
                    let vt = (keysym - xkb::KEY_XF86Switch_VT_1 + 1) as i32;
                    info!(log, "Trying to switch to vt {}", vt);
                    if let Err(err) = session.change_vt(vt) {
                        error!(log, "Error switching to vt {}: {}", vt, err);
                    }
                    false
                } else if modifiers.logo && keysym == xkb::KEY_Return && state == KeyState::Pressed {
                    info!(log, "Launching terminal");
                    let _ = Command::new("weston-terminal").spawn();
                    false
                } else {
                    true
                }
            });
    }
    fn on_pointer_move(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, evt: event::pointer::PointerMotionEvent
    ) {
        let (x, y) = (evt.dx(), evt.dy());
        let serial = self.next_serial();
        let mut location = self.pointer_location.borrow_mut();
        location.0 += x;
        location.1 += y;
        let under = self.window_map
            .borrow()
            .get_surface_under((location.0, location.1));
        self.pointer.motion(
            under.as_ref().map(|&(ref s, (x, y))| (s, x, y)),
            serial,
            evt.time(),
        );
    }
    fn on_pointer_move_absolute(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat,
        evt: event::pointer::PointerMotionAbsoluteEvent,
    ) {
        let (x, y) = (
            evt.absolute_x_transformed(self.screen_size.0),
            evt.absolute_y_transformed(self.screen_size.1),
        );
        *self.pointer_location.borrow_mut() = (x, y);
        let serial = self.next_serial();
        let under = self.window_map.borrow().get_surface_under((x, y));
        self.pointer.motion(
            under.as_ref().map(|&(ref s, (x, y))| (s, x, y)),
            serial,
            evt.time(),
        );
    }
    fn on_pointer_button(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, evt: event::pointer::PointerButtonEvent
    ) {
        let serial = self.next_serial();
        let button = evt.button();
        let state = match evt.state() {
            input::MouseButtonState::Pressed => {
                // change the keyboard focus
                let under = self.window_map
                    .borrow_mut()
                    .get_surface_and_bring_to_top(*self.pointer_location.borrow());
                self.keyboard
                    .set_focus(under.as_ref().map(|&(ref s, _)| s), serial);
                wl_pointer::ButtonState::Pressed
            }
            input::MouseButtonState::Released => wl_pointer::ButtonState::Released,
        };
        self.pointer.button(button, state, serial, evt.time());
    }
    fn on_pointer_axis(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, evt: LibinputPointerAxisEvent
    ) {
        let axis = match evt.axis() {
            input::Axis::Vertical => wayland_server::protocol::wl_pointer::Axis::VerticalScroll,
            input::Axis::Horizontal => wayland_server::protocol::wl_pointer::Axis::HorizontalScroll,
        };
        self.pointer.axis(axis, evt.amount(), evt.time());
    }
    fn on_touch_down(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, _: event::touch::TouchDownEvent
    ) {
        /* not done in this example */
    }
    fn on_touch_motion(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, _: event::touch::TouchMotionEvent
    ) {
        /* not done in this example */
    }
    fn on_touch_up(&mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, _: event::touch::TouchUpEvent) {
        /* not done in this example */
    }
    fn on_touch_cancel(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, _: event::touch::TouchCancelEvent
    ) {
        /* not done in this example */
    }
    fn on_touch_frame(
        &mut self, _evlh: &mut EventLoopHandle, _: &input::Seat, _: event::touch::TouchFrameEvent
    ) {
        /* not done in this example */
    }
    fn on_input_config_changed(&mut self, _evlh: &mut EventLoopHandle, _: &mut [LibinputDevice]) {
        /* not done in this example */
    }
}

fn main() {
    let active_egl_context = Rc::new(RefCell::new(None));

    // A logger facility, here we use the terminal for this example
    let log = Logger::root(
        slog_term::FullFormat::new(slog_term::PlainSyncDecorator::new(std::io::stdout()))
            .build()
            .fuse(),
        o!(),
    );

    // Initialize the wayland server
    let (mut display, mut event_loop) = wayland_server::create_display();

    /*
     * Add a listening socket
     */
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    println!("Listening on socket: {}", name);
    env::set_var("WAYLAND_DISPLAY", name);
    let display = Rc::new(display);

    /*
     * Initialize the compositor
     */
    init_shm_global(&mut event_loop, vec![], log.clone());

    let (compositor_token, _shell_state_token, window_map) =
        init_shell(&mut event_loop, log.clone(), active_egl_context.clone());

    /*
     * Initialize session
     */
    let (session, mut notifier) = AutoSession::new(log.clone()).unwrap();

    let running = Arc::new(AtomicBool::new(true));

    let pointer_location = Rc::new(RefCell::new((0.0, 0.0)));

    /*
     * Initialize the udev backend
     */
    let context = udev::Context::new().unwrap();
    let seat = session.seat();

    let primary_gpu = primary_gpu(&context, &seat).unwrap_or_default();

    let bytes = include_bytes!("resources/cursor2.rgba");
    let mut udev_backend = UdevBackend::new(
        &mut event_loop,
        &context,
        session.clone(),
        UdevHandlerImpl {
            compositor_token,
            active_egl_context,
            backends: HashMap::new(),
            display: display.clone(),
            primary_gpu,
            window_map: window_map.clone(),
            pointer_location: pointer_location.clone(),
            pointer_image: ImageBuffer::from_raw(64, 64, bytes.to_vec()).unwrap(),
            logger: log.clone(),
        },
        log.clone(),
    ).unwrap();

    let udev_session_id = notifier.register(&mut udev_backend);

    let (seat_token, _) = Seat::new(&mut event_loop, session.seat().into(), log.clone());

    let pointer = event_loop.state().get_mut(&seat_token).add_pointer();
    let keyboard = event_loop
        .state()
        .get_mut(&seat_token)
        .add_keyboard("", "", "", None, 1000, 500)
        .expect("Failed to initialize the keyboard");

    let (output_token, _output_global) = Output::new(
        &mut event_loop,
        "Drm".into(),
        PhysicalProperties {
            width: 0,
            height: 0,
            subpixel: wl_output::Subpixel::Unknown,
            maker: "Smithay".into(),
            model: "Generic DRM".into(),
        },
        log.clone(),
    );

    let (w, h) = (1920, 1080); // Hardcode full-hd res
    event_loop
        .state()
        .get_mut(&output_token)
        .change_current_state(
            Some(Mode {
                width: w as i32,
                height: h as i32,
                refresh: 60_000,
            }),
            None,
            None,
        );
    event_loop
        .state()
        .get_mut(&output_token)
        .set_preferred(Mode {
            width: w as i32,
            height: h as i32,
            refresh: 60_000,
        });

    /*
     * Initialize libinput backend
     */
    let mut libinput_context =
        Libinput::new_from_udev::<LibinputSessionInterface<AutoSession>>(session.clone().into(), &context);
    let libinput_session_id = notifier.register(&mut libinput_context);
    libinput_context.udev_assign_seat(&seat).unwrap();
    let mut libinput_backend = LibinputInputBackend::new(libinput_context, log.clone());
    libinput_backend.set_handler(
        &mut event_loop,
        LibinputInputHandler {
            log: log.clone(),
            pointer,
            keyboard,
            window_map: window_map.clone(),
            pointer_location,
            screen_size: (w, h),
            serial: 0,
            session: session,
            running: running.clone(),
        },
    );
    let libinput_event_source = libinput_bind(libinput_backend, &mut event_loop)
        .map_err(|(err, _)| err)
        .unwrap();

    let session_event_source = auto_session_bind(notifier, &mut event_loop)
        .map_err(|(err, _)| err)
        .unwrap();
    let udev_event_source = udev_backend_bind(&mut event_loop, udev_backend)
        .map_err(|(err, _)| err)
        .unwrap();

    while running.load(Ordering::SeqCst) {
        if let Err(_) = event_loop.dispatch(Some(16)) {
            running.store(false, Ordering::SeqCst);
        } else {
            display.flush_clients();
            window_map.borrow_mut().refresh();
        }
    }

    println!("Bye Bye");

    let mut notifier = session_event_source.unbind();
    notifier.unregister(udev_session_id);
    notifier.unregister(libinput_session_id);

    libinput_event_source.remove();

    // destroy the udev backend freeing the drm devices
    udev_event_source.remove().close(&mut event_loop)
}

struct UdevHandlerImpl {
    compositor_token: CompositorToken<SurfaceData, Roles, Rc<RefCell<Option<EGLDisplay>>>>,
    active_egl_context: Rc<RefCell<Option<EGLDisplay>>>,
    backends: HashMap<u64, Rc<RefCell<HashMap<crtc::Handle, GliumDrawer<DrmBackend<SessionFdDrmDevice>>>>>>,
    display: Rc<Display>,
    primary_gpu: Option<PathBuf>,
    window_map: Rc<RefCell<MyWindowMap>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    pointer_image: ImageBuffer<Rgba<u8>, Vec<u8>>,
    logger: ::slog::Logger,
}

impl UdevHandlerImpl {
    pub fn scan_connectors(
        &self, device: &mut DrmDevice<SessionFdDrmDevice>
    ) -> HashMap<crtc::Handle, GliumDrawer<DrmBackend<SessionFdDrmDevice>>> {
        // Get a set of all modesetting resource handles (excluding planes):
        let res_handles = device.resource_handles().unwrap();

        // Use first connected connector
        let connector_infos: Vec<ConnectorInfo> = res_handles
            .connectors()
            .iter()
            .map(|conn| ConnectorInfo::load_from_device(device, *conn).unwrap())
            .filter(|conn| conn.connection_state() == ConnectorState::Connected)
            .inspect(|conn| info!(self.logger, "Connected: {:?}", conn.connector_type()))
            .collect();

        let mut backends = HashMap::new();

        // very naive way of finding good crtc/encoder/connector combinations. This problem is np-complete
        for connector_info in connector_infos {
            let encoder_infos = connector_info
                .encoders()
                .iter()
                .flat_map(|encoder_handle| EncoderInfo::load_from_device(device, *encoder_handle))
                .collect::<Vec<EncoderInfo>>();
            for encoder_info in encoder_infos {
                for crtc in res_handles.filter_crtcs(encoder_info.possible_crtcs()) {
                    if !backends.contains_key(&crtc) {
                        let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)
                                                              // create a backend
                        let renderer = GliumDrawer::from(
                            device
                                .create_backend(crtc, mode, vec![connector_info.handle()])
                                .unwrap(),
                        );

                        // create cursor
                        renderer
                            .set_cursor_representation(&self.pointer_image, (2, 2))
                            .unwrap();

                        // render first frame
                        {
                            let mut frame = renderer.draw();
                            frame.clear_color(0.8, 0.8, 0.9, 1.0);
                            frame.finish().unwrap();
                        }

                        backends.insert(crtc, renderer);
                        break;
                    }
                }
            }
        }

        backends
    }
}

impl UdevHandler<DrmHandlerImpl> for UdevHandlerImpl {
    fn device_added(
        &mut self, _evlh: &mut EventLoopHandle, device: &mut DrmDevice<SessionFdDrmDevice>
    ) -> Option<DrmHandlerImpl> {
        // init hardware acceleration on the primary gpu.
        if device.dev_path().and_then(|path| path.canonicalize().ok()) == self.primary_gpu {
            *self.active_egl_context.borrow_mut() = device.bind_wl_display(&*self.display).ok();
        }

        let backends = Rc::new(RefCell::new(self.scan_connectors(device)));
        self.backends.insert(device.device_id(), backends.clone());

        Some(DrmHandlerImpl {
            compositor_token: self.compositor_token.clone(),
            backends,
            window_map: self.window_map.clone(),
            pointer_location: self.pointer_location.clone(),
            logger: self.logger.clone(),
        })
    }

    fn device_changed(&mut self, _evlh: &mut EventLoopHandle, device: &mut DrmDevice<SessionFdDrmDevice>) {
        //quick and dirt, just re-init all backends
        let backends = self.backends.get(&device.device_id()).unwrap();
        *backends.borrow_mut() = self.scan_connectors(device);
    }

    fn device_removed(&mut self, _evlh: &mut EventLoopHandle, device: &mut DrmDevice<SessionFdDrmDevice>) {
        // drop the backends on this side
        self.backends.remove(&device.device_id());

        // don't use hardware acceleration anymore, if this was the primary gpu
        if device.dev_path().and_then(|path| path.canonicalize().ok()) == self.primary_gpu {
            *self.active_egl_context.borrow_mut() = None;
        }
    }

    fn error(&mut self, _evlh: &mut EventLoopHandle, error: IoError) {
        error!(self.logger, "{:?}", error);
    }
}

pub struct DrmHandlerImpl {
    compositor_token: CompositorToken<SurfaceData, Roles, Rc<RefCell<Option<EGLDisplay>>>>,
    backends: Rc<RefCell<HashMap<crtc::Handle, GliumDrawer<DrmBackend<SessionFdDrmDevice>>>>>,
    window_map: Rc<RefCell<MyWindowMap>>,
    pointer_location: Rc<RefCell<(f64, f64)>>,
    logger: ::slog::Logger,
}

impl DrmHandler<SessionFdDrmDevice> for DrmHandlerImpl {
    fn ready(
        &mut self, _evlh: &mut EventLoopHandle, _device: &mut DrmDevice<SessionFdDrmDevice>,
        crtc: crtc::Handle, _frame: u32, _duration: Duration,
    ) {
        if let Some(drawer) = self.backends.borrow().get(&crtc) {
            {
                let (x, y) = *self.pointer_location.borrow();
                let _ = drawer.set_cursor_position(x.trunc().abs() as u32, y.trunc().abs() as u32);
            }
            let mut frame = drawer.draw();
            frame.clear_color(0.8, 0.8, 0.9, 1.0);
            // redraw the frame, in a simple but inneficient way
            {
                let screen_dimensions = drawer.get_framebuffer_dimensions();
                self.window_map.borrow().with_windows_from_bottom_to_top(
                    |toplevel_surface, initial_place| {
                        if let Some(wl_surface) = toplevel_surface.get_surface() {
                            // this surface is a root of a subsurface tree that needs to be drawn
                            self.compositor_token
                                .with_surface_tree_upward(
                                    wl_surface,
                                    initial_place,
                                    |_surface, attributes, role, &(mut x, mut y)| {
                                        // there is actually something to draw !
                                        if attributes.user_data.texture.is_none() {
                                            let mut remove = false;
                                            match attributes.user_data.buffer {
                                                Some(Buffer::Egl { ref images }) => {
                                                    match images.format {
                                                        Format::RGB | Format::RGBA => {
                                                            attributes.user_data.texture =
                                                                drawer.texture_from_egl(&images);
                                                        }
                                                        _ => {
                                                            // we don't handle the more complex formats here.
                                                            attributes.user_data.texture = None;
                                                            remove = true;
                                                        }
                                                    };
                                                }
                                                Some(Buffer::Shm { ref data, ref size }) => {
                                                    attributes.user_data.texture =
                                                        Some(drawer.texture_from_mem(data, *size));
                                                }
                                                _ => {}
                                            }
                                            if remove {
                                                attributes.user_data.buffer = None;
                                            }
                                        }

                                        if let Some(ref texture) = attributes.user_data.texture {
                                            if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                                                x += subdata.x;
                                                y += subdata.y;
                                            }
                                            info!(self.logger, "Render window");
                                            drawer.render_texture(
                                                &mut frame,
                                                texture,
                                                match *attributes.user_data.buffer.as_ref().unwrap() {
                                                    Buffer::Egl { ref images } => images.y_inverted,
                                                    Buffer::Shm { .. } => false,
                                                },
                                                match *attributes.user_data.buffer.as_ref().unwrap() {
                                                    Buffer::Egl { ref images } => {
                                                        (images.width, images.height)
                                                    }
                                                    Buffer::Shm { ref size, .. } => *size,
                                                },
                                                (x, y),
                                                screen_dimensions,
                                                Blend::alpha_blending(),
                                            );
                                            TraversalAction::DoChildren((x, y))
                                        } else {
                                            // we are not display, so our children are neither
                                            TraversalAction::SkipChildren
                                        }
                                    },
                                )
                                .unwrap();
                        }
                    },
                );
            }
            if let Err(err) = frame.finish() {
                error!(self.logger, "Error during rendering: {:?}", err);
            }
        }
    }

    fn error(
        &mut self, _evlh: &mut EventLoopHandle, _device: &mut DrmDevice<SessionFdDrmDevice>, error: DrmError
    ) {
        error!(self.logger, "{:?}", error);
    }
}
