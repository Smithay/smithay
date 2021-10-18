//! Implementation of the backend types using X11.
//!
//! This backend provides the appropriate backend implementations to run a Wayland compositor as an
//! X11 client.
//!
//! The backend is initialized using [`X11Backend::new`](self::X11Backend::new). The function will
//! return two objects:
//!
//! - an [`X11Backend`], which you will insert into an [`EventLoop`](calloop::EventLoop) to process events from the backend.
//! - an [`X11Surface`], which represents a surface that buffers are presented to for display.
//!
//! ## Example usage
//!
//! ```rust,no_run
//! # use std::error::Error;
//! # use smithay::backend::x11::X11Backend;
//! # struct CompositorState;
//! fn init_x11_backend(
//!    handle: calloop::LoopHandle<CompositorState>,
//!    logger: slog::Logger
//! ) -> Result<(), Box<dyn Error>> {
//!     // Create the backend, also yielding a surface that may be used to render to the window.
//!     let (backend, surface) = X11Backend::new(logger)?;
//!     // You can get a handle to the window the backend has created for later use.
//!     let window = backend.window();
//!
//!     // Insert the backend into the event loop to receive events.
//!     handle.insert_source(backend, |event, _window, state| {
//!         // Process events from the X server that apply to the window.
//!     })?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## EGL
//!
//! When using [`EGL`](crate::backend::egl), an [`X11Surface`] may be used to create an [`EGLDisplay`](crate::backend::egl::EGLDisplay).

/*
A note for future contributors and maintainers:

Do take a look at some useful reading in order to understand this backend more deeply:

DRI3 protocol documentation: https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/dri3proto.txt

Present protocol documentation: https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/presentproto.txt
*/

mod buffer;
mod error;
#[macro_use]
mod extension;
mod input;
mod window_inner;

use self::{buffer::PixmapWrapperExt, window_inner::WindowInner};
use crate::{
    backend::{
        allocator::dmabuf::{AsDmabuf, Dmabuf},
        drm::{DrmNode, NodeType},
        input::{Axis, ButtonState, InputEvent, KeyState, MouseButton},
    },
    utils::{x11rb::X11Source, Logical, Size},
};
use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};
use drm_fourcc::DrmFourcc;
use gbm::BufferObjectFlags;
use nix::fcntl;
use slog::{error, info, o, Logger};
use std::{
    io, mem,
    os::unix::prelude::AsRawFd,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Weak,
    },
};
use x11rb::{
    atom_manager,
    connection::Connection,
    protocol::{
        self as x11,
        dri3::ConnectionExt as _,
        xproto::{ColormapAlloc, ConnectionExt, Depth, PixmapWrapper, VisualClass},
        ErrorKind,
    },
    rust_connection::{ReplyError, RustConnection},
};

pub use self::error::*;
use self::extension::Extensions;
pub use self::input::*;

/// An event emitted by the X11 backend.
#[derive(Debug)]
pub enum X11Event {
    /// The X server has required the compositor to redraw the contents of window.
    Refresh,

    /// An input event occurred.
    Input(InputEvent<X11Input>),

    /// The window was resized.
    Resized(Size<u16, Logical>),

    /// The last buffer presented to the window has been displayed.
    ///
    /// When this event is scheduled, the next frame may be rendered.
    PresentCompleted,

    /// The window has received a request to be closed.
    CloseRequested,
}

/// Represents an active connection to the X to manage events on the Window provided by the backend.
#[derive(Debug)]
pub struct X11Backend {
    log: Logger,
    connection: Arc<RustConnection>,
    source: X11Source,
    screen_number: usize,
    window: Arc<WindowInner>,
    resize: Sender<Size<u16, Logical>>,
    key_counter: Arc<AtomicU32>,
    depth: Depth,
    visual_id: u32,
}

atom_manager! {
    pub(crate) Atoms: AtomCollectionCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_NAME,
        UTF8_STRING,
        _SMITHAY_X11_BACKEND_CLOSE,
    }
}

impl X11Backend {
    /// Initializes the X11 backend.
    ///
    /// This connects to the X server and configures the window using the default options.
    pub fn new<L>(logger: L) -> Result<(X11Backend, X11Surface), X11Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        Self::with_size_and_title((1280, 800).into(), "Smithay", logger)
    }

    /// Initializes the X11 backend.
    ///
    /// This connects to the X server and configures the window using the default size and the
    /// specified window title.
    pub fn with_title<L>(title: &str, logger: L) -> Result<(X11Backend, X11Surface), X11Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        Self::with_size_and_title((1280, 800).into(), title, logger)
    }

    /// Initializes the X11 backend.
    ///
    /// This connects to the X server and configures the window using the default window title
    /// and the specified window size.
    pub fn with_size<L>(size: Size<u16, Logical>, logger: L) -> Result<(X11Backend, X11Surface), X11Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        Self::with_size_and_title(size, "Smithay", logger)
    }

    /// Initializes the X11 backend.
    ///
    /// This connects to the X server and configures the window using the specified window size and title.
    pub fn with_size_and_title<L>(
        size: Size<u16, Logical>,
        title: &str,
        logger: L,
    ) -> Result<(X11Backend, X11Surface), X11Error>
    where
        L: Into<Option<slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_x11"));

        info!(logger, "Connecting to the X server");

        let (connection, screen_number) = RustConnection::connect(None)?;
        let connection = Arc::new(connection);
        info!(logger, "Connected to screen {}", screen_number);

        let extensions = Extensions::check_extensions(&*connection, &logger)?;

        let screen = &connection.setup().roots[screen_number];

        let depth = screen
            .allowed_depths
            .iter()
            .find(|depth| depth.depth == 32) // Prefer 32-bit color
            .or_else(|| screen.allowed_depths.iter().find(|depth| depth.depth == 24)) // 24-bit fallback for Xrgb8888
            .cloned()
            .ok_or(CreateWindowError::NoDepth)?;

        // Next find a visual using the supported depth
        let visual_id = depth
            .visuals
            .iter()
            // Ensure the visual is little endian to comply with the format needed with X/ARGB8888
            .filter(|visual| visual.red_mask == 0xff0000)
            .find(|visual| visual.class == VisualClass::TRUE_COLOR)
            .ok_or(CreateWindowError::NoVisual)?
            .visual_id;

        let format = match depth.depth {
            24 => DrmFourcc::Xrgb8888,
            32 => DrmFourcc::Argb8888,
            _ => unreachable!(),
        };

        // Make a colormap
        let colormap = connection.generate_id()?;
        connection.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id)?;

        let atoms = Atoms::new(&*connection)?.reply()?;

        let window = Arc::new(WindowInner::new(
            Arc::downgrade(&connection),
            screen,
            size,
            title,
            format,
            atoms,
            depth.clone(),
            visual_id,
            colormap,
            extensions,
        )?);

        let source = X11Source::new(
            connection.clone(),
            window.id,
            atoms._SMITHAY_X11_BACKEND_CLOSE,
            logger.clone(),
        );

        info!(logger, "Window created");

        let (resize_send, resize_recv) = mpsc::channel();

        let backend = X11Backend {
            log: logger,
            source,
            connection,
            window,
            key_counter: Arc::new(AtomicU32::new(0)),
            depth,
            visual_id,
            screen_number,
            resize: resize_send,
        };

        let surface = X11Surface::new(&backend, format, resize_recv)?;

        Ok((backend, surface))
    }

    /// Returns the default screen number of the X server.
    pub fn screen(&self) -> usize {
        self.screen_number
    }

    /// Returns the underlying connection to the X server.
    pub fn connection(&self) -> &RustConnection {
        &*self.connection
    }

    /// Returns a handle to the X11 window created by the backend.
    pub fn window(&self) -> Window {
        self.window.clone().into()
    }
}

/// An X11 surface which uses GBM to allocate and present buffers.
#[derive(Debug)]
pub struct X11Surface {
    connection: Weak<RustConnection>,
    window: Window,
    resize: Receiver<Size<u16, Logical>>,
    device: gbm::Device<DrmNode>,
    format: DrmFourcc,
    width: u16,
    height: u16,
    current: Dmabuf,
    next: Dmabuf,
}

impl X11Surface {
    fn new(
        backend: &X11Backend,
        format: DrmFourcc,
        resize: Receiver<Size<u16, Logical>>,
    ) -> Result<X11Surface, X11Error> {
        let connection = &backend.connection;
        let window = backend.window();

        // Determine which drm-device the Display is using.
        let screen = &connection.setup().roots[backend.screen()];
        // provider being NONE tells the X server to use the RandR provider.
        let dri3 = match connection.dri3_open(screen.root, x11rb::NONE)?.reply() {
            Ok(reply) => reply,
            Err(err) => {
                return Err(if let ReplyError::X11Error(ref protocol_error) = err {
                    match protocol_error.error_kind {
                        // Implementation is risen when the renderer is not capable of X server is not capable
                        // of rendering at all.
                        ErrorKind::Implementation => X11Error::CannotDirectRender,
                        // Match may occur when the node cannot be authenticated for the client.
                        ErrorKind::Match => X11Error::CannotDirectRender,
                        _ => err.into(),
                    }
                } else {
                    err.into()
                });
            }
        };

        // Take ownership of the container's inner value so we do not need to duplicate the fd.
        // This is fine because the X server will always open a new file descriptor.
        let drm_device_fd = dri3.device_fd.into_raw_fd();

        let fd_flags =
            fcntl::fcntl(drm_device_fd.as_raw_fd(), fcntl::F_GETFD).map_err(AllocateBuffersError::from)?;

        // Enable the close-on-exec flag.
        fcntl::fcntl(
            drm_device_fd,
            fcntl::F_SETFD(fcntl::FdFlag::from_bits_truncate(fd_flags) | fcntl::FdFlag::FD_CLOEXEC),
        )
        .map_err(AllocateBuffersError::from)?;

        // Kernel documentation explains why we should prefer the node to be a render node:
        // https://kernel.readthedocs.io/en/latest/gpu/drm-uapi.html
        //
        // > Render nodes solely serve render clients, that is, no modesetting or privileged ioctls
        // > can be issued on render nodes. Only non-global rendering commands are allowed. If a
        // > driver supports render nodes, it must advertise it via the DRIVER_RENDER DRM driver
        // > capability. If not supported, the primary node must be used for render clients together
        // > with the legacy drmAuth authentication procedure.
        //
        // Since giving the X11 backend the ability to do modesetting is a big nono, we try to only
        // ever create a gbm device from a render node.
        //
        // Of course if the DRM device does not support render nodes, no DRIVER_RENDER capability, then
        // fall back to the primary node.
        let drm_node = DrmNode::from_fd(drm_device_fd).map_err(Into::<AllocateBuffersError>::into)?;
        let drm_node = if drm_node.ty() != NodeType::Render {
            if drm_node.has_render() {
                // Try to get the render node.
                match DrmNode::from_node_with_type(drm_node, NodeType::Render) {
                    Ok(node) => node,
                    Err(err) => {
                        slog::warn!(&backend.log, "Could not create render node from existing DRM node, falling back to primary node");
                        err.node()
                    }
                }
            } else {
                slog::warn!(
                    &backend.log,
                    "DRM Device does not have a render node, falling back to primary node"
                );
                drm_node
            }
        } else {
            drm_node
        };

        // Finally create a GBMDevice to manage the buffers.
        let device = gbm::Device::new(drm_node).map_err(Into::<AllocateBuffersError>::into)?;

        let size = backend.window().size();
        let current = device
            .create_buffer_object::<()>(size.w as u32, size.h as u32, format, BufferObjectFlags::empty())
            .map_err(Into::<AllocateBuffersError>::into)?
            .export()
            .map_err(Into::<AllocateBuffersError>::into)?;

        let next = device
            .create_buffer_object::<()>(size.w as u32, size.h as u32, format, BufferObjectFlags::empty())
            .map_err(Into::<AllocateBuffersError>::into)?
            .export()
            .map_err(Into::<AllocateBuffersError>::into)?;

        Ok(X11Surface {
            connection: Arc::downgrade(connection),
            window,
            device,
            format,
            width: size.w,
            height: size.h,
            current,
            next,
            resize,
        })
    }

    /// Returns a handle to the GBM device used to allocate buffers.
    pub fn device(&self) -> &gbm::Device<DrmNode> {
        &self.device
    }

    /// Returns the format of the buffers the surface accepts.
    pub fn format(&self) -> DrmFourcc {
        self.format
    }

    /// Returns an RAII scoped object which provides the next buffer.
    ///
    /// When the object is dropped, the contents of the buffer are swapped and then presented.
    pub fn present(&mut self) -> Result<Present<'_>, AllocateBuffersError> {
        if let Some(new_size) = self.resize.try_iter().last() {
            self.resize(new_size)?;
        }

        Ok(Present { surface: self })
    }

    fn resize(&mut self, size: Size<u16, Logical>) -> Result<(), AllocateBuffersError> {
        let current = self
            .device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                self.format,
                BufferObjectFlags::empty(),
            )?
            .export()?;

        let next = self
            .device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                self.format,
                BufferObjectFlags::empty(),
            )?
            .export()?;

        self.width = size.w;
        self.height = size.h;
        self.current = current;
        self.next = next;

        Ok(())
    }
}

/// An RAII scope containing the next buffer that will be presented to the window. Presentation
/// occurs when the `Present` is dropped.
///
/// The provided buffer may be bound to a [Renderer](crate::backend::renderer::Renderer) to draw to
/// the window.
///
/// ```rust,ignore
/// // Instantiate a new present object to start the process of presenting.
/// let present = surface.present()?;
///
/// // Bind the buffer to the renderer in order to render.
/// renderer.bind(present.buffer())?;
///
/// // Rendering here!
///
/// // Make sure to unbind the buffer when done.
/// renderer.unbind()?;
///
/// // When the `present` is dropped, what was rendered will be presented to the window.
/// ```
#[derive(Debug)]
pub struct Present<'a> {
    surface: &'a mut X11Surface,
}

impl Present<'_> {
    /// Returns the next buffer that will be presented to the Window.
    ///
    /// You may bind this buffer to a renderer to render.
    pub fn buffer(&self) -> Dmabuf {
        self.surface.next.clone()
    }
}

impl Drop for Present<'_> {
    fn drop(&mut self) {
        let surface = &mut self.surface;

        if let Some(connection) = surface.connection.upgrade() {
            // Swap the buffers
            mem::swap(&mut surface.next, &mut surface.current);

            if let Ok(pixmap) = PixmapWrapper::with_dmabuf(&*connection, &surface.window, &surface.current) {
                // Now present the current buffer
                let _ = pixmap.present(&*connection, &surface.window);
            }

            // Flush the connection after presenting to the window to ensure we don't run out of buffer space in the X11 connection.
            let _ = connection.flush();
        }
    }
}

/// An X11 window.
#[derive(Debug)]
pub struct Window(Weak<WindowInner>);

impl Window {
    /// Sets the title of the window.
    pub fn set_title(&self, title: &str) {
        if let Some(inner) = self.0.upgrade() {
            inner.set_title(title);
        }
    }

    /// Maps the window, making it visible.
    pub fn map(&self) {
        if let Some(inner) = self.0.upgrade() {
            inner.map();
        }
    }

    /// Unmaps the window, making it invisible.
    pub fn unmap(&self) {
        if let Some(inner) = self.0.upgrade() {
            inner.unmap();
        }
    }

    /// Returns the size of this window.
    ///
    /// If the window has been destroyed, the size is `0 x 0`.
    pub fn size(&self) -> Size<u16, Logical> {
        self.0
            .upgrade()
            .map(|inner| inner.size())
            .unwrap_or_else(|| (0, 0).into())
    }

    /// Changes the visibility of the cursor within the confines of the window.
    ///
    /// If `false`, this will hide the cursor. If `true`, this will show the cursor.
    pub fn set_cursor_visible(&self, visible: bool) {
        if let Some(inner) = self.0.upgrade() {
            inner.set_cursor_visible(visible);
        }
    }

    /// Returns the XID of the window.
    pub fn id(&self) -> u32 {
        self.0.upgrade().map(|inner| inner.id).unwrap_or(0)
    }

    /// Returns the depth id of this window.
    pub fn depth(&self) -> u8 {
        self.0.upgrade().map(|inner| inner.depth.depth).unwrap_or(0)
    }

    /// Returns the format expected by the window.
    pub fn format(&self) -> Option<DrmFourcc> {
        self.0.upgrade().map(|inner| inner.format)
    }
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        match (self.0.upgrade(), other.0.upgrade()) {
            (Some(self_), Some(other)) => self_ == other,
            _ => false,
        }
    }
}

impl EventSource for X11Backend {
    type Event = X11Event;

    /// The window the incoming events are applicable to.
    type Metadata = Window;

    type Ret = ();

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        use self::X11Event::Input;

        let connection = self.connection.clone();
        let window = self.window.clone();
        let key_counter = self.key_counter.clone();
        let log = self.log.clone();
        let mut event_window = window.clone().into();
        let resize = &self.resize;

        self.source.process_events(readiness, token, |event, _| {
            match event {
                x11::Event::ButtonPress(button_press) => {
                    if button_press.event == window.id {
                        // X11 decided to associate scroll wheel with a button, 4, 5, 6 and 7 for
                        // up, down, right and left. For scrolling, a press event is emitted and a
                        // release is them immediately followed for scrolling. This means we can
                        // ignore release for scrolling.

                        // Ideally we would use `ButtonIndex` from XCB, however it does not cover 6 and 7
                        // for horizontal scroll and does not work nicely in match statements, so we
                        // use magic constants here:
                        //
                        // 1 => MouseButton::Left
                        // 2 => MouseButton::Middle
                        // 3 => MouseButton::Right
                        // 4 => Axis::Vertical +1.0
                        // 5 => Axis::Vertical -1.0
                        // 6 => Axis::Horizontal -1.0
                        // 7 => Axis::Horizontal +1.0
                        // Others => ??
                        match button_press.detail {
                            1..=3 => {
                                // Clicking a button.
                                callback(
                                    Input(InputEvent::PointerButton {
                                        event: X11MouseInputEvent {
                                            time: button_press.time,
                                            button: match button_press.detail {
                                                1 => MouseButton::Left,

                                                // Confusion: XCB docs for ButtonIndex and what plasma does don't match?
                                                2 => MouseButton::Middle,

                                                3 => MouseButton::Right,

                                                _ => unreachable!(),
                                            },
                                            state: ButtonState::Pressed,
                                        },
                                    }),
                                    &mut event_window,
                                )
                            }

                            4..=7 => {
                                // Scrolling
                                callback(
                                    Input(InputEvent::PointerAxis {
                                        event: X11MouseWheelEvent {
                                            time: button_press.time,
                                            axis: match button_press.detail {
                                                // Up | Down
                                                4 | 5 => Axis::Vertical,

                                                // Right | Left
                                                6 | 7 => Axis::Horizontal,

                                                _ => unreachable!(),
                                            },
                                            amount: match button_press.detail {
                                                // Up | Right
                                                4 | 7 => 1.0,

                                                // Down | Left
                                                5 | 6 => -1.0,

                                                _ => unreachable!(),
                                            },
                                        },
                                    }),
                                    &mut event_window,
                                )
                            }

                            // Unknown mouse button
                            _ => callback(
                                Input(InputEvent::PointerButton {
                                    event: X11MouseInputEvent {
                                        time: button_press.time,
                                        button: MouseButton::Other(button_press.detail),
                                        state: ButtonState::Pressed,
                                    },
                                }),
                                &mut event_window,
                            ),
                        }
                    }
                }

                x11::Event::ButtonRelease(button_release) => {
                    if button_release.event == window.id {
                        match button_release.detail {
                            1..=3 => {
                                // Releasing a button.
                                callback(
                                    Input(InputEvent::PointerButton {
                                        event: X11MouseInputEvent {
                                            time: button_release.time,
                                            button: match button_release.detail {
                                                1 => MouseButton::Left,

                                                2 => MouseButton::Middle,

                                                3 => MouseButton::Right,

                                                _ => unreachable!(),
                                            },
                                            state: ButtonState::Released,
                                        },
                                    }),
                                    &mut event_window,
                                )
                            }

                            // We may ignore the release tick for scrolling, as the X server will
                            // always emit this immediately after press.
                            4..=7 => (),

                            _ => callback(
                                Input(InputEvent::PointerButton {
                                    event: X11MouseInputEvent {
                                        time: button_release.time,
                                        button: MouseButton::Other(button_release.detail),
                                        state: ButtonState::Released,
                                    },
                                }),
                                &mut event_window,
                            ),
                        }
                    }
                }

                x11::Event::KeyPress(key_press) => {
                    if key_press.event == window.id {
                        callback(
                            Input(InputEvent::Keyboard {
                                event: X11KeyboardInputEvent {
                                    time: key_press.time,
                                    // X11's keycodes are +8 relative to the libinput keycodes
                                    // that are expected, so subtract 8 from each keycode to
                                    // match libinput.
                                    //
                                    // https://github.com/freedesktop/xorg-xf86-input-libinput/blob/master/src/xf86libinput.c#L54
                                    key: key_press.detail as u32 - 8,
                                    count: key_counter.fetch_add(1, Ordering::SeqCst) + 1,
                                    state: KeyState::Pressed,
                                },
                            }),
                            &mut event_window,
                        )
                    }
                }

                x11::Event::KeyRelease(key_release) => {
                    if key_release.event == window.id {
                        // atomic u32 has no checked_sub, so load and store to do the same.
                        let mut key_counter_val = key_counter.load(Ordering::SeqCst);
                        key_counter_val = key_counter_val.saturating_sub(1);
                        key_counter.store(key_counter_val, Ordering::SeqCst);

                        callback(
                            Input(InputEvent::Keyboard {
                                event: X11KeyboardInputEvent {
                                    time: key_release.time,
                                    // X11's keycodes are +8 relative to the libinput keycodes
                                    // that are expected, so subtract 8 from each keycode to
                                    // match libinput.
                                    //
                                    // https://github.com/freedesktop/xorg-xf86-input-libinput/blob/master/src/xf86libinput.c#L54
                                    key: key_release.detail as u32 - 8,
                                    count: key_counter_val,
                                    state: KeyState::Released,
                                },
                            }),
                            &mut event_window,
                        );
                    }
                }

                x11::Event::MotionNotify(motion_notify) => {
                    if motion_notify.event == window.id {
                        // Use event_x/y since those are relative the the window receiving events.
                        let x = motion_notify.event_x as f64;
                        let y = motion_notify.event_y as f64;

                        callback(
                            Input(InputEvent::PointerMotionAbsolute {
                                event: X11MouseMovedEvent {
                                    time: motion_notify.time,
                                    x,
                                    y,
                                    size: window.size(),
                                },
                            }),
                            &mut event_window,
                        )
                    }
                }

                x11::Event::ConfigureNotify(configure_notify) => {
                    if configure_notify.window == window.id {
                        let previous_size = { *window.size.lock().unwrap() };

                        // Did the size of the window change?
                        let configure_notify_size: Size<u16, Logical> =
                            (configure_notify.width, configure_notify.height).into();

                        if configure_notify_size != previous_size {
                            // Intentionally drop the lock on the size mutex incase a user
                            // requests a resize or does something which causes a resize
                            // inside the callback.
                            {
                                *window.size.lock().unwrap() = configure_notify_size;
                            }

                            (callback)(X11Event::Resized(configure_notify_size), &mut event_window);
                            let _ = resize.send(configure_notify_size);
                        }
                    }
                }

                x11::Event::EnterNotify(enter_notify) => {
                    if enter_notify.event == window.id {
                        window.cursor_enter();
                    }
                }

                x11::Event::LeaveNotify(leave_notify) => {
                    if leave_notify.event == window.id {
                        window.cursor_leave();
                    }
                }

                x11::Event::ClientMessage(client_message) => {
                    if client_message.data.as_data32()[0] == window.atoms.WM_DELETE_WINDOW // Destroy the window?
                            && client_message.window == window.id
                    // Same window
                    {
                        (callback)(X11Event::CloseRequested, &mut event_window);
                    }
                }

                x11::Event::Expose(expose) => {
                    if expose.window == window.id && expose.count == 0 {
                        (callback)(X11Event::Refresh, &mut event_window);
                    }
                }

                x11::Event::PresentCompleteNotify(complete_notify) => {
                    if complete_notify.window == window.id {
                        window.last_msc.store(complete_notify.msc, Ordering::SeqCst);

                        (callback)(X11Event::PresentCompleted, &mut event_window);
                    }
                }

                x11::Event::PresentIdleNotify(_) => {
                    // Pixmap is reference counted in the X server, so we do not need to take and drop.
                }

                x11::Event::Error(e) => {
                    error!(log, "X11 protocol error: {:?}", e);
                }

                _ => (),
            }

            // Flush the connection so changes to the window state during callbacks can be emitted.
            let _ = connection.flush();
        })
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.source.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.source.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> io::Result<()> {
        self.source.unregister(poll)
    }
}
