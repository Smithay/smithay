use std::{
    os::{
        raw::{c_char, c_int},
        unix::{
            net::UnixStream,
            prelude::{AsRawFd, IntoRawFd},
        },
    },
    thread::JoinHandle,
};

use smithay::reexports::calloop::channel::{channel, Sender};
use wayland_sys::{
    client::*,
    common::{wl_fixed_t, wl_fixed_to_double},
    ffi_dispatch,
};

use crate::{ffi_api::*, WlcsEvent};

macro_rules! container_of(
    ($ptr: expr, $container: ident, $field: ident) => {
        ($ptr as *mut u8).offset(-(memoffset::offset_of!($container, $field) as isize)) as *mut $container
    }
);

#[no_mangle]
pub static wlcs_server_integration: WlcsServerIntegration = WlcsServerIntegration {
    version: 1,
    create_server,
    destroy_server,
};

unsafe extern "C" fn create_server(_argc: c_int, _argv: *mut *const c_char) -> *mut WlcsDisplayServer {
    // block the SIGPIPE signal here, we are a cdylib so Rust does not do it for us
    match std::panic::catch_unwind(|| {
        use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
        sigaction(
            Signal::SIGPIPE,
            &SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty()),
        )
        .unwrap();
        let handle = Box::into_raw(Box::new(DisplayServerHandle::new()));
        &mut (*handle).wlcs_display_server
    }) {
        Ok(ptr) => ptr,
        Err(err) => {
            println!(
                "panic in create_server on ptr: {:p} (type {:?})",
                err.as_ref() as *const _,
                err.type_id()
            );
            std::ptr::null_mut()
        }
    }
}

unsafe extern "C" fn destroy_server(ptr: *mut WlcsDisplayServer) {
    match std::panic::catch_unwind(|| {
        let _server = Box::from_raw(container_of!(ptr, DisplayServerHandle, wlcs_display_server));
    }) {
        Ok(()) => {}
        Err(err) => {
            println!(
                "panic in destroy_server on ptr: {:p} (type {:?})",
                err.as_ref() as *const _,
                err.type_id()
            );
        }
    }
}

struct DisplayServerHandle {
    wlcs_display_server: WlcsDisplayServer,
    server: Option<(Sender<WlcsEvent>, JoinHandle<()>)>,
    next_device_id: u32,
}

impl DisplayServerHandle {
    fn new() -> DisplayServerHandle {
        DisplayServerHandle {
            wlcs_display_server: WlcsDisplayServer {
                version: 3,
                start: Self::start,
                stop: Self::stop,
                create_client_socket: Self::create_client_socket,
                position_window_absolute: Self::position_window_absolute,
                create_pointer: Self::create_pointer,
                create_touch: Self::create_touch,
                get_descriptor: Self::get_descriptor,
                start_on_this_thread: None,
            },
            server: None,
            next_device_id: 1,
        }
    }

    unsafe extern "C" fn start(ptr: *mut WlcsDisplayServer) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, DisplayServerHandle, wlcs_display_server);
            let (tx, rx) = channel();
            let join = crate::start_anvil(rx);
            me.server = Some((tx, join));
        }) {
            Ok(()) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_display_server::start on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn stop(ptr: *mut WlcsDisplayServer) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, DisplayServerHandle, wlcs_display_server);
            if let Some((sender, join)) = me.server.take() {
                let _ = sender.send(WlcsEvent::Exit);
                let _ = join.join();
            }
        }) {
            Ok(()) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_display_server::stop on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn create_client_socket(ptr: *mut WlcsDisplayServer) -> c_int {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, DisplayServerHandle, wlcs_display_server);
            if let Some((ref sender, _)) = me.server {
                if let Ok((client_side, server_side)) = UnixStream::pair() {
                    if sender
                        .send(WlcsEvent::NewClient {
                            stream: server_side,
                            client_id: client_side.as_raw_fd(),
                        })
                        .is_err()
                    {
                        return -1;
                    }
                    return client_side.into_raw_fd();
                }
            }
            -1
        }) {
            Ok(val) => val,
            Err(err) => {
                println!(
                    "panic in wlcs_display_server::create_client_socket on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
                -1
            }
        }
    }

    unsafe extern "C" fn position_window_absolute(
        ptr: *mut WlcsDisplayServer,
        display: *mut wl_display,
        surface: *mut wl_proxy,
        x: c_int,
        y: c_int,
    ) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, DisplayServerHandle, wlcs_display_server);
            let client_id = ffi_dispatch!(WAYLAND_CLIENT_HANDLE, wl_display_get_fd, display);
            let surface_id = ffi_dispatch!(WAYLAND_CLIENT_HANDLE, wl_proxy_get_id, surface);
            if let Some((ref sender, _)) = me.server {
                let _ = sender.send(WlcsEvent::PositionWindow {
                    client_id,
                    surface_id,
                    location: (x, y).into(),
                });
            }
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_display_server::position_window_absolute on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn create_pointer(ptr: *mut WlcsDisplayServer) -> *mut WlcsPointer {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, DisplayServerHandle, wlcs_display_server);
            if let Some((ref sender, _)) = me.server {
                let pointer = Box::into_raw(Box::new(PointerHandle::new(me.next_device_id, sender.clone())));
                me.next_device_id += 1;
                &mut (*pointer).wlcs_pointer
            } else {
                std::ptr::null_mut()
            }
        }) {
            Ok(ptr) => ptr,
            Err(err) => {
                println!(
                    "panic in wlcs_display_server::create_pointer on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
                std::ptr::null_mut()
            }
        }
    }

    unsafe extern "C" fn create_touch(ptr: *mut WlcsDisplayServer) -> *mut WlcsTouch {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, DisplayServerHandle, wlcs_display_server);
            if let Some((ref sender, _)) = me.server {
                let pointer = Box::into_raw(Box::new(TouchHandle::new(me.next_device_id, sender.clone())));
                me.next_device_id += 1;
                &mut (*pointer).wlcs_touch
            } else {
                std::ptr::null_mut()
            }
        }) {
            Ok(ptr) => ptr,
            Err(err) => {
                println!(
                    "panic in wlcs_display_server::create_touch on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
                std::ptr::null_mut()
            }
        }
    }

    unsafe extern "C" fn get_descriptor(_: *const WlcsDisplayServer) -> *const WlcsIntegrationDescriptor {
        &crate::DESCRIPTOR
    }
}

struct PointerHandle {
    wlcs_pointer: WlcsPointer,
    device_id: u32,
    sender: Sender<WlcsEvent>,
}

impl PointerHandle {
    fn new(device_id: u32, sender: Sender<WlcsEvent>) -> PointerHandle {
        PointerHandle {
            wlcs_pointer: WlcsPointer {
                version: 1,
                move_absolute: Self::move_absolute,
                move_relative: Self::move_relative,
                button_down: Self::button_down,
                button_up: Self::button_up,
                destroy: Self::destroy,
            },
            device_id,
            sender,
        }
    }

    unsafe extern "C" fn move_absolute(ptr: *mut WlcsPointer, x: wl_fixed_t, y: wl_fixed_t) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, PointerHandle, wlcs_pointer);
            let _ = me.sender.send(WlcsEvent::PointerMoveAbsolute {
                device_id: me.device_id,
                location: (wl_fixed_to_double(x), wl_fixed_to_double(y)).into(),
            });
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_pointer::move_absolute on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn move_relative(ptr: *mut WlcsPointer, x: wl_fixed_t, y: wl_fixed_t) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, PointerHandle, wlcs_pointer);
            let _ = me.sender.send(WlcsEvent::PointerMoveRelative {
                device_id: me.device_id,
                delta: (wl_fixed_to_double(x), wl_fixed_to_double(y)).into(),
            });
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_pointer::move_relative on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn button_up(ptr: *mut WlcsPointer, button_id: i32) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, PointerHandle, wlcs_pointer);
            let _ = me.sender.send(WlcsEvent::PointerButtonUp {
                device_id: me.device_id,
                button_id,
            });
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_pointer::button_up on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn button_down(ptr: *mut WlcsPointer, button_id: i32) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, PointerHandle, wlcs_pointer);
            let _ = me.sender.send(WlcsEvent::PointerButtonDown {
                device_id: me.device_id,
                button_id,
            });
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_pointer::button_down on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn destroy(ptr: *mut WlcsPointer) {
        match std::panic::catch_unwind(|| {
            let _me = Box::from_raw(container_of!(ptr, PointerHandle, wlcs_pointer));
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_pointer::destroy on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }
}

struct TouchHandle {
    wlcs_touch: WlcsTouch,
    device_id: u32,
    sender: Sender<WlcsEvent>,
}

impl TouchHandle {
    fn new(device_id: u32, sender: Sender<WlcsEvent>) -> TouchHandle {
        TouchHandle {
            wlcs_touch: WlcsTouch {
                version: 1,
                touch_down: Self::touch_down,
                touch_move: Self::touch_move,
                touch_up: Self::touch_up,
                destroy: Self::destroy,
            },
            device_id,
            sender,
        }
    }

    unsafe extern "C" fn touch_down(ptr: *mut WlcsTouch, x: wl_fixed_t, y: wl_fixed_t) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, TouchHandle, wlcs_touch);
            let _ = me.sender.send(WlcsEvent::TouchDown {
                device_id: me.device_id,
                location: (wl_fixed_to_double(x), wl_fixed_to_double(y)).into(),
            });
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_touch::touch_down on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn touch_move(ptr: *mut WlcsTouch, x: wl_fixed_t, y: wl_fixed_t) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, TouchHandle, wlcs_touch);
            let _ = me.sender.send(WlcsEvent::TouchMove {
                device_id: me.device_id,
                location: (wl_fixed_to_double(x), wl_fixed_to_double(y)).into(),
            });
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_touch::touch_move on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn touch_up(ptr: *mut WlcsTouch) {
        match std::panic::catch_unwind(|| {
            let me = &mut *container_of!(ptr, TouchHandle, wlcs_touch);
            let _ = me.sender.send(WlcsEvent::TouchUp {
                device_id: me.device_id,
            });
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_touch::touch_up on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }

    unsafe extern "C" fn destroy(ptr: *mut WlcsTouch) {
        match std::panic::catch_unwind(|| {
            let _me = Box::from_raw(container_of!(ptr, TouchHandle, wlcs_touch));
        }) {
            Ok(_) => {}
            Err(err) => {
                println!(
                    "panic in wlcs_touch::destroy on ptr: {:p} (type {:?})",
                    err.as_ref() as *const _,
                    err.type_id()
                );
            }
        }
    }
}
