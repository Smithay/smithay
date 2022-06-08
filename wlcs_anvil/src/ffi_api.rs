use std::os::raw::{c_char, c_int};

use wayland_sys::{client::*, common::wl_fixed_t, server as ssys};

#[repr(C)]
pub struct WlcsExtensionDescriptor {
    pub name: *const c_char,
    pub version: u32,
}

unsafe impl Sync for WlcsExtensionDescriptor {}
unsafe impl Send for WlcsExtensionDescriptor {}

#[repr(C)]
pub struct WlcsIntegrationDescriptor {
    pub version: u32,
    pub num_extensions: usize,
    pub supported_extensions: *const WlcsExtensionDescriptor,
}

unsafe impl Sync for WlcsIntegrationDescriptor {}
unsafe impl Send for WlcsIntegrationDescriptor {}

#[repr(C)]
pub struct WlcsDisplayServer {
    pub version: u32,
    pub start: unsafe extern "C" fn(*mut WlcsDisplayServer),
    pub stop: unsafe extern "C" fn(*mut WlcsDisplayServer),
    pub create_client_socket: unsafe extern "C" fn(*mut WlcsDisplayServer) -> c_int,
    pub position_window_absolute:
        unsafe extern "C" fn(*mut WlcsDisplayServer, *mut wl_display, *mut wl_proxy, c_int, c_int),
    pub create_pointer: unsafe extern "C" fn(*mut WlcsDisplayServer) -> *mut WlcsPointer,
    pub create_touch: unsafe extern "C" fn(*mut WlcsDisplayServer) -> *mut WlcsTouch,
    pub get_descriptor: unsafe extern "C" fn(*const WlcsDisplayServer) -> *const WlcsIntegrationDescriptor,
    pub start_on_this_thread: Option<unsafe extern "C" fn(*mut WlcsDisplayServer, *mut ssys::wl_event_loop)>,
}

#[repr(C)]
pub struct WlcsServerIntegration {
    pub version: u32,
    pub create_server: unsafe extern "C" fn(c_int, *mut *const c_char) -> *mut WlcsDisplayServer,
    pub destroy_server: unsafe extern "C" fn(*mut WlcsDisplayServer),
}

/*
 * WlcsPointer
 */

#[repr(C)]
pub struct WlcsPointer {
    pub version: u32,
    pub move_absolute: unsafe extern "C" fn(*mut WlcsPointer, wl_fixed_t, wl_fixed_t),
    pub move_relative: unsafe extern "C" fn(*mut WlcsPointer, wl_fixed_t, wl_fixed_t),
    pub button_up: unsafe extern "C" fn(*mut WlcsPointer, c_int),
    pub button_down: unsafe extern "C" fn(*mut WlcsPointer, c_int),
    pub destroy: unsafe extern "C" fn(*mut WlcsPointer),
}

/*
 * WlcsTouch
 */

#[repr(C)]
pub struct WlcsTouch {
    pub version: u32,
    pub touch_down: unsafe extern "C" fn(*mut WlcsTouch, wl_fixed_t, wl_fixed_t),
    pub touch_move: unsafe extern "C" fn(*mut WlcsTouch, wl_fixed_t, wl_fixed_t),
    pub touch_up: unsafe extern "C" fn(*mut WlcsTouch),
    pub destroy: unsafe extern "C" fn(*mut WlcsTouch),
}
