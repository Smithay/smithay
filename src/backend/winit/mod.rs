//! Implementation of backend traits for types provided by `winit`
//!
//! This module provides the appropriate implementations of the backend
//! interfaces for running a compositor as a Wayland or X11 client using [`winit`].
//!
//! ## Usage
//!
//! The backend is initialized using one of the [`init`], [`init_from_builder`] or
//! [`init_from_builder_with_gl_attr`] functions, depending on the amount of control
//! you want on the initialization of the backend. These functions will provide you
//! with two objects:
//!
//! - a [`WinitGraphicsBackend`], which can give you an implementation of a [`Renderer`]
//!   (or even [`GlesRenderer`]) through its `renderer` method in addition to further
//!   functionality to access and manage the created winit-window.
//! - a [`WinitEventLoop`], which dispatches some [`WinitEvent`] from the host graphics server.
//!
//! The other types in this module are the instances of the associated types of these
//! two traits for the winit backend.

use std::io::Error as IoError;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::{EventSource, Interest, PostAction, Readiness, Token};
use tracing::{debug, error, info, info_span, instrument, trace, warn};
use wayland_egl as wegl;
use winit::event_loop::EventLoopBuilder;
use winit::platform::pump_events::PumpStatus;
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    dpi::LogicalSize,
    event::{ElementState, Event, Touch, TouchPhase, WindowEvent},
    event_loop::EventLoop,
    platform::pump_events::EventLoopExtPumpEvents,
    window::{Window as WinitWindow, WindowBuilder},
};

use crate::{
    backend::{
        egl::{
            context::{GlAttributes, PixelFormatRequirements},
            display::EGLDisplay,
            native, EGLContext, EGLSurface, Error as EGLError,
        },
        input::InputEvent,
        renderer::{
            gles::{GlesError, GlesRenderer},
            Bind,
        },
    },
    utils::{Physical, Rectangle, Size},
};

mod input;

pub use self::input::*;

use super::renderer::Renderer;

/// Create a new [`WinitGraphicsBackend`], which implements the
/// [`Renderer`] trait and a corresponding [`WinitEventLoop`].
pub fn init<R>() -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<GlesRenderer> + Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
{
    init_from_builder(
        WindowBuilder::new()
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_title("Smithay")
            .with_visible(true),
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`]
/// trait, from a given [`WindowBuilder`] struct and a corresponding
/// [`WinitEventLoop`].
pub fn init_from_builder<R>(
    builder: WindowBuilder,
) -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<GlesRenderer> + Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
{
    init_from_builder_with_gl_attr(
        builder,
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: false,
        },
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`]
/// trait, from a given [`WindowBuilder`] struct, as well as given
/// [`GlAttributes`] for further customization of the rendering pipeline and a
/// corresponding [`WinitEventLoop`].
pub fn init_from_builder_with_gl_attr<R>(
    builder: WindowBuilder,
    attributes: GlAttributes,
) -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<GlesRenderer> + Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
{
    let span = info_span!("backend_winit", window = tracing::field::Empty);
    let _guard = span.enter();
    info!("Initializing a winit backend");

    let event_loop = EventLoopBuilder::new()
        .build()
        .map_err(Error::EventLoopCreation)?;

    let window = Arc::new(builder.build(&event_loop).map_err(Error::WindowCreation)?);

    span.record("window", Into::<u64>::into(window.id()));
    debug!("Window created");

    let (display, context, surface, is_x11) = {
        let display = unsafe { EGLDisplay::new(window.clone())? };

        let context = EGLContext::new_with_config(&display, attributes, PixelFormatRequirements::_10_bit())
            .or_else(|_| {
            EGLContext::new_with_config(&display, attributes, PixelFormatRequirements::_8_bit())
        })?;

        let (surface, is_x11) = match window.window_handle().map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Wayland(handle)) => {
                debug!("Winit backend: Wayland");
                let size = window.inner_size();
                let surface = unsafe {
                    wegl::WlEglSurface::new_from_raw(
                        handle.surface.as_ptr() as *mut _,
                        size.width as i32,
                        size.height as i32,
                    )
                }
                .map_err(|err| Error::Surface(err.into()))?;
                unsafe {
                    (
                        EGLSurface::new(
                            &display,
                            context.pixel_format().unwrap(),
                            context.config_id(),
                            surface,
                        )
                        .map_err(EGLError::CreationFailed)?,
                        false,
                    )
                }
            }
            Ok(RawWindowHandle::Xlib(handle)) => {
                debug!("Winit backend: X11");
                unsafe {
                    (
                        EGLSurface::new(
                            &display,
                            context.pixel_format().unwrap(),
                            context.config_id(),
                            native::XlibWindow(handle.window),
                        )
                        .map_err(EGLError::CreationFailed)?,
                        true,
                    )
                }
            }
            _ => panic!("only running on Wayland or with Xlib is supported"),
        };

        let _ = context.unbind();
        (display, context, surface, is_x11)
    };

    let egl = Rc::new(surface);
    let renderer = unsafe { GlesRenderer::new(context)?.into() };
    let damage_tracking = display.supports_damage();

    drop(_guard);

    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let event_loop = Generic::new(event_loop, Interest::READ, calloop::Mode::Level);

    Ok((
        WinitGraphicsBackend {
            window: window.clone(),
            span: span.clone(),
            _display: display,
            egl_surface: egl,
            damage_tracking,
            bind_size: None,
            renderer,
        },
        WinitEventLoop {
            scale_factor: window.scale_factor(),
            start_time: Instant::now(),
            key_counter: 0,
            fake_token: None,
            event_loop,
            window,
            pending_events: Vec::new(),
            is_x11,
            span,
        },
    ))
}

/// Errors thrown by the `winit` backends
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failed to initialize an event loop.
    #[error("Failed to initialize an event loop")]
    EventLoopCreation(#[from] winit::error::EventLoopError),
    /// Failed to initialize a window.
    #[error("Failed to initialize a window")]
    WindowCreation(#[from] winit::error::OsError),
    #[error("Failed to create a surface for the window")]
    /// Surface creation error.
    Surface(Box<dyn std::error::Error>),
    /// Context creation is not supported on the current window system
    #[error("Context creation is not supported on the current window system")]
    NotSupported,
    /// EGL error.
    #[error("EGL error: {0}")]
    Egl(#[from] EGLError),
    /// Renderer initialization failed.
    #[error("Renderer creation failed: {0}")]
    RendererCreationError(#[from] GlesError),
}

/// Window with an active EGL Context created by `winit`.
#[derive(Debug)]
pub struct WinitGraphicsBackend<R> {
    renderer: R,
    // The display isn't used past this point but must be kept alive.
    _display: EGLDisplay,
    egl_surface: Rc<EGLSurface>,
    window: Arc<WinitWindow>,
    damage_tracking: bool,
    bind_size: Option<Size<i32, Physical>>,
    span: tracing::Span,
}

impl<R> WinitGraphicsBackend<R>
where
    R: Bind<Rc<EGLSurface>>,
    crate::backend::SwapBuffersError: From<<R as Renderer>::Error>,
{
    /// Window size of the underlying window
    pub fn window_size(&self) -> Size<i32, Physical> {
        let (w, h): (i32, i32) = self.window.inner_size().into();
        (w, h).into()
    }

    /// Scale factor of the underlying window.
    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    /// Reference to the underlying window
    pub fn window(&self) -> &WinitWindow {
        &self.window
    }

    /// Access the underlying renderer
    pub fn renderer(&mut self) -> &mut R {
        &mut self.renderer
    }

    /// Bind the underlying window to the underlying renderer.
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    pub fn bind(&mut self) -> Result<(), crate::backend::SwapBuffersError> {
        // NOTE: we must resize before making the current context current, otherwise the back
        // buffer will be latched. Some nvidia drivers may not like it, but a lot of wayland
        // software does the order that way due to mesa latching back buffer on each
        // `make_current`.
        let window_size = self.window_size();
        if Some(window_size) != self.bind_size {
            self.egl_surface.resize(window_size.w, window_size.h, 0, 0);
        }
        self.bind_size = Some(window_size);

        self.renderer.bind(self.egl_surface.clone())?;

        Ok(())
    }

    /// Retrieve the underlying `EGLSurface` for advanced operations
    ///
    /// **Note:** Don't carelessly use this to manually bind the renderer to the surface,
    /// `WinitGraphicsBackend::bind` transparently handles window resizes for you.
    pub fn egl_surface(&self) -> Rc<EGLSurface> {
        self.egl_surface.clone()
    }

    /// Retrieve the buffer age of the current backbuffer of the window.
    ///
    /// This will only return a meaningful value, if this `WinitGraphicsBackend`
    /// is currently bound (by previously calling [`WinitGraphicsBackend::bind`]).
    ///
    /// Otherwise and on error this function returns `None`.
    /// If you are using this value actively e.g. for damage-tracking you should
    /// likely interpret an error just as if "0" was returned.
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    pub fn buffer_age(&self) -> Option<usize> {
        if self.damage_tracking {
            self.egl_surface.buffer_age().map(|x| x as usize)
        } else {
            Some(0)
        }
    }

    /// Submits the back buffer to the window by swapping, requires the window to be previously
    /// bound (see [`WinitGraphicsBackend::bind`]).
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    pub fn submit(
        &mut self,
        damage: Option<&[Rectangle<i32, Physical>]>,
    ) -> Result<(), crate::backend::SwapBuffersError> {
        let mut damage = match damage {
            Some(damage) if self.damage_tracking && !damage.is_empty() => {
                let bind_size = self
                    .bind_size
                    .expect("submitting without ever binding the renderer.");
                let damage = damage
                    .iter()
                    .map(|rect| {
                        Rectangle::from_loc_and_size(
                            (rect.loc.x, bind_size.h - rect.loc.y - rect.size.h),
                            rect.size,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(damage)
            }
            _ => None,
        };

        // Request frame callback.
        self.window.pre_present_notify();
        self.egl_surface.swap_buffers(damage.as_deref_mut())?;
        Ok(())
    }
}

/// Abstracted event loop of a [`WinitWindow`].
///
/// You can register it into `calloop` or call
/// [`dispatch_new_events`](WinitEventLoop::dispatch_new_events) periodically to receive any
/// events.
#[derive(Debug)]
pub struct WinitEventLoop {
    window: Arc<WinitWindow>,
    start_time: Instant,
    fake_token: Option<Token>,
    key_counter: u32,
    pending_events: Vec<WinitEvent>,
    is_x11: bool,
    scale_factor: f64,
    event_loop: Generic<EventLoop<()>>,
    span: tracing::Span,
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
    #[instrument(level = "trace", parent = &self.span, skip_all)]
    #[profiling::function]
    pub fn dispatch_new_events<F>(&mut self, mut callback: F) -> PumpStatus
    where
        F: FnMut(WinitEvent),
    {
        // SAFETY: we don't drop event loop ourselves.
        let event_loop = unsafe { self.event_loop.get_mut() };
        let timestamp = || self.start_time.elapsed().as_micros() as u64;

        event_loop.pump_events(Some(Duration::ZERO), |event, _window_target| {
            match event {
                Event::Resumed => {
                    callback(WinitEvent::Input(InputEvent::DeviceAdded {
                        device: WinitVirtualDevice,
                    }));
                }
                Event::UserEvent(_) => (),
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::Resized(size) => {
                        trace!("Resizing window to {size:?}");
                        let (w, h): (i32, i32) = size.into();

                        callback(WinitEvent::Resized {
                            size: (w, h).into(),
                            scale_factor: self.scale_factor,
                        });
                    }
                    WindowEvent::ScaleFactorChanged {
                        scale_factor: new_scale_factor,
                        ..
                    } => {
                        trace!("Scale factor changed to {new_scale_factor}");
                        self.scale_factor = new_scale_factor;
                        let (w, h): (i32, i32) = self.window.inner_size().into();
                        callback(WinitEvent::Resized {
                            size: (w, h).into(),
                            scale_factor: self.scale_factor,
                        });
                    }
                    WindowEvent::RedrawRequested => {
                        callback(WinitEvent::Redraw);
                    }
                    WindowEvent::CloseRequested => {
                        callback(WinitEvent::CloseRequested);
                    }
                    WindowEvent::Focused(focused) => {
                        callback(WinitEvent::Focus(focused));
                    }
                    WindowEvent::KeyboardInput {
                        event, is_synthetic, ..
                    } if !is_synthetic && !event.repeat => {
                        match event.state {
                            ElementState::Pressed => self.key_counter += 1,
                            ElementState::Released => {
                                self.key_counter = self.key_counter.saturating_sub(1);
                            }
                        };

                        let scancode = event.physical_key.to_scancode().unwrap_or(0);
                        let event = InputEvent::Keyboard {
                            event: WinitKeyboardInputEvent {
                                time: timestamp(),
                                key: scancode,
                                count: self.key_counter,
                                state: event.state,
                            },
                        };
                        callback(WinitEvent::Input(event));
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        let size = self.window.inner_size();
                        let x = position.x / size.width as f64;
                        let y = position.y / size.height as f64;
                        let event = InputEvent::PointerMotionAbsolute {
                            event: WinitMouseMovedEvent {
                                time: timestamp(),
                                position: RelativePosition::new(x, y),
                                global_position: position,
                            },
                        };
                        callback(WinitEvent::Input(event));
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let event = InputEvent::PointerAxis {
                            event: WinitMouseWheelEvent {
                                time: timestamp(),
                                delta,
                            },
                        };
                        callback(WinitEvent::Input(event));
                    }
                    WindowEvent::MouseInput { state, button, .. } => {
                        let event = InputEvent::PointerButton {
                            event: WinitMouseInputEvent {
                                time: timestamp(),
                                button,
                                state,
                                is_x11: self.is_x11,
                            },
                        };
                        callback(WinitEvent::Input(event));
                    }
                    WindowEvent::Touch(Touch {
                        phase: TouchPhase::Started,
                        location,
                        id,
                        ..
                    }) => {
                        let size = self.window.inner_size();
                        let x = location.x / size.width as f64;
                        let y = location.y / size.width as f64;
                        let event = InputEvent::TouchDown {
                            event: WinitTouchStartedEvent {
                                time: timestamp(),
                                global_position: location,
                                position: RelativePosition::new(x, y),
                                id,
                            },
                        };

                        callback(WinitEvent::Input(event));
                    }
                    WindowEvent::Touch(Touch {
                        phase: TouchPhase::Moved,
                        location,
                        id,
                        ..
                    }) => {
                        let size = self.window.inner_size();
                        let x = location.x / size.width as f64;
                        let y = location.y / size.width as f64;
                        let event = InputEvent::TouchMotion {
                            event: WinitTouchMovedEvent {
                                time: timestamp(),
                                position: RelativePosition::new(x, y),
                                global_position: location,
                                id,
                            },
                        };

                        callback(WinitEvent::Input(event));
                    }

                    WindowEvent::Touch(Touch {
                        phase: TouchPhase::Ended,
                        location,
                        id,
                        ..
                    }) => {
                        let size = self.window.inner_size();
                        let x = location.x / size.width as f64;
                        let y = location.y / size.width as f64;
                        let event = InputEvent::TouchMotion {
                            event: WinitTouchMovedEvent {
                                time: timestamp(),
                                position: RelativePosition::new(x, y),
                                global_position: location,
                                id,
                            },
                        };
                        callback(WinitEvent::Input(event));

                        let event = InputEvent::TouchUp {
                            event: WinitTouchEndedEvent {
                                time: timestamp(),
                                id,
                            },
                        };

                        callback(WinitEvent::Input(event));
                    }

                    WindowEvent::Touch(Touch {
                        phase: TouchPhase::Cancelled,
                        id,
                        ..
                    }) => {
                        let event = InputEvent::TouchCancel {
                            event: WinitTouchCancelledEvent {
                                time: timestamp(),
                                id,
                            },
                        };
                        callback(WinitEvent::Input(event));
                    }
                    WindowEvent::DroppedFile(_)
                    | WindowEvent::Destroyed
                    | WindowEvent::CursorEntered { .. }
                    | WindowEvent::AxisMotion { .. }
                    | WindowEvent::CursorLeft { .. }
                    | WindowEvent::ModifiersChanged(_)
                    | WindowEvent::KeyboardInput { .. }
                    | WindowEvent::HoveredFile(_)
                    | WindowEvent::HoveredFileCancelled
                    | WindowEvent::Ime(_)
                    | WindowEvent::Moved(_)
                    | WindowEvent::Occluded(_)
                    | WindowEvent::SmartMagnify { .. }
                    | WindowEvent::ThemeChanged(_)
                    | WindowEvent::TouchpadMagnify { .. }
                    | WindowEvent::TouchpadPressure { .. }
                    | WindowEvent::TouchpadRotate { .. }
                    | WindowEvent::ActivationTokenDone { .. } => (),
                },
                Event::NewEvents(_)
                | Event::DeviceEvent { .. }
                | Event::MemoryWarning
                | Event::Suspended
                | Event::AboutToWait
                | Event::LoopExiting => (),
            };
        })
    }
}

impl EventSource for WinitEventLoop {
    type Event = WinitEvent;
    type Metadata = ();
    type Ret = ();
    type Error = IoError;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    fn before_sleep(&mut self) -> calloop::Result<Option<(calloop::Readiness, calloop::Token)>> {
        let mut pending_events = std::mem::take(&mut self.pending_events);
        let callback = |event| {
            pending_events.push(event);
        };
        // NOTE: drain winit's event loop before going to sleep, so we can
        // wake up if other thread has woken up underlying winit loop, like other
        // event queue got dispatched during that.
        self.dispatch_new_events(callback);
        self.pending_events = pending_events;
        if self.pending_events.is_empty() {
            Ok(None)
        } else {
            Ok(Some((Readiness::EMPTY, self.fake_token.unwrap())))
        }
    }

    fn process_events<F>(
        &mut self,
        _readiness: calloop::Readiness,
        _token: calloop::Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let mut callback = |event| callback(event, &mut ());
        for event in self.pending_events.drain(..) {
            callback(event);
        }
        Ok(match self.dispatch_new_events(callback) {
            PumpStatus::Continue => PostAction::Continue,
            PumpStatus::Exit(_) => PostAction::Remove,
        })
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.fake_token = Some(token_factory.token());
        self.event_loop.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.event_loop.register(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        self.event_loop.unregister(poll)
    }
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

    /// The user requested to close the window.
    CloseRequested,

    /// A redraw was requested
    Redraw,
}
