mod main_loop;

use std::{
    io::{Error, ErrorKind},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::atomic::{AtomicU32, Ordering},
    thread::JoinHandle,
};

use smithay::{
    reexports::calloop::{
        self,
        channel::{channel, Sender},
    },
    utils::{Logical, Point},
};

use wayland_sys::{
    client::{wl_display, wl_display_get_fd, wl_proxy, wl_proxy_get_id},
    common::{wl_fixed_t, wl_fixed_to_double},
    ffi_dispatch,
};
use wlcs::{
    extension_list,
    ffi_display_server_api::{WlcsExtensionDescriptor, WlcsIntegrationDescriptor, WlcsServerIntegration},
    ffi_wrappers::wlcs_server,
    wlcs_server_integration, Wlcs,
};

wlcs_server_integration!(AnvilDisplayServerHandle);

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

static DEVICE_ID: AtomicU32 = AtomicU32::new(0);

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

struct AnvilDisplayServerHandle {
    server: Option<(Sender<WlcsEvent>, JoinHandle<()>)>,
}

impl Wlcs for AnvilDisplayServerHandle {
    type Pointer = PointerHandle;
    type Touch = TouchHandle;

    fn new() -> Self {
        AnvilDisplayServerHandle { server: None }
    }

    fn start(&mut self) {
        let (tx, rx) = channel();
        let join = crate::start_anvil(rx);
        self.server = Some((tx, join));
    }

    fn stop(&mut self) {
        if let Some((sender, join)) = self.server.take() {
            let _ = sender.send(WlcsEvent::Exit);
            let _ = join.join();
        }
    }

    fn create_client_socket(&self) -> std::io::Result<OwnedFd> {
        if let Some((ref sender, _)) = self.server {
            if let Ok((client_side, server_side)) = UnixStream::pair() {
                if let Err(e) = sender.send(WlcsEvent::NewClient {
                    stream: server_side,
                    client_id: client_side.as_raw_fd(),
                }) {
                    return Err(Error::new(ErrorKind::ConnectionReset, e));
                }
                return Ok(client_side.into());
            }
        }
        Err(Error::from(ErrorKind::NotFound))
    }

    fn position_window_absolute(&self, display: *mut wl_display, surface: *mut wl_proxy, x: i32, y: i32) {
        let client_id = unsafe { ffi_dispatch!(WAYLAND_CLIENT_HANDLE, wl_display_get_fd, display) };
        let surface_id = unsafe { ffi_dispatch!(WAYLAND_CLIENT_HANDLE, wl_proxy_get_id, surface) };
        if let Some((ref sender, _)) = self.server {
            let _ = sender.send(WlcsEvent::PositionWindow {
                client_id,
                surface_id,
                location: (x, y).into(),
            });
        }
    }

    fn create_pointer(&mut self) -> Option<Self::Pointer> {
        let Some(ref server) = self.server else {
            return None;
        };
        Some(PointerHandle {
            device_id: DEVICE_ID.fetch_add(1, Ordering::Relaxed),
            sender: server.0.clone(),
        })
    }

    fn create_touch(&mut self) -> Option<Self::Touch> {
        let Some(ref server) = self.server else {
            return None;
        };
        Some(TouchHandle {
            device_id: DEVICE_ID.fetch_add(1, Ordering::Relaxed),
            sender: server.0.clone(),
        })
    }

    fn get_descriptor(&self) -> &WlcsIntegrationDescriptor {
        &crate::DESCRIPTOR
    }
}

struct PointerHandle {
    device_id: u32,
    sender: Sender<WlcsEvent>,
}

impl wlcs::Pointer for PointerHandle {
    fn move_absolute(&mut self, x: wl_fixed_t, y: wl_fixed_t) {
        let _ = self.sender.send(WlcsEvent::PointerMoveAbsolute {
            device_id: self.device_id,
            location: (wl_fixed_to_double(x), wl_fixed_to_double(y)).into(),
        });
    }

    fn move_relative(&mut self, dx: wl_fixed_t, dy: wl_fixed_t) {
        let _ = self.sender.send(WlcsEvent::PointerMoveRelative {
            device_id: self.device_id,
            delta: (wl_fixed_to_double(dx), wl_fixed_to_double(dy)).into(),
        });
    }

    fn button_up(&mut self, button: i32) {
        let _ = self.sender.send(WlcsEvent::PointerButtonUp {
            device_id: self.device_id,
            button_id: button,
        });
    }

    fn button_down(&mut self, button: i32) {
        let _ = self.sender.send(WlcsEvent::PointerButtonDown {
            device_id: self.device_id,
            button_id: button,
        });
    }

    fn destroy(&mut self) {}
}

struct TouchHandle {
    device_id: u32,
    sender: Sender<WlcsEvent>,
}

impl wlcs::Touch for TouchHandle {
    fn touch_down(&mut self, x: wl_fixed_t, y: wl_fixed_t) {
        let _ = self.sender.send(WlcsEvent::TouchDown {
            device_id: self.device_id,
            location: (wl_fixed_to_double(x), wl_fixed_to_double(y)).into(),
        });
    }

    fn touch_move(&mut self, x: wl_fixed_t, y: wl_fixed_t) {
        let _ = self.sender.send(WlcsEvent::TouchMove {
            device_id: self.device_id,
            location: (wl_fixed_to_double(x), wl_fixed_to_double(y)).into(),
        });
    }

    fn touch_up(&mut self) {
        let _ = self.sender.send(WlcsEvent::TouchUp {
            device_id: self.device_id,
        });
    }

    fn destroy(&mut self) {}
}

fn start_anvil(channel: calloop::channel::Channel<WlcsEvent>) -> JoinHandle<()> {
    std::thread::spawn(move || main_loop::run(channel))
}
