//! Implementation of backend traits for types provided by `winit`
//!
//! This module provides the appropriate implementations of the backend
//! interfaces for running a compositor as a Wayland of X11 client using [`winit`].
//!
//! ## Usage
//!
//! The backend is initialized using of of the [`init`], [`init_from_builder`] or
//! [`init_from_builder_with_gl_attr`] functions, depending on the amount of control
//! you want on the initialization of the backend. These functions will provide you
//! with two objects:
//!
//! - a [`WinitGraphicsBackend`], which can give you an implementation of a
//!   [`Renderer`](crate::backend::renderer::Renderer)
//!   (or even [`Gles2Renderer`]) through its `renderer` method in addition to further
//!   functionality to access and manage the created winit-window.
//! - a [`WinitEventLoop`], which dispatches some [`WinitEvent`] from the host graphics server.
//!
//! The other types in this module are the instances of the associated types of these
//! two traits for the winit backend.

mod input;

use crate::{
    backend::{
        egl::{
            context::GlAttributes, display::EGLDisplay, native, EGLContext, EGLSurface, Error as EGLError,
        },
        input::InputEvent,
        renderer::{
            gles2::{Gles2Error, Gles2Renderer},
            Bind,
        },
    },
    utils::{Logical, Physical, Rectangle, Size},
};
use std::{cell::RefCell, rc::Rc, time::Instant};
use wayland_egl as wegl;
use winit::{
    dpi::LogicalSize,
    event::{ElementState, Event, KeyboardInput, Touch, TouchPhase, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::run_return::EventLoopExtRunReturn,
    platform::unix::WindowExtUnix,
    window::{Window as WinitWindow, WindowBuilder},
};

use slog::{debug, error, info, o, trace, warn};
use std::cell::Cell;

pub use self::input::*;

use super::renderer::Renderer;

/// Errors thrown by the `winit` backends
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failed to initialize a window
    #[error("Failed to initialize a window")]
    InitFailed(#[from] winit::error::OsError),
    #[error("Failed to create a surface for the window")]
    /// Surface creation error
    Surface(Box<dyn std::error::Error>),
    /// Context creation is not supported on the current window system
    #[error("Context creation is not supported on the current window system")]
    NotSupported,
    /// EGL error
    #[error("EGL error: {0}")]
    Egl(#[from] EGLError),
    /// Renderer initialization failed
    #[error("Renderer creation failed: {0}")]
    RendererCreationError(#[from] Gles2Error),
}

/// Size properties of a winit window
#[derive(Debug, Clone)]
pub struct WindowSize {
    /// Pixel side of the window
    pub physical_size: Size<i32, Physical>,
    /// Scaling factor of the window
    pub scale_factor: f64,
}

impl WindowSize {
    fn logical_size(&self) -> Size<f64, Logical> {
        self.physical_size.to_f64().to_logical(self.scale_factor)
    }
}

/// Window with an active EGL Context created by `winit`.
#[derive(Debug)]
pub struct WinitGraphicsBackend<R> {
    renderer: R,
    // The display isn't used past this point but must be kept alive.
    _display: EGLDisplay,
    egl: Rc<EGLSurface>,
    window: Rc<WinitWindow>,
    size: Rc<RefCell<WindowSize>>,
    damage_tracking: bool,
    resize_notification: Rc<Cell<Option<Size<i32, Physical>>>>,
}

/// Abstracted event loop of a [`WinitWindow`].
///
/// You need to call [`dispatch_new_events`](WinitEventLoop::dispatch_new_events)
/// periodically to receive any events.
#[derive(Debug)]
pub struct WinitEventLoop {
    window: Rc<WinitWindow>,
    events_loop: EventLoop<()>,
    time: Instant,
    key_counter: u32,
    logger: ::slog::Logger,
    initialized: bool,
    size: Rc<RefCell<WindowSize>>,
    resize_notification: Rc<Cell<Option<Size<i32, Physical>>>>,
    /// Whether winit is using Wayland or X11 as it's backend.
    is_x11: bool,
}

/// Create a new [`WinitGraphicsBackend`], which implements the
/// [`Renderer`](crate::backend::renderer::Renderer) trait and a corresponding
/// [`WinitEventLoop`].
pub fn init<R, L>(logger: L) -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<Gles2Renderer> + Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
    L: Into<Option<::slog::Logger>>,
{
    init_from_builder(
        WindowBuilder::new()
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_title("Smithay")
            .with_visible(true),
        logger,
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the
/// [`Renderer`](crate::backend::renderer::Renderer) trait, from a given [`WindowBuilder`]
/// struct and a corresponding [`WinitEventLoop`].
pub fn init_from_builder<R, L>(
    builder: WindowBuilder,
    logger: L,
) -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<Gles2Renderer> + Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
    L: Into<Option<::slog::Logger>>,
{
    init_from_builder_with_gl_attr(
        builder,
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: true,
        },
        logger,
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the
/// [`Renderer`](crate::backend::renderer::Renderer) trait, from a given [`WindowBuilder`]
/// struct, as well as given [`GlAttributes`] for further customization of the rendering pipeline and a
/// corresponding [`WinitEventLoop`].
pub fn init_from_builder_with_gl_attr<R, L>(
    builder: WindowBuilder,
    attributes: GlAttributes,
    logger: L,
) -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<Gles2Renderer> + Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
    L: Into<Option<::slog::Logger>>,
{
    let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_winit"));
    info!(log, "Initializing a winit backend");

    let events_loop = EventLoop::new();
    let winit_window = builder.build(&events_loop).map_err(Error::InitFailed)?;

    debug!(log, "Window created");

    let reqs = Default::default();
    let (display, context, surface, is_x11) = {
        let display = unsafe { EGLDisplay::new(&winit_window, log.clone())? };
        let context = EGLContext::new_with_config(&display, attributes, reqs, log.clone())?;

        let (surface, is_x11) = if let Some(wl_surface) = winit_window.wayland_surface() {
            debug!(log, "Winit backend: Wayland");
            let size = winit_window.inner_size();
            let surface = unsafe {
                wegl::WlEglSurface::new_from_raw(wl_surface as *mut _, size.width as i32, size.height as i32)
            }
            .map_err(|err| Error::Surface(err.into()))?;
            (
                EGLSurface::new(
                    &display,
                    context.pixel_format().unwrap(),
                    context.config_id(),
                    surface,
                    log.clone(),
                )
                .map_err(EGLError::CreationFailed)?,
                false,
            )
        } else if let Some(xlib_window) = winit_window.xlib_window().map(native::XlibWindow) {
            debug!(log, "Winit backend: X11");
            (
                EGLSurface::new(
                    &display,
                    context.pixel_format().unwrap(),
                    context.config_id(),
                    xlib_window,
                    log.clone(),
                )
                .map_err(EGLError::CreationFailed)?,
                true,
            )
        } else {
            unreachable!("No backends for winit other then Wayland and X11 are supported")
        };

        let _ = context.unbind();

        (display, context, surface, is_x11)
    };

    let (w, h): (u32, u32) = winit_window.inner_size().into();
    let size = Rc::new(RefCell::new(WindowSize {
        physical_size: (w as i32, h as i32).into(),
        scale_factor: winit_window.scale_factor(),
    }));

    let window = Rc::new(winit_window);
    let egl = Rc::new(surface);
    let renderer = unsafe { Gles2Renderer::new(context, log.clone())?.into() };
    let resize_notification = Rc::new(Cell::new(None));
    let damage_tracking = display.supports_damage();

    Ok((
        WinitGraphicsBackend {
            window: window.clone(),
            _display: display,
            egl,
            renderer,
            damage_tracking,
            size: size.clone(),
            resize_notification: resize_notification.clone(),
        },
        WinitEventLoop {
            resize_notification,
            events_loop,
            window,
            time: Instant::now(),
            key_counter: 0,
            initialized: false,
            logger: log.new(o!("smithay_winit_component" => "event_loop")),
            size,
            is_x11,
        },
    ))
}

/// Specific events generated by Winit
#[derive(Debug)]
pub enum WinitEvent {
    /// The window has been resized
    Resized {
        /// The new physical size (in pixels)
        size: Size<i32, Physical>,
        /// The new scale factor
        scale_factor: f64,
    },

    /// The focus state of the window changed
    Focus(bool),

    /// An input event occurred.
    Input(InputEvent<WinitInput>),

    /// A redraw was requested
    Refresh,
}

impl<R> WinitGraphicsBackend<R>
where
    R: Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
{
    /// Window size of the underlying window
    pub fn window_size(&self) -> WindowSize {
        self.size.borrow().clone()
    }

    /// Reference to the underlying window
    pub fn window(&self) -> &WinitWindow {
        &self.window
    }

    /// Access the underlying renderer
    pub fn renderer(&mut self) -> &mut R {
        &mut self.renderer
    }

    /// Bind the underlying window to the underlying renderer
    pub fn bind(&mut self) -> Result<(), crate::backend::SwapBuffersError> {
        // apparently the nvidia-driver doesn't like `wl_egl_window_resize`, if the surface is not current.
        // So the order here is important.
        self.renderer.bind(self.egl.clone())?;

        // Were we told to resize?
        if let Some(size) = self.resize_notification.take() {
            self.egl.resize(size.w, size.h, 0, 0);
        }

        Ok(())
    }

    /// Retrieve the underlying `EGLSurface` for advanced operations
    ///
    /// **Note:** Don't carelessly use this to manually bind the renderer to the surface,
    /// `WinitGraphicsBackend::bind` transparently handles window resizes for you.
    pub fn egl_surface(&self) -> Rc<EGLSurface> {
        self.egl.clone()
    }

    /// Retrieve the buffer age of the current backbuffer of the window.
    ///
    /// This will only return a meaningful value, if this `WinitGraphicsBackend`
    /// is currently bound (by previously calling [`WinitGraphicsBackend::bind`]).
    ///
    /// Otherwise and on error this function returns `None`.
    /// If you are using this value actively e.g. for damage-tracking you should
    /// likely interpret an error just as if "0" was returned.
    pub fn buffer_age(&self) -> Option<usize> {
        if self.damage_tracking {
            self.egl.buffer_age().map(|x| x as usize)
        } else {
            Some(0)
        }
    }

    /// Submits the back buffer to the window by swapping, requires the window to be previously bound (see [`WinitGraphicsBackend::bind`]).
    pub fn submit(
        &mut self,
        damage: Option<&[Rectangle<i32, Physical>]>,
    ) -> Result<(), crate::backend::SwapBuffersError> {
        let mut damage = match damage {
            Some(damage) if self.damage_tracking && !damage.is_empty() => {
                let size = self.size.borrow().physical_size;
                let damage = damage
                    .iter()
                    .map(|rect| {
                        Rectangle::from_loc_and_size(
                            (rect.loc.x, size.h - rect.loc.y - rect.size.h),
                            rect.size,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(damage)
            }
            _ => None,
        };
        self.egl.swap_buffers(damage.as_deref_mut())?;
        Ok(())
    }
}

/// Errors that may happen when driving a [`WinitEventLoop`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum WinitError {
    /// The underlying [`WinitWindow`] was closed. No further events can be processed.
    ///
    /// See `dispatch_new_events`.
    #[error("Winit window was closed")]
    WindowClosed,
}

impl WinitEventLoop {
    /// Processes new events of the underlying event loop and calls the provided callback.
    ///
    /// You need to periodically call this function to keep the underlying event loop and
    /// [`WinitWindow`] active. Otherwise the window may not respond to user interaction.
    ///
    /// Returns an error if the [`WinitWindow`] the window has been closed. Calling
    /// `dispatch_new_events` again after the [`WinitWindow`] has been closed is considered an
    /// application error and unspecified behaviour may occur.
    ///
    /// The linked [`WinitGraphicsBackend`] will error with a lost context and should
    /// not be used anymore as well.
    pub fn dispatch_new_events<F>(&mut self, mut callback: F) -> Result<(), WinitError>
    where
        F: FnMut(WinitEvent),
    {
        use self::WinitEvent::*;

        let mut closed = false;

        {
            // NOTE: This ugly pile of references is here, because rustc could not
            // figure out how to reference all these objects correctly into the
            // upcoming closure, which is why all are borrowed manually and the
            // assignments are then moved into the closure to avoid rustc's
            // wrong interference.
            let closed_ptr = &mut closed;
            let key_counter = &mut self.key_counter;
            let time = &self.time;
            let window = &self.window;
            let resize_notification = &self.resize_notification;
            let logger = &self.logger;
            let window_size = &self.size;
            let is_x11 = self.is_x11;

            if !self.initialized {
                callback(Input(InputEvent::DeviceAdded {
                    device: WinitVirtualDevice,
                }));
                self.initialized = true;
            }

            self.events_loop
                .run_return(move |event, _target, control_flow| match event {
                    Event::RedrawEventsCleared => {
                        *control_flow = ControlFlow::Exit;
                    }
                    Event::RedrawRequested(_id) => {
                        callback(WinitEvent::Refresh);
                    }
                    Event::WindowEvent { event, .. } => {
                        let duration = Instant::now().duration_since(*time);
                        let nanos = duration.subsec_nanos() as u64;
                        let time = ((1000 * duration.as_secs()) + (nanos / 1_000_000)) as u32;
                        match event {
                            WindowEvent::Resized(psize) => {
                                trace!(logger, "Resizing window to {:?}", psize);
                                let scale_factor = window.scale_factor();
                                let mut wsize = window_size.borrow_mut();
                                let (pw, ph): (u32, u32) = psize.into();
                                wsize.physical_size = (pw as i32, ph as i32).into();
                                wsize.scale_factor = scale_factor;

                                resize_notification.set(Some(wsize.physical_size));

                                callback(WinitEvent::Resized {
                                    size: wsize.physical_size,
                                    scale_factor,
                                });
                            }
                            WindowEvent::Focused(focus) => {
                                callback(WinitEvent::Focus(focus));
                            }

                            WindowEvent::ScaleFactorChanged {
                                scale_factor,
                                new_inner_size: new_psize,
                            } => {
                                let mut wsize = window_size.borrow_mut();
                                wsize.scale_factor = scale_factor;

                                let (pw, ph): (u32, u32) = (*new_psize).into();
                                resize_notification.set(Some((pw as i32, ph as i32).into()));

                                callback(WinitEvent::Resized {
                                    size: (pw as i32, ph as i32).into(),
                                    scale_factor: wsize.scale_factor,
                                });
                            }
                            WindowEvent::KeyboardInput {
                                input: KeyboardInput { scancode, state, .. },
                                ..
                            } => {
                                match state {
                                    ElementState::Pressed => *key_counter += 1,
                                    ElementState::Released => {
                                        *key_counter = key_counter.checked_sub(1).unwrap_or(0)
                                    }
                                };
                                callback(Input(InputEvent::Keyboard {
                                    event: WinitKeyboardInputEvent {
                                        time,
                                        key: scancode,
                                        count: *key_counter,
                                        state,
                                    },
                                }));
                            }
                            WindowEvent::CursorMoved { position, .. } => {
                                let lpos = position.to_logical(window_size.borrow().scale_factor);
                                callback(Input(InputEvent::PointerMotionAbsolute {
                                    event: WinitMouseMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        logical_position: lpos,
                                    },
                                }));
                            }
                            WindowEvent::MouseWheel { delta, .. } => {
                                let event = WinitMouseWheelEvent { time, delta };
                                callback(Input(InputEvent::PointerAxis { event }));
                            }
                            WindowEvent::MouseInput { state, button, .. } => {
                                callback(Input(InputEvent::PointerButton {
                                    event: WinitMouseInputEvent {
                                        time,
                                        button,
                                        state,
                                        is_x11,
                                    },
                                }));
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Started,
                                location,
                                id,
                                ..
                            }) => {
                                let location = location.to_logical(window_size.borrow().scale_factor);
                                callback(Input(InputEvent::TouchDown {
                                    event: WinitTouchStartedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location,
                                        id,
                                    },
                                }));
                            }
                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Moved,
                                location,
                                id,
                                ..
                            }) => {
                                let location = location.to_logical(window_size.borrow().scale_factor);
                                callback(Input(InputEvent::TouchMotion {
                                    event: WinitTouchMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location,
                                        id,
                                    },
                                }));
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Ended,
                                location,
                                id,
                                ..
                            }) => {
                                let location = location.to_logical(window_size.borrow().scale_factor);
                                callback(Input(InputEvent::TouchMotion {
                                    event: WinitTouchMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location,
                                        id,
                                    },
                                }));
                                callback(Input(InputEvent::TouchUp {
                                    event: WinitTouchEndedEvent { time, id },
                                }))
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Cancelled,
                                id,
                                ..
                            }) => {
                                callback(Input(InputEvent::TouchCancel {
                                    event: WinitTouchCancelledEvent { time, id },
                                }));
                            }
                            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                                callback(Input(InputEvent::DeviceRemoved {
                                    device: WinitVirtualDevice,
                                }));
                                warn!(logger, "Window closed");
                                *closed_ptr = true;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                });
        }

        if closed {
            Err(WinitError::WindowClosed)
        } else {
            Ok(())
        }
    }
}
