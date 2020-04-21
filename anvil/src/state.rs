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
        calloop::{
            generic::{Generic, SourceRawFd},
            mio::Interest,
            LoopHandle, Source,
        },
        wayland_server::{protocol::wl_surface::WlSurface, Display},
    },
    wayland::{
        compositor::CompositorToken,
        data_device::{default_action_chooser, init_data_device, DataDeviceEvent},
        shm::init_shm_global,
    },
};

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
    // things we must keep alive
    _wayland_event_source: Source<Generic<SourceRawFd>>,
}

impl AnvilState {
    pub fn init(
        display: Rc<RefCell<Display>>,
        handle: LoopHandle<AnvilState>,
        buffer_utils: BufferUtils,
        log: slog::Logger,
    ) -> AnvilState {
        // init the wayland connection
        let _wayland_event_source = handle
            .insert_source(
                {
                    let mut source = Generic::from_raw_fd(display.borrow().get_poll_fd());
                    source.set_interest(Interest::READABLE);
                    source
                },
                {
                    let display = display.clone();
                    let log = log.clone();
                    move |_, state: &mut AnvilState| {
                        let mut display = display.borrow_mut();
                        match display.dispatch(std::time::Duration::from_millis(0), state) {
                            Ok(_) => {}
                            Err(e) => {
                                error!(log, "I/O error on the Wayland display: {}", e);
                                state.running.store(false, Ordering::SeqCst);
                            }
                        }
                    }
                },
            )
            .expect("Failed to init the wayland event source.");

        init_shm_global(&mut display.borrow_mut(), vec![], log.clone());

        let (ctoken, _, _, window_map) = init_shell(&mut display.borrow_mut(), buffer_utils, log.clone());

        let socket_name = display
            .borrow_mut()
            .add_socket_auto()
            .unwrap()
            .into_string()
            .unwrap();
        info!(log, "Listening on wayland socket"; "name" => socket_name.clone());
        ::std::env::set_var("WAYLAND_DISPLAY", &socket_name);

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
            ctoken,
            log.clone(),
        );

        AnvilState {
            running: Arc::new(AtomicBool::new(true)),
            display,
            handle,
            ctoken,
            window_map,
            dnd_icon,
            log,
            socket_name,
            _wayland_event_source,
        }
    }
}
