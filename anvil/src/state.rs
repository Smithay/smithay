use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use smithay::{
    reexports::{
        calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction},
        wayland_server::{protocol::wl_surface::WlSurface, Display},
    },
    utils::{Logical, Point},
    wayland::{
        data_device::{default_action_chooser, init_data_device, set_data_device_focus, DataDeviceEvent},
        output::xdg::init_xdg_output_manager,
        seat::{CursorImageStatus, KeyboardHandle, PointerHandle, Seat, XkbConfig},
        shm::init_shm_global,
        tablet_manager::{init_tablet_manager_global, TabletSeatTrait},
        xdg_activation::{init_xdg_activation_global, XdgActivationEvent},
    },
};

#[cfg(feature = "xwayland")]
use smithay::xwayland::{XWayland, XWaylandEvent};

use crate::{output_map::OutputMap, shell::init_shell, window_map::WindowMap};

#[derive(Debug)]
pub struct AnvilState<BackendData> {
    pub backend_data: BackendData,
    pub socket_name: Option<String>,
    pub running: Arc<AtomicBool>,
    pub display: Rc<RefCell<Display>>,
    pub handle: LoopHandle<'static, AnvilState<BackendData>>,
    pub window_map: Rc<RefCell<crate::window_map::WindowMap>>,
    pub output_map: Rc<RefCell<crate::output_map::OutputMap>>,
    pub dnd_icon: Arc<Mutex<Option<WlSurface>>>,
    pub log: slog::Logger,
    // input-related fields
    pub pointer: PointerHandle,
    pub keyboard: KeyboardHandle,
    pub suppressed_keys: Vec<u32>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: Arc<Mutex<CursorImageStatus>>,
    pub seat_name: String,
    pub seat: Seat,
    pub start_time: std::time::Instant,
    // things we must keep alive
    #[cfg(feature = "xwayland")]
    pub xwayland: XWayland<AnvilState<BackendData>>,
}

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    pub fn init(
        display: Rc<RefCell<Display>>,
        handle: LoopHandle<'static, AnvilState<BackendData>>,
        backend_data: BackendData,
        log: slog::Logger,
        listen_on_socket: bool,
    ) -> AnvilState<BackendData> {
        // init the wayland connection
        handle
            .insert_source(
                Generic::from_fd(display.borrow().get_poll_fd(), Interest::READ, Mode::Level),
                move |_, _, state: &mut AnvilState<BackendData>| {
                    let display = state.display.clone();
                    let mut display = display.borrow_mut();
                    match display.dispatch(std::time::Duration::from_millis(0), state) {
                        Ok(_) => Ok(PostAction::Continue),
                        Err(e) => {
                            error!(state.log, "I/O error on the Wayland display: {}", e);
                            state.running.store(false, Ordering::SeqCst);
                            Err(e)
                        }
                    }
                },
            )
            .expect("Failed to init the wayland event source.");

        // Init a window map, to track the location of our windows
        let window_map = Rc::new(RefCell::new(WindowMap::default()));
        let output_map = Rc::new(RefCell::new(OutputMap::new(
            display.clone(),
            window_map.clone(),
            log.clone(),
        )));

        // Init the basic compositor globals

        init_shm_global(&mut (*display).borrow_mut(), vec![], log.clone());

        // Init the shell states
        init_shell::<BackendData>(display.clone(), log.clone());

        init_xdg_output_manager(&mut display.borrow_mut(), log.clone());
        init_xdg_activation_global(
            &mut display.borrow_mut(),
            |state, req, mut ddata| {
                let anvil_state = ddata.get::<AnvilState<BackendData>>().unwrap();
                match req {
                    XdgActivationEvent::RequestActivation {
                        token,
                        token_data,
                        surface,
                    } => {
                        if token_data.timestamp.elapsed().as_secs() < 10 {
                            // Just grant the wish
                            anvil_state.window_map.borrow_mut().bring_surface_to_top(&surface);
                        } else {
                            // Discard the request
                            state.lock().unwrap().remove_request(&token);
                        }
                    }
                    XdgActivationEvent::DestroyActivationRequest { .. } => {}
                }
            },
            log.clone(),
        );

        let socket_name = if listen_on_socket {
            let socket_name = display
                .borrow_mut()
                .add_socket_auto()
                .unwrap()
                .into_string()
                .unwrap();
            info!(log, "Listening on wayland socket"; "name" => socket_name.clone());
            ::std::env::set_var("WAYLAND_DISPLAY", &socket_name);
            Some(socket_name)
        } else {
            None
        };

        // init data device

        let dnd_icon = Arc::new(Mutex::new(None));

        let dnd_icon2 = dnd_icon.clone();
        init_data_device(
            &mut display.borrow_mut(),
            move |event| match event {
                DataDeviceEvent::DnDStarted { icon, .. } => {
                    *dnd_icon2.lock().unwrap() = icon;
                }
                DataDeviceEvent::DnDDropped => {
                    *dnd_icon2.lock().unwrap() = None;
                }
                _ => {}
            },
            default_action_chooser,
            log.clone(),
        );

        // init input
        let seat_name = backend_data.seat_name();

        let (mut seat, _) = Seat::new(&mut display.borrow_mut(), seat_name.clone(), log.clone());

        let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));

        let cursor_status2 = cursor_status.clone();
        let pointer = seat.add_pointer(move |new_status| {
            // TODO: hide winit system cursor when relevant
            *cursor_status2.lock().unwrap() = new_status
        });

        init_tablet_manager_global(&mut display.borrow_mut());

        let cursor_status3 = cursor_status.clone();
        seat.tablet_seat().on_cursor_surface(move |_tool, new_status| {
            // TODO: tablet tools should have their own cursors
            *cursor_status3.lock().unwrap() = new_status;
        });

        let keyboard = seat
            .add_keyboard(XkbConfig::default(), 200, 25, |seat, focus| {
                set_data_device_focus(seat, focus.and_then(|s| s.as_ref().client()))
            })
            .expect("Failed to initialize the keyboard");

        #[cfg(feature = "xwayland")]
        let xwayland = {
            let (xwayland, channel) = XWayland::new(handle.clone(), display.clone(), log.clone());
            let ret = handle.insert_source(channel, |event, _, anvil_state| match event {
                XWaylandEvent::Ready { connection, client } => anvil_state.xwayland_ready(connection, client),
                XWaylandEvent::Exited => anvil_state.xwayland_exited(),
            });
            if let Err(e) = ret {
                error!(
                    log,
                    "Failed to insert the XWaylandSource into the event loop: {}", e
                );
            }
            xwayland
        };

        AnvilState {
            backend_data,
            running: Arc::new(AtomicBool::new(true)),
            display,
            handle,
            window_map,
            output_map,
            dnd_icon,
            log,
            socket_name,
            pointer,
            keyboard,
            suppressed_keys: Vec::new(),
            cursor_status,
            pointer_location: (0.0, 0.0).into(),
            seat_name,
            seat,
            start_time: std::time::Instant::now(),
            #[cfg(feature = "xwayland")]
            xwayland,
        }
    }
}

pub trait Backend {
    fn seat_name(&self) -> String;
}
