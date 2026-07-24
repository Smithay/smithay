//! Implementation of backend traits for types provided by `winit`
//!
//! This module provides the appropriate implementations of the backend
//! interfaces for running a compositor as a Wayland or X11 client using [`winit`].
//!
//! ## Usage
//!
//! The backend is initialized using one of the [`init`], [`init_from_attributes`] or
//! [`init_from_attributes_with_gl_attr`] functions, depending on the amount of control
//! you want on the initialization of the backend. These functions will provide you
//! with two objects:
//!
//! - a [`WinitGraphicsBackend`], which can give you an implementation of a [`Renderer`](crate::backend::renderer::Renderer)
//!   (or even [`GlesRenderer`]) through its `renderer` method in addition to further
//!   functionality to access and manage the created winit-window.
//! - a [`WinitEventLoop`], which dispatches some [`WinitEvent`] from the host graphics server.
//!
//! The other types in this module are the instances of the associated types of these
//! two traits for the winit backend.

use std::io::Error as IoError;
use std::sync::Arc;
use std::time::Duration;

use calloop::generic::Generic;
use calloop::{EventSource, Interest, PostAction, Readiness, Token};
use tracing::{debug, info, info_span, instrument, trace, warn};
use wayland_egl as wegl;
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ButtonSource, ElementState, PointerKind, PointerSource, WindowEvent},
    event_loop::{
        ActiveEventLoop, EventLoop,
        pump_events::{EventLoopExtPumpEvents, PumpStatus},
    },
    window::{Window as WinitWindow, WindowAttributes, WindowId},
};

use crate::{
    backend::{
        egl::{
            EGLContext, EGLSurface, Error as EGLError,
            context::{GlAttributes, PixelFormatRequirements},
            display::EGLDisplay,
            native,
        },
        input::{InputEvent, InputTime},
        renderer::{
            Bind,
            gles::{GlesError, GlesRenderer},
        },
    },
    utils::{Clock, Monotonic, Physical, Rectangle, Size},
};

mod input;

pub use self::input::*;

/// Create a new [`WinitGraphicsBackend`], which implements the
/// [`Renderer`](crate::backend::renderer::Renderer) trait and a corresponding [`WinitEventLoop`].
pub fn init<R>() -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<GlesRenderer> + Bind<EGLSurface>,
    crate::backend::SwapBuffersError: From<R::Error>,
{
    init_from_attributes(
        WindowAttributes::default()
            .with_surface_size(LogicalSize::new(1280.0, 800.0))
            .with_title("Smithay")
            .with_visible(true),
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`](crate::backend::renderer::Renderer)
/// trait, from a given [`WindowAttributes`] struct and a corresponding
/// [`WinitEventLoop`].
pub fn init_from_attributes<R>(
    attributes: WindowAttributes,
) -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<GlesRenderer> + Bind<EGLSurface>,
    crate::backend::SwapBuffersError: From<R::Error>,
{
    init_from_attributes_with_gl_attr(
        attributes,
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: false,
        },
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`](crate::backend::renderer::Renderer)
/// trait, from a given [`WindowAttributes`] struct, as well as given
/// [`GlAttributes`] for further customization of the rendering pipeline and a
/// corresponding [`WinitEventLoop`].
pub fn init_from_attributes_with_gl_attr<R>(
    attributes: WindowAttributes,
    gl_attributes: GlAttributes,
) -> Result<(WinitGraphicsBackend<R>, WinitEventLoop), Error>
where
    R: From<GlesRenderer> + Bind<EGLSurface>,
    crate::backend::SwapBuffersError: From<R::Error>,
{
    let span = info_span!("backend_winit", window = tracing::field::Empty);
    let _guard = span.enter();
    info!("Initializing a winit backend");

    let mut event_loop = EventLoop::builder().build().map_err(Error::EventLoopCreation)?;

    let mut window_event_loop_inner = WinitEventLoopInner {
        window_attributes: attributes,
        scale_factor: 1.0,
        clock: Clock::<Monotonic>::new(),
        key_counter: 0,
        // Will be initialized after window creation
        window: None,
        window_create_error: None,
        is_x11: false,
    };
    let mut initial_events = Vec::new();
    while window_event_loop_inner.window.is_none() {
        event_loop.pump_app_events(
            None,
            &mut WinitEventLoopApp {
                inner: &mut window_event_loop_inner,
                callback: |evt| initial_events.push(evt),
            },
        );
        if let Some(err) = window_event_loop_inner.window_create_error {
            return Err(err);
        }
    }
    let window = window_event_loop_inner.window.clone().unwrap();

    window_event_loop_inner.is_x11 = matches!(
        window.window_handle().map(|handle| handle.as_raw()),
        Ok(RawWindowHandle::Xlib(_))
    );

    span.record("window", window.id().into_raw());
    debug!("Window created");

    let winit_graphics_backend =
        WinitGraphicsBackend::new_with_gl_attr(window.clone(), &span, gl_attributes)?;

    drop(_guard);

    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let event_loop = Generic::new(event_loop, Interest::READ, calloop::Mode::Level);

    Ok((
        winit_graphics_backend,
        WinitEventLoop {
            inner: window_event_loop_inner,
            fake_token: None,
            event_loop,
            initial_events,
            pending_events: Vec::new(),
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
    WindowCreation(#[from] winit::error::RequestError),
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
    egl_surface: EGLSurface,
    window: Arc<dyn WinitWindow>,
    damage_tracking: bool,
    bind_size: Option<Size<i32, Physical>>,
    span: tracing::Span,
}

impl<R> WinitGraphicsBackend<R>
where
    R: Bind<EGLSurface>,
    crate::backend::SwapBuffersError: From<R::Error>,
{
    fn new_with_gl_attr(
        window: Arc<dyn winit::window::Window>,
        span: &tracing::Span,
        gl_attributes: GlAttributes,
    ) -> Result<Self, Error>
    where
        R: From<GlesRenderer>,
    {
        let (display, context, surface) = {
            let display = unsafe { EGLDisplay::new(window.clone())? };

            let context =
                EGLContext::new_with_config(&display, gl_attributes, PixelFormatRequirements::_10_bit())
                    .or_else(|_| {
                        EGLContext::new_with_config(
                            &display,
                            gl_attributes,
                            PixelFormatRequirements::_8_bit(),
                        )
                    })?;

            let surface = match window.window_handle().map(|handle| handle.as_raw()) {
                Ok(RawWindowHandle::Wayland(handle)) => {
                    debug!("Winit backend: Wayland");
                    let size = window.surface_size();
                    let surface = unsafe {
                        wegl::WlEglSurface::new_from_raw(
                            handle.surface.as_ptr() as *mut _,
                            size.width as i32,
                            size.height as i32,
                        )
                    }
                    .map_err(|err| Error::Surface(err.into()))?;
                    unsafe {
                        EGLSurface::new(
                            &display,
                            context.pixel_format().unwrap(),
                            context.config_id(),
                            surface,
                        )
                        .map_err(EGLError::CreationFailed)?
                    }
                }
                Ok(RawWindowHandle::Xlib(handle)) => {
                    debug!("Winit backend: X11");
                    unsafe {
                        EGLSurface::new(
                            &display,
                            context.pixel_format().unwrap(),
                            context.config_id(),
                            native::XlibWindow(handle.window),
                        )
                        .map_err(EGLError::CreationFailed)?
                    }
                }
                _ => panic!("only running on Wayland or with Xlib is supported"),
            };

            let _ = context.unbind();
            (display, context, surface)
        };

        let renderer = unsafe { GlesRenderer::new(context)?.into() };
        let damage_tracking = display.supports_damage();

        Ok(WinitGraphicsBackend {
            window: window.clone(),
            span: span.clone(),
            _display: display,
            egl_surface: surface,
            damage_tracking,
            bind_size: None,
            renderer,
        })
    }

    /// Window size of the underlying window
    pub fn window_size(&self) -> Size<i32, Physical> {
        let (w, h): (i32, i32) = self.window.surface_size().into();
        (w, h).into()
    }

    /// Scale factor of the underlying window.
    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    /// Reference to the underlying window
    pub fn window(&self) -> &dyn WinitWindow {
        &*self.window
    }

    /// Access the underlying renderer
    pub fn renderer(&mut self) -> &mut R {
        &mut self.renderer
    }

    /// Bind the underlying window to the underlying renderer.
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    pub fn bind(&mut self) -> Result<(&mut R, R::Framebuffer<'_>), crate::backend::SwapBuffersError> {
        // NOTE: we must resize before making the current context current, otherwise the back
        // buffer will be latched. Some nvidia drivers may not like it, but a lot of wayland
        // software does the order that way due to mesa latching back buffer on each
        // `make_current`.
        let window_size = self.window_size();
        if Some(window_size) != self.bind_size {
            self.egl_surface.resize(window_size.w, window_size.h, 0, 0);
        }
        self.bind_size = Some(window_size);

        let fb = self.renderer.bind(&mut self.egl_surface)?;

        Ok((&mut self.renderer, fb))
    }

    /// Retrieve the underlying `EGLSurface` for advanced operations
    ///
    /// **Note:** Don't carelessly use this to manually bind the renderer to the surface,
    /// `WinitGraphicsBackend::bind` transparently handles window resizes for you.
    pub fn egl_surface(&self) -> &EGLSurface {
        &self.egl_surface
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
                        Rectangle::new(
                            (rect.loc.x, bind_size.h - rect.loc.y - rect.size.h).into(),
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

#[derive(Debug)]
struct WinitEventLoopInner {
    window_attributes: WindowAttributes,
    window: Option<Arc<dyn WinitWindow>>,
    window_create_error: Option<Error>,
    clock: Clock<Monotonic>,
    key_counter: u32,
    is_x11: bool,
    scale_factor: f64,
}

/// Abstracted event loop of a [`WinitWindow`].
///
/// You can register it into `calloop` or call
/// [`dispatch_new_events`](WinitEventLoop::dispatch_new_events) periodically to receive any
/// events.
#[derive(Debug)]
pub struct WinitEventLoop {
    inner: WinitEventLoopInner,
    fake_token: Option<Token>,
    initial_events: Vec<WinitEvent>,
    pending_events: Vec<WinitEvent>,
    event_loop: Generic<EventLoop>,
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
        // On first call, handle events produced during window creation
        for event in std::mem::take(&mut self.initial_events) {
            callback(event);
        }

        // SAFETY: we don't drop event loop ourselves.
        let event_loop = unsafe { self.event_loop.get_mut() };

        event_loop.pump_app_events(
            Some(Duration::ZERO),
            &mut WinitEventLoopApp {
                inner: &mut self.inner,
                callback,
            },
        )
    }
}

struct WinitEventLoopApp<'a, F: FnMut(WinitEvent)> {
    inner: &'a mut WinitEventLoopInner,
    callback: F,
}

impl<F: FnMut(WinitEvent)> WinitEventLoopApp<'_, F> {
    fn timestamp(&self) -> InputTime {
        InputTime::from_micros(self.inner.clock.now().as_micros())
    }
}

impl<F: FnMut(WinitEvent)> ApplicationHandler for WinitEventLoopApp<'_, F> {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.inner.window.is_none() {
            match event_loop.create_window(self.inner.window_attributes.clone()) {
                Ok(window) => self.inner.window = Some(Arc::from(window)),
                Err(err) => self.inner.window_create_error = Some(err.into()),
            }

            (self.callback)(WinitEvent::Input(InputEvent::DeviceAdded {
                device: WinitVirtualDevice,
            }));
        }
    }

    fn window_event(&mut self, _event_loop: &dyn ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        let Some(window) = self.inner.window.as_ref() else {
            return;
        };

        match event {
            WindowEvent::SurfaceResized(size) => {
                trace!("Resizing window to {size:?}");
                let (w, h): (i32, i32) = size.into();

                (self.callback)(WinitEvent::Resized {
                    size: (w, h).into(),
                    scale_factor: self.inner.scale_factor,
                });
            }
            WindowEvent::ScaleFactorChanged {
                scale_factor: new_scale_factor,
                ..
            } => {
                trace!("Scale factor changed to {new_scale_factor}");
                self.inner.scale_factor = new_scale_factor;
                let (w, h): (i32, i32) = window.surface_size().into();
                (self.callback)(WinitEvent::Resized {
                    size: (w, h).into(),
                    scale_factor: self.inner.scale_factor,
                });
            }
            WindowEvent::RedrawRequested => {
                (self.callback)(WinitEvent::Redraw);
            }
            WindowEvent::CloseRequested => {
                (self.callback)(WinitEvent::CloseRequested);
            }
            WindowEvent::Focused(focused) => {
                (self.callback)(WinitEvent::Focus(focused));
            }
            WindowEvent::KeyboardInput {
                event, is_synthetic, ..
            } if !is_synthetic && !event.repeat => {
                match event.state {
                    ElementState::Pressed => self.inner.key_counter += 1,
                    ElementState::Released => {
                        self.inner.key_counter = self.inner.key_counter.saturating_sub(1);
                    }
                };

                let scancode = event.physical_key.to_scancode().unwrap_or(0);
                let event = InputEvent::Keyboard {
                    event: WinitKeyboardInputEvent {
                        time: self.timestamp(),
                        key: scancode,
                        count: self.inner.key_counter,
                        state: event.state,
                    },
                };
                (self.callback)(WinitEvent::Input(event));
            }
            WindowEvent::PointerMoved { position, source, .. } => {
                let size = window.surface_size();
                let x = position.x / size.width as f64;
                let y = position.y / size.height as f64;
                let relative_position = RelativePosition::new(x, y);

                match source {
                    PointerSource::Mouse => {
                        let event = InputEvent::PointerMotionAbsolute {
                            event: WinitMouseMovedEvent {
                                time: self.timestamp(),
                                position: relative_position,
                                global_position: position,
                            },
                        };
                        (self.callback)(WinitEvent::Input(event));
                    }
                    PointerSource::Touch { finger_id, force: _ } => {
                        let event = InputEvent::TouchMotion {
                            event: WinitTouchMovedEvent {
                                time: self.timestamp(),
                                position: relative_position,
                                global_position: position,
                                id: finger_id.into_raw() as u64,
                            },
                        };
                        (self.callback)(WinitEvent::Input(event));
                    }
                    // TODO Handle tablet events
                    PointerSource::TabletTool { .. } => {}
                    PointerSource::Unknown => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let event = InputEvent::PointerAxis {
                    event: WinitMouseWheelEvent {
                        time: self.timestamp(),
                        delta,
                    },
                };
                (self.callback)(WinitEvent::Input(event));
            }
            WindowEvent::PointerButton {
                state,
                button,
                position,
                ..
            } => {
                match button {
                    ButtonSource::Mouse(button) => {
                        let event = InputEvent::PointerButton {
                            event: WinitMouseInputEvent {
                                time: self.timestamp(),
                                button,
                                state,
                                is_x11: self.inner.is_x11,
                            },
                        };
                        (self.callback)(WinitEvent::Input(event));
                    }
                    ButtonSource::Touch { finger_id, force: _ } => match state {
                        ElementState::Pressed => {
                            let size = window.surface_size();
                            let x = position.x / size.width as f64;
                            let y = position.y / size.width as f64;
                            let event = InputEvent::TouchDown {
                                event: WinitTouchStartedEvent {
                                    time: self.timestamp(),
                                    global_position: position,
                                    position: RelativePosition::new(x, y),
                                    id: finger_id.into_raw() as u64,
                                },
                            };

                            (self.callback)(WinitEvent::Input(event));
                        }
                        ElementState::Released => {
                            let size = window.surface_size();
                            let x = position.x / size.width as f64;
                            let y = position.y / size.width as f64;
                            let event = InputEvent::TouchMotion {
                                event: WinitTouchMovedEvent {
                                    time: self.timestamp(),
                                    position: RelativePosition::new(x, y),
                                    global_position: position,
                                    id: finger_id.into_raw() as u64,
                                },
                            };
                            (self.callback)(WinitEvent::Input(event));

                            let event = InputEvent::TouchUp {
                                event: WinitTouchEndedEvent {
                                    time: self.timestamp(),
                                    id: finger_id.into_raw() as u64,
                                },
                            };

                            (self.callback)(WinitEvent::Input(event));
                        }
                    },
                    // TODO Handle tablet events
                    ButtonSource::TabletTool { .. } => {}
                    ButtonSource::Unknown(_) => {}
                }
            }
            WindowEvent::PointerLeft {
                kind: PointerKind::Touch(finger_id),
                ..
            } => {
                let event = InputEvent::TouchCancel {
                    event: WinitTouchCancelledEvent {
                        time: self.timestamp(),
                        id: finger_id.into_raw() as u64,
                    },
                };
                (self.callback)(WinitEvent::Input(event));
            }
            WindowEvent::DragDropped { .. }
            | WindowEvent::Destroyed
            | WindowEvent::PointerEntered { .. }
            | WindowEvent::PointerLeft { .. }
            | WindowEvent::ModifiersChanged(_)
            | WindowEvent::KeyboardInput { .. }
            | WindowEvent::DragEntered { .. }
            | WindowEvent::DragLeft { .. }
            | WindowEvent::DragMoved { .. }
            | WindowEvent::Ime(_)
            | WindowEvent::Moved(_)
            | WindowEvent::Occluded(_)
            | WindowEvent::DoubleTapGesture { .. }
            | WindowEvent::ThemeChanged(_)
            | WindowEvent::PinchGesture { .. }
            | WindowEvent::TouchpadPressure { .. }
            | WindowEvent::RotationGesture { .. }
            | WindowEvent::PanGesture { .. }
            | WindowEvent::ActivationTokenDone { .. } => (),
        }
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
