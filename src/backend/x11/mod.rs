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
//! # use smithay::backend::x11::{X11Connection, X11Backend, WindowBuilder};
//! use smithay::backend::egl::{EGLDisplay, EGLContext, EGLSurface, context::GlAttributes};
//! use std::collections::HashSet;
//!
//! # struct CompositorState;
//! fn init_x11_backend(
//!    handle: calloop::LoopHandle<CompositorState>,
//!    logger: slog::Logger
//! ) -> Result<(), Box<dyn Error>> {
//!     // First lets connect to the X11 server
//!     let connection = X11Connection::new(logger.clone())?;
//!
//!     // Create the EGLDisplay and EGLContext from the connection for direct rendering
//!     let display = EGLDisplay::new(&connection, logger.clone())?;
//!     let context = EGLContext::new_with_config(
//!         &display,
//!         GlAttributes {
//!             version: (3, 0),
//!             profile: None,
//!             debug: false,
//!             vsync: true,
//!         },
//!         Default::default(),
//!         logger.clone(),
//!     )?;
//!
//!     // Create the backend, also yielding a surface that may be used to render to the window.
//!     let backend = X11Backend::new(connection)?;
//!
//!     // Get a handle from the backend to interface with the X server
//!     let x_handle = backend.handle();
//!     // Create a window
//!     let window = WindowBuilder::new()
//!         .title("Wayland inside X11")
//!         .visual_from_context(&context)?
//!         .build(&x_handle)?;
//!
//!     // To render to a window, we need to create an EGLSurface.
//!     let surface = EGLSurface::new(
//!         &display,
//!         context.pixel_format().unwrap(),
//!         context.config_id(),
//!         window.clone(),
//!         logger.clone(),
//!     )?;
//!     // you can now bind the egl surface to a renderer
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

mod conn;
mod error;
#[macro_use]
mod extension;
mod input;
mod window_inner;

use crate::{
    backend::{
        egl::{ffi, EGLContext},
        input::{Axis, ButtonState, InputEvent, KeyState},
    },
    utils::{x11rb::X11Source, Logical, Size},
};
use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};
use slog::{error, Logger};
use std::{
    collections::HashMap,
    io,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex, Weak,
    },
};
use x11rb::{
    atom_manager,
    connection::Connection,
    protocol::{
        self as x11,
        xproto::{ColormapAlloc, ConnectionExt, CreateWindowAux, WindowClass, WindowWrapper},
    },
    xcb_ffi::XCBConnection,
};

use self::window_inner::WindowInner;

pub use self::conn::*;
pub use self::error::*;
pub use self::input::*;

/// An event emitted by the X11 backend.
#[derive(Debug)]
pub enum X11Event {
    /// An input event occurred.
    Input(InputEvent<X11Input>),

    /// The window was resized.
    Resized {
        /// The new size of the window
        new_size: Size<u16, Logical>,
        /// XID of the window
        window_id: u32,
    },

    /// The window has received a request to be closed.
    CloseRequested {
        /// XID of the window
        window_id: u32,
    },
}

/// Represents an active connection to the X to manage events on the Window provided by the backend.
#[derive(Debug)]
pub struct X11Backend {
    connection: X11Connection,
    source: X11Source<XCBConnection>,
    inner: Arc<Mutex<X11Inner>>,
    log: Logger,
}

impl X11Backend {
    /// Initializes the X11 backend by connecting to the X server.
    pub fn new(connection: X11Connection) -> Result<X11Backend, X11Error> {
        let screen = &connection.setup().roots[connection.screen];
        let logger = connection.logger.clone();

        let atoms = Atoms::new(&connection)?.reply()?;

        // We need to give the X11Source a window we have created, we cannot send the close event to the root
        // window (0). To handle this, we will create a window we never map or provide to users to the backend
        // can be sent a message for shutdown.

        let close_window = WindowWrapper::create_window(
            &connection,
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
            connection.xcb.clone(),
            close_window,
            atoms._SMITHAY_X11_BACKEND_CLOSE,
            logger.clone(),
        );

        let inner = X11Inner {
            screen_number: connection.screen,
            windows: HashMap::new(),
            key_counter: Arc::new(AtomicU32::new(0)),
            atoms,
            devices: false,
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
            connection: self.connection.clone(),
            inner: self.inner.clone(),
        }
    }
}

/// A handle to the X11 backend.
///
/// This is the primary object used to interface with the backend.
#[derive(Debug)]
pub struct X11Handle {
    connection: X11Connection,
    inner: Arc<Mutex<X11Inner>>,
}

impl X11Handle {
    /// Returns the default screen number of the X server.
    pub fn screen(&self) -> usize {
        self.inner.lock().unwrap().screen_number
    }

    /// Returns the underlying connection to the X server.
    pub fn connection(&self) -> X11Connection {
        self.connection.clone()
    }

    /// Get a temporary reference to a window by its XID
    pub fn window_ref_from_id(&self, id: u32) -> Option<impl AsRef<Window> + '_> {
        X11Inner::window_ref_from_id(&self.inner, &id)
            .and_then(|w| w.upgrade())
            .map(Window)
            .map(WindowTemporary)
    }
}

/// Builder used to construct a window.
#[derive(Debug)]
pub struct WindowBuilder<'a> {
    name: Option<&'a str>,
    size: Option<Size<u16, Logical>>,
    visual_id: Option<u32>,
}

impl<'a> WindowBuilder<'a> {
    #[allow(clippy::new_without_default)]
    /// Returns a new builder.
    pub fn new() -> WindowBuilder<'a> {
        WindowBuilder {
            name: None,
            size: None,
            visual_id: None,
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

    /// Derives the visual_id used for window creation from the given [`EGLContext`].
    ///
    /// The context needs to originate from an `EGLDisplay` created from the same [`X11Connection`],
    /// as the [`X11Handle`], that will be passed to [`WindowBuilder::build`].
    /// Additionally the [`EGLContext`] needs to have been created with a config (see [`EGLContext::new_with_config`]),
    /// otherwise this function will fail.
    pub fn visual_from_context(self, context: &EGLContext) -> Result<Self, X11Error> {
        Ok(Self {
            visual_id: unsafe {
                let mut id = 0i32;
                if ffi::egl::GetConfigAttrib(
                    **context.display.display,
                    context.config_id(),
                    ffi::egl::NATIVE_VISUAL_ID as i32,
                    &mut id as *mut _,
                ) == ffi::egl::FALSE
                {
                    return Err(CreateWindowError::NoVisual.into());
                } else {
                    Some(id as u32)
                }
            },
            ..self
        })
    }

    /// Manually sets the visual id of this window.
    pub fn visual(self, visual_id: u32) -> Self {
        Self {
            visual_id: Some(visual_id),
            ..self
        }
    }

    /// Creates a window using the options specified in the builder.
    pub fn build(self, handle: &X11Handle) -> Result<Window, X11Error> {
        let visual_id = self.visual_id.ok_or(CreateWindowError::NoVisual)?;
        let connection = &handle.connection;
        let screen = &connection.setup().roots[connection.screen];
        let depth = screen
            .allowed_depths
            .iter()
            .filter(|depth| depth.visuals.iter().any(|visual| visual.visual_id == visual_id))
            .find(|depth| depth.depth == 32) // Prefer 32-bit color
            .or_else(
                || {
                    screen
                        .allowed_depths
                        .iter()
                        .filter(|depth| depth.visuals.iter().any(|visual| visual.visual_id == visual_id))
                        .find(|depth| depth.depth == 24)
                }, // 24-bit fallback for Xrgb8888
            )
            .cloned()
            .ok_or(CreateWindowError::NoDepth)?;

        // Make a colormap
        let colormap = connection.generate_id()?;
        connection
            .xcb
            .create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id)?;

        let inner = &mut *handle.inner.lock().unwrap();
        let window = Arc::new(WindowInner::new(
            connection.weak(),
            screen,
            self.size.unwrap_or_else(|| (1280, 800).into()),
            self.name.unwrap_or("Smithay"),
            inner.atoms,
            depth,
            visual_id,
            colormap,
        )?);

        let downgrade = Arc::downgrade(&window);
        inner.windows.insert(window.id, downgrade);

        Ok(Window(window))
    }
}

/// An X11 window.
///
/// Dropping all instances of the window will destroy it.
#[derive(Debug, Clone)]
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
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

struct WindowTemporary(Window);

impl AsRef<Window> for WindowTemporary {
    fn as_ref(&self) -> &Window {
        &self.0
    }
}

impl EventSource for X11Backend {
    type Event = X11Event;

    /// The window the incoming events are applicable to.
    type Metadata = ();

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

atom_manager! {
    pub(crate) Atoms: AtomCollectionCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_NAME,
        UTF8_STRING,
        _SMITHAY_X11_BACKEND_CLOSE,
    }
}

#[derive(Debug)]
pub(crate) struct X11Inner {
    screen_number: usize,
    windows: HashMap<u32, Weak<WindowInner>>,
    key_counter: Arc<AtomicU32>,
    atoms: Atoms,
    devices: bool,
}

impl X11Inner {
    fn window_ref_from_id(inner: &Arc<Mutex<X11Inner>>, id: &u32) -> Option<Weak<WindowInner>> {
        let mut inner = inner.lock().unwrap();
        inner.windows.retain(|_, weak| weak.upgrade().is_some());
        inner.windows.get(id).cloned()
    }

    fn process_event<F>(inner: &Arc<Mutex<X11Inner>>, log: &Logger, event: x11::Event, callback: &mut F)
    where
        F: FnMut(X11Event, &mut ()),
    {
        {
            let mut inner = inner.lock().unwrap();
            if !inner.windows.is_empty() && !inner.devices {
                callback(
                    Input(InputEvent::DeviceAdded {
                        device: X11VirtualDevice,
                    }),
                    &mut (),
                );
                inner.devices = true;
            } else if inner.windows.is_empty() && inner.devices {
                callback(
                    Input(InputEvent::DeviceRemoved {
                        device: X11VirtualDevice,
                    }),
                    &mut (),
                );
                inner.devices = false;
            }
        }

        use self::X11Event::Input;

        // If X11 is deadlocking somewhere here, make sure you drop your mutex guards.

        match dbg!(event) {
            x11::Event::ButtonPress(button_press) => {
                if let Some(window) = X11Inner::window_ref_from_id(inner, &button_press.event) {
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
                                    window,
                                },
                            }),
                            &mut (),
                        )
                    } else {
                        callback(
                            Input(InputEvent::PointerButton {
                                event: X11MouseInputEvent {
                                    time: button_press.time,
                                    raw: button_press.detail as u32,
                                    state: ButtonState::Pressed,
                                    window,
                                },
                            }),
                            &mut (),
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

                if let Some(window) = X11Inner::window_ref_from_id(inner, &button_release.event) {
                    callback(
                        Input(InputEvent::PointerButton {
                            event: X11MouseInputEvent {
                                time: button_release.time,
                                raw: button_release.detail as u32,
                                state: ButtonState::Released,
                                window,
                            },
                        }),
                        &mut (),
                    );
                }
            }

            x11::Event::KeyPress(key_press) => {
                if let Some(window) = X11Inner::window_ref_from_id(inner, &key_press.event) {
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
                                window,
                            },
                        }),
                        &mut (),
                    )
                }
            }

            x11::Event::KeyRelease(key_release) => {
                if let Some(window) = X11Inner::window_ref_from_id(inner, &key_release.event) {
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
                                window,
                            },
                        }),
                        &mut (),
                    );
                }
            }

            x11::Event::MotionNotify(motion_notify) => {
                if let Some(window) =
                    X11Inner::window_ref_from_id(inner, &motion_notify.event).and_then(|w| w.upgrade())
                {
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
                                window: Arc::downgrade(&window),
                            },
                        }),
                        &mut (),
                    )
                }
            }

            x11::Event::ConfigureNotify(configure_notify) => {
                if let Some(window) =
                    X11Inner::window_ref_from_id(inner, &configure_notify.window).and_then(|w| w.upgrade())
                {
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
                            X11Event::Resized {
                                new_size: configure_notify_size,
                                window_id: configure_notify.window,
                            },
                            &mut (),
                        );

                        if let Some(resize_sender) = window.resize.lock().unwrap().as_ref() {
                            let _ = resize_sender.send(configure_notify_size);
                        }
                    }
                }
            }

            x11::Event::EnterNotify(enter_notify) => {
                if let Some(window) =
                    X11Inner::window_ref_from_id(inner, &enter_notify.event).and_then(|w| w.upgrade())
                {
                    window.cursor_enter();
                }
            }

            x11::Event::LeaveNotify(leave_notify) => {
                if let Some(window) =
                    X11Inner::window_ref_from_id(inner, &leave_notify.event).and_then(|w| w.upgrade())
                {
                    window.cursor_leave();
                }
            }

            x11::Event::ClientMessage(client_message) => {
                if let Some(window) =
                    X11Inner::window_ref_from_id(inner, &client_message.window).and_then(|w| w.upgrade())
                {
                    if client_message.data.as_data32()[0] == window.atoms.WM_DELETE_WINDOW
                    // Destroy the window?
                    {
                        (callback)(
                            X11Event::CloseRequested {
                                window_id: client_message.window,
                            },
                            &mut (),
                        );
                    }
                }
            }

            x11::Event::Error(e) => {
                error!(log, "X11 protocol error: {:?}", e);
            }

            _ => (),
        }
    }
}
