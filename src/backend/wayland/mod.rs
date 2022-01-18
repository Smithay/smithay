#![allow(missing_docs)] // Remove when more complete

//! Implementation of the backend types using Wayland.
//!
//! This backend provides the appropriate backend implementations to run a Wayland compositor as a Wayland
//! client.

// TODO: What needs to be done:
// - Hiding the cursor within the extent of the window (pending on SCTK).
// - Keyboard implementation (pending on repeating source).
// - Client side decorations (when there is no xdg-decoration, pending on SCTK).
//
// TODO: What could be done in the future:
// - Presentation using WlShm. This would make the Wayland backend capable of software rendering (pending on SCTK pools).
// - Touch (implemented in SCTK)
// - Tablet (Could useful in SCTK)
// - More precise presentation (explicit synchronization).
// - Input timestamps (for more precise timing info)

mod data;
mod dmabuf;
mod input;
mod protocol;

pub mod window;

use std::{
    io,
    os::unix::prelude::RawFd,
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc, Mutex},
};

use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use gbm::DeviceDestroyedError;
use nix::{
    fcntl::{self, OFlag},
    sys::stat::{major, minor, Mode},
};
use sctk::{
    compositor::CompositorState,
    event_loop::WaylandSource,
    output::OutputState,
    reexports::client::{
        backend::{self, InvalidId},
        ConnectError, Connection, DispatchError, QueueHandle,
    },
    registry::RegistryState,
    seat::SeatState,
    shell::xdg::{window::XdgWindowState, XdgShellState},
};
use smithay_client_toolkit::reexports::calloop::{
    self, EventSource, Poll, PostAction, Readiness, Token, TokenFactory,
};

use crate::{
    backend::{
        drm::{node::dev_path, NodeType},
        input::InputEvent,
        wayland::{
            data::{Protocols, WindowFocus},
            dmabuf::{DmabufState, MainDevice},
        },
    },
    utils::{Logical, Size},
};

use self::{
    data::WaylandBackendData,
    window::{Data, Window, WindowId},
};

pub use self::input::*;

use super::allocator::{gbm::GbmConvertError, Swapchain};

#[derive(Debug, thiserror::Error)]
pub enum WaylandError {
    /// Failed to connect to a Wayland compositor.
    #[error(transparent)]
    Connect(#[from] ConnectError),

    /// An invalid object was operated on.
    #[error(transparent)]
    InvalidId(#[from] InvalidId),

    /// Error while dispatching events.
    #[error(transparent)]
    Dispatch(#[from] DispatchError),

    /// Error when using the wayland connection.
    #[error(transparent)]
    Connection(#[from] backend::WaylandError),

    /// Error when allocating buffers.
    #[error(transparent)]
    AllocateBuffers(#[from] AllocateBuffersError),
}

/// Buffer allocation error.
#[derive(Debug, thiserror::Error)]
pub enum AllocateBuffersError {
    /// Failed to open the DRM device.
    #[error(transparent)]
    Open(#[from] io::Error),

    /// The device used for buffer allocation has been destroyed.
    #[error(transparent)]
    DeviceDestroyed(#[from] DeviceDestroyedError),

    /// Exporting a dmabuf for presentation has failed.
    #[error(transparent)]
    ExportDmabuf(#[from] GbmConvertError),

    /// There are no free slots to allocate more buffers.
    #[error("no free slots")]
    NoFreeSlots,

    /// All buffers are in use by the Wayland compositor.
    ///
    /// If you receive this error, you should wait until you receive a frame event to present again.
    #[error("the buffer being submitted is currently being used by the compositor")]
    InUse,

    /// The compositor does not support dmabuf backed wl_buffers.
    #[error("the compositor does not support importing a dmabuf backed wl_buffers")]
    Unsupported,
}

/// A connection to a Wayland compositor.
///
/// This is an event source that handles communication with the compositor.
#[derive(Debug)]
pub struct WaylandBackend {
    source: WaylandSource<WaylandBackendData>,
    backend_data: Arc<Mutex<WaylandBackendData>>,
}

impl WaylandBackend {
    pub fn new<L>(logger: L) -> Result<(WaylandBackend, WaylandHandle), WaylandError>
    where
        L: Into<Option<slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "backend_wayland"));

        let conn = Connection::connect_to_env()?;
        slog::info!(logger, "Connected to Wayland compositor");

        let mut event_queue = conn.new_event_queue();
        let queue_handle = event_queue.handle();

        let mut backend_data = WaylandBackendData {
            protocols: Protocols {
                registry_state: RegistryState::new(&conn, &queue_handle),
                output_state: OutputState::new(),
                compositor_state: CompositorState::new(),
                seat_state: SeatState::new(),
                xdg_shell_state: XdgShellState::new(),
                xdg_window_state: XdgWindowState::new(),
                dmabuf_state: DmabufState::new(),
            },

            wl_seat: None,
            wl_pointer: None,
            pointer: None,
            wl_keyboard: None,
            keyboard: None,

            id_counter: AtomicUsize::new(0),
            windows: vec![],
            focus: WindowFocus {
                keyboard: None,
                pointer: None,
            },
            allocator: None,
            recorded: vec![],
            logger: logger.clone(),
        };

        // Send requests to the server and block until we receive events from the server.
        while !backend_data.protocols.registry_state.ready() {
            event_queue.blocking_dispatch(&mut backend_data)?;
        }

        // Roundtrip to get the main device
        event_queue.roundtrip(&mut backend_data)?;

        // Allocator setup for creating dmabuf backed wl_surfaces.
        {
            match backend_data.protocols.dmabuf_state.main_device() {
                Some(main_device) => {
                    let dev_path = match main_device {
                        MainDevice::LinuxDmabuf(dev) => {
                            let major = major(*dev);
                            let minor = minor(*dev);

                            // If there is no render node we can access, then we are at the mercy of the driver
                            // to allow use of a primary node.
                            dev_path(major, minor, NodeType::Render)
                                .or_else(|_| dev_path(major, minor, NodeType::Primary))
                                .map_err(Into::<AllocateBuffersError>::into)?
                        }

                        // Mesa will provide a path to the Drm node for us.
                        MainDevice::LegacyWlDrm(path) => PathBuf::from(path),
                    };

                    slog::info!(logger, "Opening drm node at {}", dev_path.display());

                    let fd = fcntl::open(&dev_path, OFlag::O_RDWR | OFlag::O_CLOEXEC, Mode::empty())
                        .map_err(Into::<io::Error>::into)
                        .map_err(Into::<AllocateBuffersError>::into)?;

                    let device = Arc::new(Mutex::new(
                        gbm::Device::new(fd).map_err(Into::<AllocateBuffersError>::into)?,
                    ));

                    backend_data.allocator = Some(device);
                }

                None => return Err(AllocateBuffersError::Unsupported.into()),
            }
        }

        let source = WaylandSource::new(event_queue)?;
        let backend_data = Arc::new(Mutex::new(backend_data));

        Ok((
            WaylandBackend {
                source,
                backend_data: backend_data.clone(),
            },
            WaylandHandle {
                conn,
                queue_handle,
                backend_data,
                logger,
            },
        ))
    }
}

/// A handle to the Wayland input and output backend.
#[derive(Debug)]
pub struct WaylandHandle {
    conn: Connection,
    queue_handle: QueueHandle<WaylandBackendData>,
    backend_data: Arc<Mutex<WaylandBackendData>>,
    logger: slog::Logger,
}

impl WaylandHandle {
    /// Returns a handle to the connection.
    pub fn connection(&self) -> Connection {
        self.conn.clone()
    }

    /// Returns a list of formats the compositor can import from a dmabuf.
    pub fn formats(&self) -> impl Iterator<Item = DrmFormat> {
        // TODO: This may need to be per window if we use surface feedback zwp_linux_dmabuf
        self.backend_data.lock().unwrap().protocols.dmabuf_state.formats()
    }

    /// Returns the device used to allocate buffers for presentation.
    pub fn device(&self) -> Arc<Mutex<gbm::Device<RawFd>>> {
        self.backend_data.lock().unwrap().allocator.clone().unwrap()
    }

    // TODO: Replace this with a builder
    pub fn create_window(
        &self,
        format: DrmFourcc,
        modifiers: impl Iterator<Item = DrmModifier>,
    ) -> Result<Window, WaylandError> {
        let mut backend_data = self.backend_data.lock().unwrap();
        let backend_data = &mut *backend_data;

        // Cleanup any windows that have been destroyed so we have room.
        backend_data.windows.retain(|inner| inner.upgrade().is_some());

        // Create a surface to associate with the window.
        let surface = backend_data
            .protocols
            .compositor_state
            .create_surface(&self.queue_handle)
            .expect("TODO"); // TODO: These errors need to be forwarded

        let window = smithay_client_toolkit::shell::xdg::window::Window::builder()
            .map(
                &self.queue_handle,
                &mut backend_data.protocols.xdg_shell_state,
                &mut backend_data.protocols.xdg_window_state,
                surface,
            )
            .expect("TODO"); // TODO: These errors need to be forwarded

        slog::info!(self.logger, "Created new Wayland window");

        let swapchain = Swapchain::new(
            backend_data
                .allocator
                .clone()
                .ok_or(AllocateBuffersError::Unsupported)?,
            0,
            0,
            format,
            modifiers.collect(),
        );

        let id = WindowId(backend_data.next_window_id());

        let inner = Arc::new(window::Inner {
            sctk: window,
            id,
            conn: self.conn.clone(),
            queue_handle: self.queue_handle.clone(),
            backend_data: self.backend_data.clone(),
            data: Mutex::new(Data {
                current_size: (0, 0).into(),
                new_size: None,
                swapchain,
                buffer: None,
                current_buffers: vec![],
                pending_destruction: vec![],
            }),
        });

        let weak = Arc::downgrade(&inner);
        backend_data.windows.push(weak);

        Ok(Window(inner))
    }
}

/// An event from the Wayland compositor.
#[derive(Debug)]
pub enum WaylandEvent {
    /// An input event.
    Input(InputEvent<WaylandInput>),

    /// The window was resized.
    Resized {
        /// The new size of the window.
        new_size: Size<u32, Logical>,

        /// The id of the window which was resized.
        window_id: WindowId,
    },

    /// The compositor has completed a frame callback, indicating the next frame will be drawn soon.
    ///
    /// This event may be used to limit drawing to a window to the frame rate of the output.
    Frame { window_id: WindowId },

    /// A window has received a request to close.
    ///
    /// You need to drop the [`Window`] handle in order to destroy the window.
    CloseRequested {
        /// The window which a close request has been sent to.
        window_id: WindowId,
    },
}

//
// Implementation details
//

impl WaylandBackend {
    fn replay_events<F>(&self, mut f: F)
    where
        F: FnMut(WaylandEvent, &mut ()),
    {
        let events = {
            // Must drop the lock in this scope otherwise events sent to the user will deadlock when using
            // WaylandHandle
            self.backend_data.lock().unwrap().take_recorded()
        };

        for event in events {
            f(event, &mut ());
        }
    }
}

impl EventSource for WaylandBackend {
    type Event = WaylandEvent;
    type Metadata = ();
    type Ret = ();
    type Error = calloop::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        callback: F,
    ) -> calloop::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let mut guard = self.backend_data.lock().unwrap();

        self.source
            .process_events(readiness, token, |_, queue| queue.dispatch_pending(&mut *guard))?;
        // Since the callback may invoke functions on WaylandHandle, we need to unlock the Mutex to prevent deadlocks.
        drop(guard);

        self.replay_events(callback);

        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.source.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.source.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.source.unregister(poll)
    }

    fn pre_run<F>(&mut self, callback: F) -> calloop::Result<()>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let mut guard = self.backend_data.lock().unwrap();

        self.source
            .pre_run(|_, queue| queue.dispatch_pending(&mut *guard))?;
        // Since the callback may invoke functions on WaylandHandle, we need to unlock the Mutex to prevent deadlocking.
        drop(guard);

        self.replay_events(callback);

        Ok(())
    }

    fn post_run<F>(&mut self, callback: F) -> calloop::Result<()>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let mut guard = self.backend_data.lock().unwrap();

        self.source
            .post_run(|_, queue| queue.dispatch_pending(&mut *guard))?;
        // Since the callback may invoke functions on WaylandHandle, we need to unlock the Mutex to prevent deadlocking.
        drop(guard);

        self.replay_events(callback);

        Ok(())
    }
}
