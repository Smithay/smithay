use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use smithay::{
    backend::session::auto::AutoSession,
    reexports::{
        calloop::{
            generic::{Fd, Generic},
            Interest, LoopHandle, Mode, Source,
        },
        wayland_server::{protocol::wl_surface::WlSurface, Display},
    },
    wayland::{
        compositor::CompositorToken,
        data_device::{default_action_chooser, init_data_device, set_data_device_focus, DataDeviceEvent},
        seat::{CursorImageStatus, KeyboardHandle, PointerHandle, Seat, XkbConfig},
        shm::init_shm_global,
    },
};

#[cfg(feature = "udev")]
use smithay::backend::session::Session;

use crate::{buffer_utils::BufferUtils, shell::init_shell};

pub struct AnvilState {
    pub socket_name: String,
    pub running: Arc<AtomicBool>,
    pub display: Rc<RefCell<Display>>,
    pub handle: LoopHandle<AnvilState>,
    pub ctoken: CompositorToken<crate::shell::Roles>,
    pub window_map: Rc<RefCell<crate::window_map::WindowMap<crate::shell::Roles>>>,
    pub dnd_icon: Arc<Mutex<Option<WlSurface>>>,
    pub log: slog::Logger,
    // input-related fields
    pub pointer: PointerHandle,
    pub keyboard: KeyboardHandle,
    pub pointer_location: Rc<RefCell<(f64, f64)>>,
    pub cursor_status: Arc<Mutex<CursorImageStatus>>,
    pub screen_size: (u32, u32),
    pub seat_name: String,
    #[cfg(feature = "udev")]
    pub session: Option<AutoSession>,
    // things we must keep alive
    _wayland_event_source: Source<Generic<Fd>>,
}

impl AnvilState {
    pub fn init(
        display: Rc<RefCell<Display>>,
        handle: LoopHandle<AnvilState>,
        buffer_utils: BufferUtils,
        #[cfg(feature = "udev")] session: Option<AutoSession>,
        #[cfg(not(feature = "udev"))] _session: Option<()>,
        log: slog::Logger,
    ) -> AnvilState {
        // init the wayland connection
        let _wayland_event_source = handle
            .insert_source(
                Generic::from_fd(display.borrow().get_poll_fd(), Interest::Readable, Mode::Level),
                {
                    let display = display.clone();
                    let log = log.clone();
                    move |_, _, state: &mut AnvilState| {
                        let mut display = display.borrow_mut();
                        match display.dispatch(std::time::Duration::from_millis(0), state) {
                            Ok(_) => Ok(()),
                            Err(e) => {
                                error!(log, "I/O error on the Wayland display: {}", e);
                                state.running.store(false, Ordering::SeqCst);
                                Err(e)
                            }
                        }
                    }
                },
            )
            .expect("Failed to init the wayland event source.");

        // Init the basic compositor globals

        init_shm_global(&mut display.borrow_mut(), vec![], log.clone());

        let shell_handles = init_shell(&mut display.borrow_mut(), buffer_utils, log.clone());

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
        #[cfg(feature = "udev")]
        let seat_name = if let Some(ref session) = session {
            session.seat()
        } else {
            "anvil".into()
        };
        #[cfg(not(feature = "udev"))]
        let seat_name = "anvil".into();

        let (mut seat, _) = Seat::new(
            &mut display.borrow_mut(),
            seat_name.clone(),
            shell_handles.token,
            log.clone(),
        );

        let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));

        let cursor_status2 = cursor_status.clone();
        let pointer = seat.add_pointer(shell_handles.token.clone(), move |new_status| {
            // TODO: hide winit system cursor when relevant
            *cursor_status2.lock().unwrap() = new_status
        });

        let keyboard = seat
            .add_keyboard(XkbConfig::default(), 200, 25, |seat, focus| {
                set_data_device_focus(seat, focus.and_then(|s| s.as_ref().client()))
            })
            .expect("Failed to initialize the keyboard");

        AnvilState {
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
            screen_size: (1920, 1080),
            seat_name,
            #[cfg(feature = "udev")]
            session,
            _wayland_event_source,
        }
    }
}
