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
        calloop::{generic::Generic, Interest, LoopHandle, Mode},
        wayland_server::{protocol::wl_surface::WlSurface, Display},
    },
    wayland::{
        compositor::CompositorToken,
        data_device::{default_action_chooser, init_data_device, set_data_device_focus, DataDeviceEvent},
        seat::{CursorImageStatus, KeyboardHandle, PointerHandle, Seat, XkbConfig},
        shm::init_shm_global,
    },
};

#[cfg(feature = "egl")]
use smithay::backend::egl::display::EGLBufferReader;
#[cfg(feature = "xwayland")]
use smithay::xwayland::XWayland;

use crate::shell::init_shell;
#[cfg(feature = "xwayland")]
use crate::xwayland::XWm;

pub struct AnvilState<BackendData> {
    pub backend_data: BackendData,
    pub socket_name: String,
    pub running: Arc<AtomicBool>,
    pub display: Rc<RefCell<Display>>,
    pub handle: LoopHandle<'static, AnvilState<BackendData>>,
    pub ctoken: CompositorToken<crate::shell::Roles>,
    pub window_map: Rc<RefCell<crate::window_map::WindowMap<crate::shell::Roles>>>,
    pub dnd_icon: Arc<Mutex<Option<WlSurface>>>,
    pub log: slog::Logger,
    // input-related fields
    pub pointer: PointerHandle,
    pub keyboard: KeyboardHandle,
    pub pointer_location: Rc<RefCell<(f64, f64)>>,
    pub cursor_status: Arc<Mutex<CursorImageStatus>>,
    pub seat_name: String,
    pub start_time: std::time::Instant,
    #[cfg(feature = "egl")]
    pub egl_reader: Rc<RefCell<Option<EGLBufferReader>>>,
    // things we must keep alive
    #[cfg(feature = "xwayland")]
    _xwayland: XWayland<XWm<BackendData>>,
}

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    pub fn init(
        display: Rc<RefCell<Display>>,
        handle: LoopHandle<'static, AnvilState<BackendData>>,
        backend_data: BackendData,
        #[cfg(feature = "egl")] egl_reader: Rc<RefCell<Option<EGLBufferReader>>>,
        log: slog::Logger,
    ) -> AnvilState<BackendData> {
        // init the wayland connection
        handle
            .insert_source(
                Generic::from_fd(display.borrow().get_poll_fd(), Interest::READ, Mode::Level),
                move |_, _, state: &mut AnvilState<BackendData>| {
                    let display = state.display.clone();
                    let mut display = display.borrow_mut();
                    match display.dispatch(std::time::Duration::from_millis(0), state) {
                        Ok(_) => Ok(()),
                        Err(e) => {
                            error!(state.log, "I/O error on the Wayland display: {}", e);
                            state.running.store(false, Ordering::SeqCst);
                            Err(e)
                        }
                    }
                },
            )
            .expect("Failed to init the wayland event source.");

        // Init the basic compositor globals

        init_shm_global(&mut (*display).borrow_mut(), vec![], log.clone());

        #[cfg(feature = "egl")]
        let shell_handles =
            init_shell::<BackendData>(&mut display.borrow_mut(), egl_reader.clone(), log.clone());
        #[cfg(not(feature = "egl"))]
        let shell_handles = init_shell(&mut display.borrow_mut(), log.clone());

        let socket_name = display
            .borrow_mut()
            .add_socket_auto()
            .unwrap()
            .into_string()
            .unwrap();
        info!(log, "Listening on wayland socket"; "name" => socket_name.clone());
        ::std::env::set_var("WAYLAND_DISPLAY", &socket_name);

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
            shell_handles.token,
            log.clone(),
        );

        // init input
        let seat_name = backend_data.seat_name();

        let (mut seat, _) = Seat::new(
            &mut display.borrow_mut(),
            seat_name.clone(),
            shell_handles.token,
            log.clone(),
        );

        let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));

        let cursor_status2 = cursor_status.clone();
        let pointer = seat.add_pointer(shell_handles.token, move |new_status| {
            // TODO: hide winit system cursor when relevant
            *cursor_status2.lock().unwrap() = new_status
        });

        let keyboard = seat
            .add_keyboard(XkbConfig::default(), 200, 25, |seat, focus| {
                set_data_device_focus(seat, focus.and_then(|s| s.as_ref().client()))
            })
            .expect("Failed to initialize the keyboard");

        #[cfg(feature = "xwayland")]
        let _xwayland = {
            let xwm = XWm::new(
                handle.clone(),
                shell_handles.token,
                shell_handles.window_map.clone(),
                log.clone(),
            );
            XWayland::init(xwm, handle.clone(), display.clone(), &mut (), log.clone()).unwrap()
        };

        AnvilState {
            backend_data,
            running: Arc::new(AtomicBool::new(true)),
            display,
            handle,
            ctoken: shell_handles.token,
            window_map: shell_handles.window_map,
            dnd_icon,
            log,
            socket_name,
            pointer,
            keyboard,
            cursor_status,
            pointer_location: Rc::new(RefCell::new((0.0, 0.0))),
            seat_name,
            #[cfg(feature = "egl")]
            egl_reader,
            start_time: std::time::Instant::now(),
            #[cfg(feature = "xwayland")]
            _xwayland,
        }
    }
}

pub trait Backend {
    fn seat_name(&self) -> String;
}
