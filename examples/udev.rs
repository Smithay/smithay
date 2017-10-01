extern crate drm;
#[macro_use]
extern crate glium;
extern crate rand;
extern crate libudev;
#[macro_use(define_roles)]
extern crate smithay;
extern crate wayland_server;

#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;

extern crate ctrlc;

mod helpers;

use drm::control::{Device as ControlDevice, ResourceInfo};
use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
use drm::control::encoder::Info as EncoderInfo;
use drm::control::crtc;
use drm::result::Error as DrmError;
use glium::Surface;
use helpers::{init_shell, GliumDrawer, MyWindowMap, Roles, SurfaceData};
use slog::{Drain, Logger};
use smithay::backend::drm::{DrmBackend, DrmDevice, DrmHandler};
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::udev::{UdevBackend, UdevHandler, udev_backend_bind};
use smithay::backend::session::SessionNotifier;
use smithay::backend::session::direct::{direct_session_bind, DirectSession};
use smithay::wayland::compositor::{CompositorToken, SubsurfaceRole, TraversalAction};
use smithay::wayland::compositor::roles::Role;
use smithay::wayland::shell::ShellState;
use smithay::wayland::shm::init_shm_global;
use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Error as IoError;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wayland_server::{StateToken, StateProxy};

fn main() {
    // A logger facility, here we use the terminal for this example
    let log = Logger::root(
        slog_term::FullFormat::new(slog_term::PlainSyncDecorator::new(std::io::stdout())).build().fuse(),
        o!(),
    );

    // Initialize the wayland server
    let (mut display, mut event_loop) = wayland_server::create_display();

    /*
     * Initialize the compositor
     */
    init_shm_global(&mut event_loop, vec![], log.clone());

    let (compositor_token, shell_state_token, window_map) = init_shell(&mut event_loop, log.clone());

    /*
     * Initialize session on the current tty
     */
    let (session, mut notifier) = DirectSession::new(None, log.clone()).unwrap();
    let session_token = event_loop.state().insert(session);

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    /*
     * Initialize the udev backend
     */
    let context = libudev::Context::new().unwrap();
    let udev
        = UdevBackend::new(&mut event_loop, &context, &session_token, UdevHandlerImpl {
            shell_state_token,
            compositor_token,
            window_map: window_map.clone(),
            logger: log.clone(),
        }, log.clone()).unwrap();

    let udev_token = event_loop.state().insert(udev);
    let udev_session_id = notifier.register(udev_token.clone());
    let session_event_source = direct_session_bind(notifier, &mut event_loop, log.clone()).unwrap();
    let udev_event_source = udev_backend_bind(&mut event_loop, udev_token).unwrap();
    /*
     * Add a listening socket:
     */
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    println!("Listening on socket: {}", name);

    while running.load(Ordering::SeqCst) {
        event_loop.dispatch(Some(16)).unwrap();
        display.flush_clients();
        window_map.borrow_mut().refresh();
    }

    let mut notifier = session_event_source.remove();
    notifier.unregister(udev_session_id);

    let udev_token = udev_event_source.remove();
    let udev = event_loop.state().remove(udev_token);
    udev.close(event_loop.state());

    event_loop.state().remove(session_token);
}

struct UdevHandlerImpl {
    shell_state_token: StateToken<ShellState<SurfaceData, Roles, (), ()>>,
    compositor_token: CompositorToken<SurfaceData, Roles, ()>,
    window_map: Rc<RefCell<MyWindowMap>>,
    logger: ::slog::Logger,
}

impl UdevHandlerImpl {
    pub fn scan_connectors<'a, S: Into<StateProxy<'a>>>(&self, state: S, device: &mut DrmDevice<GliumDrawer<DrmBackend>>) {
        // Get a set of all modesetting resource handles (excluding planes):
        let res_handles = device.resource_handles().unwrap();

        // Use first connected connector
        let connector_infos: Vec<ConnectorInfo> = res_handles
            .connectors()
            .iter()
            .map(|conn| {
                ConnectorInfo::load_from_device(device, *conn).unwrap()
            })
            .filter(|conn| conn.connection_state() == ConnectorState::Connected)
            .inspect(|conn| info!(self.logger, "Connected: {:?}", conn.connector_type()))
            .collect();

        let mut used_crtcs: HashSet<crtc::Handle> = HashSet::new();

        let mut state = state.into();

        // very naive way of finding good crtc/encoder/connector combinations. This problem is np-complete
        for connector_info in connector_infos {
            let encoder_infos = connector_info.encoders().iter().flat_map(|encoder_handle| EncoderInfo::load_from_device(device, *encoder_handle)).collect::<Vec<EncoderInfo>>();
            for encoder_info in encoder_infos {
                for crtc in res_handles.filter_crtcs(encoder_info.possible_crtcs()) {
                    if !used_crtcs.contains(&crtc) {
                        let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)
                        // create a backend
                        let renderer_token = device.create_backend(&mut state, crtc, mode, vec![connector_info.handle()]).unwrap();

                        // render first frame
                        {
                            let renderer = state.get_mut(renderer_token);
                            let mut frame = renderer.draw();
                            frame.clear_color(0.8, 0.8, 0.9, 1.0);
                            frame.finish().unwrap();
                        }

                        used_crtcs.insert(crtc);
                        break;
                    }
                }
            }
        }
    }
}

impl UdevHandler<GliumDrawer<DrmBackend>, DrmHandlerImpl> for UdevHandlerImpl {
    fn device_added<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, device: &mut DrmDevice<GliumDrawer<DrmBackend>>) -> Option<DrmHandlerImpl>
    {
        self.scan_connectors(state, device);

        Some(DrmHandlerImpl {
            shell_state_token: self.shell_state_token.clone(),
            compositor_token: self.compositor_token.clone(),
            window_map: self.window_map.clone(),
            logger: self.logger.clone(),
        })
    }

    fn device_changed<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, device: &StateToken<DrmDevice<GliumDrawer<DrmBackend>>>) {
        //quick and dirty
        let mut state = state.into();
        self.device_removed(&mut state, device);
        state.with_value(device, |state, device| self.scan_connectors(state, device));
    }

    fn device_removed<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, device: &StateToken<DrmDevice<GliumDrawer<DrmBackend>>>) {
        state.into().with_value(device, |state, device| {
            let crtcs = device.current_backends().into_iter().map(|backend| state.get(backend).crtc()).collect::<Vec<crtc::Handle>>();
            let mut state: StateProxy = state.into();
            for crtc in crtcs {
                device.destroy_backend(&mut state, &crtc);
            }
        });
    }

    fn error<'a, S: Into<StateProxy<'a>>>(&mut self, _state: S, error: IoError) {
        error!(self.logger, "{:?}", error);
    }
}

pub struct DrmHandlerImpl {
    shell_state_token: StateToken<ShellState<SurfaceData, Roles, (), ()>>,
    compositor_token: CompositorToken<SurfaceData, Roles, ()>,
    window_map: Rc<RefCell<MyWindowMap>>,
    logger: ::slog::Logger,
}

impl DrmHandler<GliumDrawer<DrmBackend>> for DrmHandlerImpl {
    fn ready<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, _device: &mut DrmDevice<GliumDrawer<DrmBackend>>,
             backend: &StateToken<GliumDrawer<DrmBackend>>, _crtc: crtc::Handle, _frame: u32, _duration: Duration) {
        let state = state.into();
        let drawer = state.get(backend);
        let mut frame = drawer.draw();
        frame.clear_color(0.8, 0.8, 0.9, 1.0);
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = drawer.get_framebuffer_dimensions();
            self.window_map
                .borrow()
                .with_windows_from_bottom_to_top(|toplevel_surface, initial_place| {
                    if let Some(wl_surface) = toplevel_surface.get_surface() {
                        // this surface is a root of a subsurface tree that needs to be drawn
                        self.compositor_token
                            .with_surface_tree_upward(
                                wl_surface,
                                initial_place,
                                |_surface, attributes, role, &(mut x, mut y)| {
                                    if let Some((ref contents, (w, h))) = attributes.user_data.buffer {
                                        // there is actually something to draw !
                                        if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                                            x += subdata.x;
                                            y += subdata.y;
                                        }
                                        drawer.render(
                                            &mut frame,
                                            contents,
                                            (w, h),
                                            (x, y),
                                            screen_dimensions,
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
                });
        }
        if let Err(err) = frame.finish() {
            error!(self.logger, "Error during rendering: {:?}", err);
        }
    }

    fn error<'a, S: Into<StateProxy<'a>>>(&mut self, _state: S, _device: &mut DrmDevice<GliumDrawer<DrmBackend>>,
             error: DrmError) {
        error!(self.logger, "{:?}", error);
    }
}
