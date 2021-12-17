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
//! # use std::{sync::{Arc, Mutex}, error::Error};
//! # use smithay::backend::x11::{X11Backend, X11Surface, WindowBuilder};
//! use smithay::backend::egl::{EGLDisplay, EGLContext};
//! use smithay::reexports::gbm;
//! use std::collections::HashSet;
//!
//! # struct CompositorState;
//! fn init_x11_backend(
//!    handle: calloop::LoopHandle<CompositorState>,
//!    logger: slog::Logger
//! ) -> Result<(), Box<dyn Error>> {
//!     // Create the backend, also yielding a surface that may be used to render to the window.
//!     let backend = X11Backend::new(logger.clone())?;
//!
//!     // Get a handle from the backend to interface with the X server
//!     let x_handle = backend.handle();
//!     // Create a window
//!     let window = WindowBuilder::new()
//!         .title("Wayland inside X11")
//!         .build(&x_handle)
//!         .expect("Could not create window");
//!
//!     // To render to a window, we need to create an X11 surface.
//!
//!     // Get the DRM node used by the X server for direct rendering.
//!     let drm_node = x_handle.drm_node()?;
//!     // Create the gbm device for allocating buffers
//!     let device = gbm::Device::new(drm_node)?;
//!     // Initialize EGL to retrieve the support modifier list
//!     let egl = EGLDisplay::new(&device, logger.clone()).expect("Failed to create EGLDisplay");
//!     let context = EGLContext::new(&egl, logger).expect("Failed to create EGLContext");
//!     let modifiers = context.dmabuf_render_formats().iter().map(|format| format.modifier).collect::<HashSet<_>>();
//!
//!     // Wrap up the device for sharing among the created X11 surfaces.
//!     let device = Arc::new(Mutex::new(device));
//!     // Finally create the X11 surface, you will use this to obtain buffers that will be presented to the
//!     // window.
//!     let surface = x_handle.create_surface(&window, device, modifiers.into_iter());
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
        allocator::{
            dmabuf::{AsDmabuf, Dmabuf},
            Slot, Swapchain,
        },
        drm::{node::path_to_type, CreateDrmNodeError, DrmNode, NodeType},
        egl::{native::X11DefaultDisplay, EGLDevice, EGLDisplay, Error as EGLError},
        input::{Axis, ButtonState, InputEvent, KeyState},
    },
    utils::{x11rb::X11Source, Logical, Size},
};
use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};
use drm_fourcc::{DrmFourcc, DrmModifier};
use gbm::BufferObject;
use nix::{
    fcntl::{self, OFlag},
    sys::stat::Mode,
};
use slog::{error, info, o, Logger};
use std::{
    collections::HashMap,
    io, mem,
    os::unix::prelude::AsRawFd,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex, MutexGuard, Weak,
    },
};
use x11rb::{
    atom_manager,
    connection::Connection,
    protocol::{
        self as x11,
        dri3::ConnectionExt as _,
        xproto::{
            ColormapAlloc, ConnectionExt, CreateWindowAux, PixmapWrapper, VisualClass, WindowClass,
            WindowWrapper,
        },
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
    inner: Arc<Mutex<X11Inner>>,
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
    /// Initializes the X11 backend by connecting to the X server.
    pub fn new<L>(logger: L) -> Result<X11Backend, X11Error>
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

        // We need to give the X11Source a window we have created, we cannot send the close event to the root
        // window (0). To handle this, we will create a window we never map or provide to users to the backend
        // can be sent a message for shutdown.

        let close_window = WindowWrapper::create_window(
            &*connection,
            x11rb::COPY_DEPTH_FROM_PARENT,
            screen.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )?
        .into_window();

        let source = X11Source::new(
            connection.clone(),
            close_window,
            atoms._SMITHAY_X11_BACKEND_CLOSE,
            logger.clone(),
        );

        let inner = X11Inner {
            log: logger.clone(),
            connection: connection.clone(),
            screen_number,
            windows: HashMap::new(),
            key_counter: Arc::new(AtomicU32::new(0)),
            window_format: format,
            extensions,
            colormap,
            atoms,
            depth,
            visual_id,
        };

        Ok(X11Backend {
            log: logger,
            connection,
            source,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Returns a handle to the X11 backend.
    pub fn handle(&self) -> X11Handle {
        X11Handle {
            log: self.log.clone(),
            connection: self.connection.clone(),
            inner: self.inner.clone(),
        }
    }
}

/// An X11 surface which uses GBM to allocate and present buffers.
#[derive(Debug)]
pub struct X11Surface {
    connection: Weak<RustConnection>,
    window: Weak<WindowInner>,
    resize: Receiver<Size<u16, Logical>>,
    swapchain: Swapchain<Arc<Mutex<gbm::Device<DrmNode>>>, BufferObject<()>>,
    format: DrmFourcc,
    width: u16,
    height: u16,
    buffer: Option<Slot<BufferObject<()>>>,
}

#[derive(Debug, thiserror::Error)]
enum EGLInitError {
    #[error(transparent)]
    EGL(#[from] EGLError),
    #[error(transparent)]
    IO(#[from] io::Error),
}

fn egl_init(_: &X11Inner) -> Result<DrmNode, EGLInitError> {
    let display = EGLDisplay::new(&X11DefaultDisplay, None)?;
    let device = EGLDevice::device_for_display(&display)?;
    let path = path_to_type(device.drm_device_path()?, NodeType::Render)?;
    fcntl::open(&path, OFlag::O_RDWR | OFlag::O_CLOEXEC, Mode::empty())
        .map_err(Into::<io::Error>::into)
        .and_then(|fd| {
            DrmNode::from_fd(fd).map_err(|err| match err {
                CreateDrmNodeError::Io(err) => err,
                _ => unreachable!(),
            })
        })
        .map_err(EGLInitError::IO)
}

fn dri3_init(x11: &X11Inner) -> Result<DrmNode, X11Error> {
    let connection = &x11.connection;

    // Determine which drm-device the Display is using.
    let screen = &connection.setup().roots[x11.screen_number];
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
    let drm_node = DrmNode::from_fd(drm_device_fd).map_err(Into::<AllocateBuffersError>::into)?;

    if drm_node.ty() != NodeType::Render && drm_node.has_render() {
        // Try to get the render node.
        if let Some(path) = drm_node.dev_path_with_type(NodeType::Render) {
            return Ok(fcntl::open(&path, OFlag::O_RDWR | OFlag::O_CLOEXEC, Mode::empty())
                .map_err(Into::<std::io::Error>::into)
                .map_err(CreateDrmNodeError::Io)
                .and_then(DrmNode::from_fd)
                .unwrap_or_else(|err| {
                    slog::warn!(&x11.log, "Could not create render node from existing DRM node ({}), falling back to primary node", err);
                    drm_node
                }));
        }
    }

    slog::warn!(
        &x11.log,
        "DRM Device does not have a render node, falling back to primary node"
    );
    Ok(drm_node)
}

/// A handle to the X11 backend.
///
/// This is the primary object used to interface with the backend.
#[derive(Debug)]
pub struct X11Handle {
    log: Logger,
    connection: Arc<RustConnection>,
    inner: Arc<Mutex<X11Inner>>,
}

impl X11Handle {
    /// Returns the default screen number of the X server.
    pub fn screen(&self) -> usize {
        self.inner.lock().unwrap().screen_number
    }

    /// Returns the underlying connection to the X server.
    pub fn connection(&self) -> Arc<RustConnection> {
        self.connection.clone()
    }

    /// Returns the format of the window.
    pub fn format(&self) -> DrmFourcc {
        self.inner.lock().unwrap().window_format
    }

    /// Returns the DRM node the X server uses for direct rendering.
    ///
    /// The DRM node may be used to create a [`gbm::Device`] to allocate buffers.
    pub fn drm_node(&self) -> Result<DrmNode, X11Error> {
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

        // We cannot fallback on the egl_init method, because there is no way for us to authenticate a primary node.
        // dri3 does not work for closed-source drivers, but *may* give us a authenticated fd as a fallback.
        // As a result we try to use egl for a cleaner, better supported approach at first and only if that fails use dri3.
        let inner = self.inner.lock().unwrap();

        egl_init(&*inner).or_else(|err| {
            slog::warn!(
                &self.log,
                "Failed to init X11 surface via egl, falling back to dri3: {}",
                err
            );
            dri3_init(&*inner)
        })
    }

    /// Creates a surface that allocates and presents buffers to the window.
    ///
    /// This will fail if the window has already been used to create a surface.
    pub fn create_surface(
        &self,
        window: &Window,
        device: Arc<Mutex<gbm::Device<DrmNode>>>,
        modifiers: impl Iterator<Item = DrmModifier>,
    ) -> Result<X11Surface, X11Error> {
        let has_resize = { window.0.resize.lock().unwrap().is_some() };

        if has_resize {
            return Err(X11Error::SurfaceExists);
        }

        let inner = self.inner.clone();
        let inner_guard = inner.lock().unwrap();

        // Fail if the window is not managed by this backend or is destroyed
        if !inner_guard.windows.contains_key(&window.id()) {
            return Err(X11Error::InvalidWindow);
        }

        let modifiers = modifiers.collect::<Vec<_>>();

        let format = window.0.format;
        let size = window.size();
        let swapchain = Swapchain::new(device, size.w as u32, size.h as u32, format, modifiers);

        let (sender, recv) = mpsc::channel();

        {
            let mut resize = window.0.resize.lock().unwrap();
            *resize = Some(sender);
        }

        Ok(X11Surface {
            connection: Arc::downgrade(&inner_guard.connection),
            window: Arc::downgrade(&window.0),
            swapchain,
            format,
            width: size.w,
            height: size.h,
            buffer: None,
            resize: recv,
        })
    }
}

impl X11Surface {
    /// Returns the window the surface presents to.
    ///
    /// This will return [`None`] if the window has been destroyed.
    pub fn window(&self) -> Option<impl AsRef<Window> + '_> {
        let window = self.window.upgrade().map(Window).map(WindowTemporary);

        struct WindowTemporary(Window);

        impl AsRef<Window> for WindowTemporary {
            fn as_ref(&self) -> &Window {
                &self.0
            }
        }

        window
    }

    /// Returns a handle to the GBM device used to allocate buffers.
    pub fn device(&self) -> MutexGuard<'_, gbm::Device<DrmNode>> {
        self.swapchain.allocator.lock().unwrap()
    }

    /// Returns the format of the buffers the surface accepts.
    pub fn format(&self) -> DrmFourcc {
        self.format
    }

    /// Returns the next buffer that will be presented to the Window and its age.
    ///
    /// You may bind this buffer to a renderer to render.
    /// This function will return the same buffer until [`submit`](Self::submit) is called
    /// or [`reset_buffers`](Self::reset_buffer) is used to reset the buffers.
    pub fn buffer(&mut self) -> Result<(Dmabuf, u8), AllocateBuffersError> {
        if let Some(new_size) = self.resize.try_iter().last() {
            self.resize(new_size)?;
        }

        if self.buffer.is_none() {
            self.buffer = Some(
                self.swapchain
                    .acquire()
                    .map_err(Into::<AllocateBuffersError>::into)?
                    .ok_or(AllocateBuffersError::NoFreeSlots)?,
            );
        }

        let slot = self.buffer.as_ref().unwrap();
        let age = slot.age();
        match slot.userdata().get::<Dmabuf>() {
            Some(dmabuf) => Ok((dmabuf.clone(), age)),
            None => {
                let dmabuf = slot.export().map_err(Into::<AllocateBuffersError>::into)?;
                slot.userdata().insert_if_missing(|| dmabuf.clone());
                Ok((dmabuf, age))
            }
        }
    }

    /// Consume and submit the buffer to the window.
    pub fn submit(&mut self) -> Result<(), AllocateBuffersError> {
        if let Some(connection) = self.connection.upgrade() {
            // Get a new buffer
            let mut next = self
                .swapchain
                .acquire()
                .map_err(Into::<AllocateBuffersError>::into)?
                .ok_or(AllocateBuffersError::NoFreeSlots)?;

            // Swap the buffers
            if let Some(current) = self.buffer.as_mut() {
                mem::swap(&mut next, current);
            }

            let window = self.window().ok_or(AllocateBuffersError::WindowDestroyed)?;

            if let Ok(pixmap) = PixmapWrapper::with_dmabuf(
                &*connection,
                window.as_ref(),
                next.userdata().get::<Dmabuf>().unwrap(),
            ) {
                // Now present the current buffer
                let _ = pixmap.present(&*connection, window.as_ref());
            }
            self.swapchain.submitted(next);

            // Flush the connection after presenting to the window to ensure we don't run out of buffer space in the X11 connection.
            let _ = connection.flush();
        }
        Ok(())
    }

    /// Resets the internal buffers, e.g. to reset age values
    pub fn reset_buffers(&mut self) {
        self.swapchain.reset_buffers();
        self.buffer = None;
    }

    fn resize(&mut self, size: Size<u16, Logical>) -> Result<(), AllocateBuffersError> {
        self.swapchain.resize(size.w as u32, size.h as u32);
        self.buffer = None;

        self.width = size.w;
        self.height = size.h;

        Ok(())
    }
}

/// Builder used to construct a window.
#[derive(Debug)]
pub struct WindowBuilder<'a> {
    name: Option<&'a str>,
    size: Option<Size<u16, Logical>>,
}

impl<'a> WindowBuilder<'a> {
    #[allow(clippy::new_without_default)]
    /// Returns a new builder.
    pub fn new() -> WindowBuilder<'a> {
        WindowBuilder {
            name: None,
            size: None,
        }
    }

    /// Sets the title of the window that will be created by the builder.
    pub fn title(self, name: &'a str) -> Self {
        Self {
            name: Some(name),
            ..self
        }
    }

    /// Sets the size of the window that will be created.
    ///
    /// There is no guarantee the size specified here will be the actual size of the window when it is
    /// presented.
    pub fn size(self, size: Size<u16, Logical>) -> Self {
        Self {
            size: Some(size),
            ..self
        }
    }

    /// Creates a window using the options specified in the builder.
    pub fn build(self, handle: &X11Handle) -> Result<Window, X11Error> {
        let connection = handle.connection();

        let inner = &mut *handle.inner.lock().unwrap();

        let window = Arc::new(WindowInner::new(
            Arc::downgrade(&connection),
            &connection.setup().roots[inner.screen_number],
            self.size.unwrap_or_else(|| (1280, 800).into()),
            self.name.unwrap_or("Smithay"),
            inner.window_format,
            inner.atoms,
            inner.depth.clone(),
            inner.visual_id,
            inner.colormap,
            inner.extensions,
        )?);

        let downgrade = Arc::downgrade(&window);
        inner.windows.insert(window.id, downgrade);

        Ok(Window(window))
    }
}

/// An X11 window.
///
/// Dropping an instance of the window will destroy it.
#[derive(Debug)]
pub struct Window(Arc<WindowInner>);

impl Window {
    /// Sets the title of the window.
    pub fn set_title(&self, title: &str) {
        self.0.set_title(title);
    }

    /// Maps the window, making it visible.
    pub fn map(&self) {
        self.0.map();
    }

    /// Unmaps the window, making it invisible.
    pub fn unmap(&self) {
        self.0.unmap();
    }

    /// Returns the size of this window.
    ///
    /// If the window has been destroyed, the size is `0 x 0`.
    pub fn size(&self) -> Size<u16, Logical> {
        self.0.size()
    }

    /// Changes the visibility of the cursor within the confines of the window.
    ///
    /// If `false`, this will hide the cursor. If `true`, this will show the cursor.
    pub fn set_cursor_visible(&self, visible: bool) {
        self.0.set_cursor_visible(visible);
    }

    /// Returns the XID of the window.
    pub fn id(&self) -> u32 {
        self.0.id
    }

    /// Returns the depth id of this window.
    pub fn depth(&self) -> u8 {
        self.0.depth.depth
    }

    /// Returns the format expected by the window.
    pub fn format(&self) -> DrmFourcc {
        self.0.format
    }
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
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
        let connection = self.connection.clone();
        let log = self.log.clone();
        let inner = self.inner.clone();

        let post_action = self.source.process_events(readiness, token, |event, _| {
            X11Inner::process_event(&inner, &log, event, &mut callback);
        })?;

        // Flush the connection so changes to the window state during callbacks can be emitted.
        let _ = connection.flush();

        Ok(post_action)
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

#[derive(Debug)]
pub(crate) struct X11Inner {
    log: Logger,
    connection: Arc<RustConnection>,
    screen_number: usize,
    windows: HashMap<u32, Weak<WindowInner>>,
    key_counter: Arc<AtomicU32>,
    window_format: DrmFourcc,
    extensions: Extensions,
    colormap: u32,
    atoms: Atoms,
    depth: x11::xproto::Depth,
    visual_id: u32,
}

impl X11Inner {
    fn process_event<F>(inner: &Arc<Mutex<X11Inner>>, log: &Logger, event: x11::Event, callback: &mut F)
    where
        F: FnMut(X11Event, &mut Window),
    {
        use self::X11Event::Input;

        fn window_from_id(inner: &Arc<Mutex<X11Inner>>, id: &u32) -> Option<Arc<WindowInner>> {
            inner
                .lock()
                .unwrap()
                .windows
                .get(id)
                .cloned()
                .and_then(|weak| weak.upgrade())
        }

        // If X11 is deadlocking somewhere here, make sure you drop your mutex guards.

        match event {
            x11::Event::ButtonPress(button_press) => {
                if let Some(window) = window_from_id(inner, &button_press.event) {
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

                    // Scrolling
                    if button_press.detail >= 4 && button_press.detail <= 7 {
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
                            &mut Window(window),
                        )
                    } else {
                        callback(
                            Input(InputEvent::PointerButton {
                                event: X11MouseInputEvent {
                                    time: button_press.time,
                                    raw: button_press.detail as u32,
                                    state: ButtonState::Pressed,
                                },
                            }),
                            &mut Window(window),
                        )
                    }
                }
            }

            x11::Event::ButtonRelease(button_release) => {
                // Ignore release tick because this event is always sent immediately after the press
                // tick for scrolling and the backend will dispatch release event automatically during
                // the press event.
                if button_release.detail >= 4 && button_release.detail <= 7 {
                    return;
                }

                if let Some(window) = window_from_id(inner, &button_release.event) {
                    callback(
                        Input(InputEvent::PointerButton {
                            event: X11MouseInputEvent {
                                time: button_release.time,
                                raw: button_release.detail as u32,
                                state: ButtonState::Released,
                            },
                        }),
                        &mut Window(window),
                    );
                }
            }

            x11::Event::KeyPress(key_press) => {
                if let Some(window) = window_from_id(inner, &key_press.event) {
                    // Do not hold the lock.
                    let count = { inner.lock().unwrap().key_counter.fetch_add(1, Ordering::SeqCst) + 1 };

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
                                count,
                                state: KeyState::Pressed,
                            },
                        }),
                        &mut Window(window),
                    )
                }
            }

            x11::Event::KeyRelease(key_release) => {
                if let Some(window) = window_from_id(inner, &key_release.event) {
                    let count = {
                        let key_counter = inner.lock().unwrap().key_counter.clone();

                        // atomic u32 has no checked_sub, so load and store to do the same.
                        let mut count = key_counter.load(Ordering::SeqCst);
                        count = count.saturating_sub(1);
                        key_counter.store(count, Ordering::SeqCst);

                        count
                    };

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
                                count,
                                state: KeyState::Released,
                            },
                        }),
                        &mut Window(window),
                    );
                }
            }

            x11::Event::MotionNotify(motion_notify) => {
                if let Some(window) = window_from_id(inner, &motion_notify.event) {
                    // Use event_x/y since those are relative the the window receiving events.
                    let x = motion_notify.event_x as f64;
                    let y = motion_notify.event_y as f64;

                    let window_size = { *window.size.lock().unwrap() };

                    callback(
                        Input(InputEvent::PointerMotionAbsolute {
                            event: X11MouseMovedEvent {
                                time: motion_notify.time,
                                x,
                                y,
                                size: window_size,
                            },
                        }),
                        &mut Window(window),
                    )
                }
            }

            x11::Event::ConfigureNotify(configure_notify) => {
                if let Some(window) = window_from_id(inner, &configure_notify.window) {
                    let previous_size = { *window.size.lock().unwrap() };

                    // Did the size of the window change?
                    let configure_notify_size: Size<u16, Logical> =
                        (configure_notify.width, configure_notify.height).into();

                    if configure_notify_size != previous_size {
                        // Intentionally drop the lock on the size mutex incase a user
                        // requests a resize or does something which causes a resize
                        // inside the callback.
                        {
                            let mut resize_guard = window.size.lock().unwrap();
                            *resize_guard = configure_notify_size;
                        }

                        (callback)(
                            X11Event::Resized(configure_notify_size),
                            &mut Window(window.clone()),
                        );

                        if let Some(resize_sender) = window.resize.lock().unwrap().as_ref() {
                            let _ = resize_sender.send(configure_notify_size);
                        }
                    }
                }
            }

            x11::Event::EnterNotify(enter_notify) => {
                if let Some(window) = window_from_id(inner, &enter_notify.event) {
                    window.cursor_enter();
                }
            }

            x11::Event::LeaveNotify(leave_notify) => {
                if let Some(window) = window_from_id(inner, &leave_notify.event) {
                    window.cursor_leave();
                }
            }

            x11::Event::ClientMessage(client_message) => {
                if let Some(window) = window_from_id(inner, &client_message.window) {
                    if client_message.data.as_data32()[0] == window.atoms.WM_DELETE_WINDOW
                    // Destroy the window?
                    {
                        (callback)(X11Event::CloseRequested, &mut Window(window));
                    }
                }
            }

            x11::Event::Expose(expose) => {
                if let Some(window) = window_from_id(inner, &expose.window) {
                    if expose.count == 0 {
                        (callback)(X11Event::Refresh, &mut Window(window));
                    }
                }
            }

            x11::Event::PresentCompleteNotify(complete_notify) => {
                if let Some(window) = window_from_id(inner, &complete_notify.window) {
                    window.last_msc.store(complete_notify.msc, Ordering::SeqCst);

                    (callback)(X11Event::PresentCompleted, &mut Window(window));
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
    }
}
