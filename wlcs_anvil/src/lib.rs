mod ffi_api;
mod ffi_wrappers;
mod main_loop;
mod renderer;

use std::{os::unix::net::UnixStream, thread::JoinHandle};

use smithay::{
    reexports::calloop,
    utils::{Logical, Point},
};

use ffi_api::{WlcsExtensionDescriptor, WlcsIntegrationDescriptor};

macro_rules! extension_list {
    ($(($name: expr, $version: expr)),* $(,)?) => {
        &[$(
            WlcsExtensionDescriptor {
                name: concat!($name, "\0").as_ptr() as *const std::os::raw::c_char,
                version: $version
            }
        ),*]
    };
}

static SUPPORTED_EXTENSIONS: &[WlcsExtensionDescriptor] = extension_list!(
    ("wl_compositor", 4),
    ("wl_subcompositor", 1),
    ("wl_data_device_manager", 3),
    ("wl_seat", 7),
    ("wl_output", 4),
    ("xdg_wm_base", 3),
);

static DESCRIPTOR: WlcsIntegrationDescriptor = WlcsIntegrationDescriptor {
    version: 1,
    num_extensions: SUPPORTED_EXTENSIONS.len(),
    supported_extensions: SUPPORTED_EXTENSIONS.as_ptr(),
};

/// Event sent by WLCS to control the compositor
#[derive(Debug)]
pub enum WlcsEvent {
    /// Stop the running server
    Exit,
    /// Create a new client from given RawFd
    NewClient {
        stream: UnixStream,
        client_id: i32,
    },
    /// Position this window from the client associated with this Fd on the global space
    PositionWindow {
        client_id: i32,
        surface_id: u32,
        location: Point<i32, Logical>,
    },
    /* Pointer related events */
    /// A new pointer device is available
    NewPointer {
        device_id: u32,
    },
    /// Move the pointer in absolute coordinate space
    PointerMoveAbsolute {
        device_id: u32,
        location: Point<f64, Logical>,
    },
    /// Move the pointer in relative coordinate space
    PointerMoveRelative {
        device_id: u32,
        delta: Point<f64, Logical>,
    },
    /// Press a pointer button
    PointerButtonDown {
        device_id: u32,
        button_id: i32,
    },
    /// Release a pointer button
    PointerButtonUp {
        device_id: u32,
        button_id: i32,
    },
    /// A pointer device is removed
    PointerRemoved {
        device_id: u32,
    },
    /* Touch related events */
    /// A new touch device is available
    NewTouch {
        device_id: u32,
    },
    /// A touch point is down
    TouchDown {
        device_id: u32,
        location: Point<f64, Logical>,
    },
    /// A touch point moved
    TouchMove {
        device_id: u32,
        location: Point<f64, Logical>,
    },
    /// A touch point is up
    TouchUp {
        device_id: u32,
    },
    TouchRemoved {
        device_id: u32,
    },
}

fn start_anvil(channel: calloop::channel::Channel<WlcsEvent>) -> JoinHandle<()> {
    std::thread::spawn(move || main_loop::run(channel))
}
