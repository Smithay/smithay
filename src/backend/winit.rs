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
//! - a [`WinitGraphicsBackend`], which can give you an implementation of a [`Renderer`]
//!   (or even [`Gles2Renderer`]) through its `renderer` method in addition to further
//!   functionality to access and manage the created winit-window.
//! - a [`WinitInputBackend`], which is an implementation of the [`InputBackend`] trait
//!   using the input events forwarded from the host graphics server.
//!
//! The other types in this module are the instances of the associated types of these
//! two traits for the winit backend.

use crate::{
    backend::{
        egl::{
            context::GlAttributes, display::EGLDisplay, native, EGLContext, EGLSurface, Error as EGLError,
        },
        input::{
            Axis, AxisSource, ButtonState, Device, DeviceCapability, Event as BackendEvent, InputBackend,
            InputEvent, KeyState, KeyboardKeyEvent, MouseButton, PointerAxisEvent, PointerButtonEvent,
            PointerMotionAbsoluteEvent, TouchCancelEvent, TouchDownEvent, TouchMotionEvent, TouchSlot,
            TouchUpEvent, UnusedEvent,
        },
        renderer::{
            gles2::{Gles2Error, Gles2Frame, Gles2Renderer},
            Bind, Renderer, Transform, Unbind,
        },
    },
    utils::{Logical, Physical, Size},
};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Instant};
use wayland_egl as wegl;
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event::{
        ElementState, Event, KeyboardInput, MouseButton as WinitMouseButton, MouseScrollDelta, Touch,
        TouchPhase, WindowEvent,
    },
    event_loop::{ControlFlow, EventLoop},
    platform::run_return::EventLoopExtRunReturn,
    platform::unix::WindowExtUnix,
    window::{Window as WinitWindow, WindowBuilder},
};

use slog::{debug, error, info, o, trace, warn};
use std::cell::Cell;

/// Errors thrown by the `winit` backends
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failed to initialize a window
    #[error("Failed to initialize a window")]
    InitFailed(#[from] winit::error::OsError),
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

/// Window with an active EGL Context created by `winit`. Implements the [`Renderer`] trait
#[derive(Debug)]
pub struct WinitGraphicsBackend {
    renderer: Gles2Renderer,
    display: EGLDisplay,
    egl: Rc<EGLSurface>,
    window: Rc<WinitWindow>,
    size: Rc<RefCell<WindowSize>>,
    resize_notification: Rc<Cell<Option<Size<i32, Physical>>>>,
}

/// Abstracted event loop of a [`WinitWindow`] implementing the [`InputBackend`] trait
///
/// You need to call [`dispatch_new_events`](InputBackend::dispatch_new_events)
/// periodically to receive any events.
#[derive(Debug)]
pub struct WinitInputBackend {
    window: Rc<WinitWindow>,
    events_loop: EventLoop<()>,
    time: Instant,
    key_counter: u32,
    logger: ::slog::Logger,
    initialized: bool,
    size: Rc<RefCell<WindowSize>>,
    resize_notification: Rc<Cell<Option<Size<i32, Physical>>>>,
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`] trait and a corresponding [`WinitInputBackend`],
/// which implements the [`InputBackend`] trait
pub fn init<L>(logger: L) -> Result<(WinitGraphicsBackend, WinitInputBackend), Error>
where
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

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`] trait, from a given [`WindowBuilder`]
/// struct and a corresponding [`WinitInputBackend`], which implements the [`InputBackend`] trait
pub fn init_from_builder<L>(
    builder: WindowBuilder,
    logger: L,
) -> Result<(WinitGraphicsBackend, WinitInputBackend), Error>
where
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

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`] trait, from a given [`WindowBuilder`]
/// struct, as well as given [`GlAttributes`] for further customization of the rendering pipeline and a
/// corresponding [`WinitInputBackend`], which implements the [`InputBackend`] trait.
pub fn init_from_builder_with_gl_attr<L>(
    builder: WindowBuilder,
    attributes: GlAttributes,
    logger: L,
) -> Result<(WinitGraphicsBackend, WinitInputBackend), Error>
where
    L: Into<Option<::slog::Logger>>,
{
    let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_winit"));
    info!(log, "Initializing a winit backend");

    let events_loop = EventLoop::new();
    let winit_window = builder.build(&events_loop).map_err(Error::InitFailed)?;

    debug!(log, "Window created");

    let reqs = Default::default();
    let (display, context, surface) = {
        let display = EGLDisplay::new(&winit_window, log.clone())?;
        let context = EGLContext::new_with_config(&display, attributes, reqs, log.clone())?;

        let surface = if let Some(wl_surface) = winit_window.wayland_surface() {
            debug!(log, "Winit backend: Wayland");
            let size = winit_window.inner_size();
            let surface = unsafe {
                wegl::WlEglSurface::new_from_raw(wl_surface as *mut _, size.width as i32, size.height as i32)
            };
            EGLSurface::new(
                &display,
                context.pixel_format().unwrap(),
                context.config_id(),
                surface,
                log.clone(),
            )
            .map_err(EGLError::CreationFailed)?
        } else if let Some(xlib_window) = winit_window.xlib_window().map(native::XlibWindow) {
            debug!(log, "Winit backend: X11");
            EGLSurface::new(
                &display,
                context.pixel_format().unwrap(),
                context.config_id(),
                xlib_window,
                log.clone(),
            )
            .map_err(EGLError::CreationFailed)?
        } else {
            unreachable!("No backends for winit other then Wayland and X11 are supported")
        };

        let _ = context.unbind();

        (display, context, surface)
    };

    let (w, h): (u32, u32) = winit_window.inner_size().into();
    let size = Rc::new(RefCell::new(WindowSize {
        physical_size: (w as i32, h as i32).into(),
        scale_factor: winit_window.scale_factor(),
    }));

    let window = Rc::new(winit_window);
    let egl = Rc::new(surface);
    let renderer = unsafe { Gles2Renderer::new(context, log.clone())? };
    let resize_notification = Rc::new(Cell::new(None));

    Ok((
        WinitGraphicsBackend {
            window: window.clone(),
            display,
            egl,
            renderer,
            size: size.clone(),
            resize_notification: resize_notification.clone(),
        },
        WinitInputBackend {
            resize_notification,
            events_loop,
            window,
            time: Instant::now(),
            key_counter: 0,
            initialized: false,
            logger: log.new(o!("smithay_winit_component" => "input")),
            size,
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
    /// A redraw was requested
    Refresh,
}

impl WinitGraphicsBackend {
    /// Window size of the underlying window
    pub fn window_size(&self) -> WindowSize {
        self.size.borrow().clone()
    }

    /// Reference to the underlying window
    pub fn window(&self) -> &WinitWindow {
        &*self.window
    }

    /// Access the underlying renderer
    pub fn renderer(&mut self) -> &mut Gles2Renderer {
        &mut self.renderer
    }

    /// Shortcut to `Renderer::render` with the current window dimensions
    /// and this window set as the rendering target.
    pub fn render<F, R>(&mut self, rendering: F) -> Result<R, crate::backend::SwapBuffersError>
    where
        F: FnOnce(&mut Gles2Renderer, &mut Gles2Frame) -> R,
    {
        // Were we told to resize?
        if let Some(size) = self.resize_notification.take() {
            self.egl.resize(size.w, size.h, 0, 0);
        }

        let size = {
            let size = self.size.borrow();
            size.physical_size
        };

        self.renderer.bind(self.egl.clone())?;
        let result = self.renderer.render(size, Transform::Normal, rendering)?;
        self.egl.swap_buffers()?;
        self.renderer.unbind()?;
        Ok(result)
    }
}

/// Virtual input device used by the backend to associate input events
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct WinitVirtualDevice;

impl Device for WinitVirtualDevice {
    fn id(&self) -> String {
        String::from("winit")
    }

    fn name(&self) -> String {
        String::from("winit virtual input")
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Keyboard | DeviceCapability::Pointer | DeviceCapability::Touch
        )
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

/// Errors that may happen when driving the event loop of [`WinitInputBackend`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum WinitInputError {
    /// The underlying [`WinitWindow`] was closed. No further events can be processed.
    ///
    /// See `dispatch_new_events`.
    #[error("Winit window was closed")]
    WindowClosed,
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`KeyboardKeyEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitKeyboardInputEvent {
    time: u32,
    key: u32,
    count: u32,
    state: ElementState,
}

impl BackendEvent<WinitInputBackend> for WinitKeyboardInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl KeyboardKeyEvent<WinitInputBackend> for WinitKeyboardInputEvent {
    fn key_code(&self) -> u32 {
        self.key
    }

    fn state(&self) -> KeyState {
        self.state.into()
    }

    fn count(&self) -> u32 {
        self.count
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerMotionAbsoluteEvent`]
#[derive(Debug, Clone)]
pub struct WinitMouseMovedEvent {
    size: Rc<RefCell<WindowSize>>,
    time: u32,
    logical_position: LogicalPosition<f64>,
}

impl BackendEvent<WinitInputBackend> for WinitMouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<WinitInputBackend> for WinitMouseMovedEvent {
    // TODO: maybe use {Logical, Physical}Position from winit?
    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.logical_position.x * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.logical_position.y * wsize.scale_factor
    }

    fn x_transformed(&self, width: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.logical_size().w;
        f64::max(self.logical_position.x * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.logical_size().h;
        f64::max(self.logical_position.y * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerAxisEvent`]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinitMouseWheelEvent {
    time: u32,
    delta: MouseScrollDelta,
}

impl BackendEvent<WinitInputBackend> for WinitMouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerAxisEvent<WinitInputBackend> for WinitMouseWheelEvent {
    fn source(&self) -> AxisSource {
        match self.delta {
            MouseScrollDelta::LineDelta(_, _) => AxisSource::Wheel,
            MouseScrollDelta::PixelDelta(_) => AxisSource::Continuous,
        }
    }

    fn amount(&self, axis: Axis) -> Option<f64> {
        match (axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::PixelDelta(delta)) => Some(delta.x),
            (Axis::Vertical, MouseScrollDelta::PixelDelta(delta)) => Some(delta.y),
            (_, MouseScrollDelta::LineDelta(_, _)) => None,
        }
    }

    fn amount_discrete(&self, axis: Axis) -> Option<f64> {
        match (axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::LineDelta(x, _)) => Some(x as f64),
            (Axis::Vertical, MouseScrollDelta::LineDelta(_, y)) => Some(y as f64),
            (_, MouseScrollDelta::PixelDelta(_)) => None,
        }
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerButtonEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitMouseInputEvent {
    time: u32,
    button: WinitMouseButton,
    state: ElementState,
}

impl BackendEvent<WinitInputBackend> for WinitMouseInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerButtonEvent<WinitInputBackend> for WinitMouseInputEvent {
    fn button(&self) -> MouseButton {
        self.button.into()
    }

    fn state(&self) -> ButtonState {
        self.state.into()
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchDownEvent`]
#[derive(Debug, Clone)]
pub struct WinitTouchStartedEvent {
    size: Rc<RefCell<WindowSize>>,
    time: u32,
    location: LogicalPosition<f64>,
    id: u64,
}

impl BackendEvent<WinitInputBackend> for WinitTouchStartedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchDownEvent<WinitInputBackend> for WinitTouchStartedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }

    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.x * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.y * wsize.scale_factor
    }

    fn x_transformed(&self, width: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.logical_size().w;
        f64::max(self.location.x * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.logical_size().h;
        f64::max(self.location.y * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchMotionEvent`]
#[derive(Debug, Clone)]
pub struct WinitTouchMovedEvent {
    size: Rc<RefCell<WindowSize>>,
    time: u32,
    location: LogicalPosition<f64>,
    id: u64,
}

impl BackendEvent<WinitInputBackend> for WinitTouchMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchMotionEvent<WinitInputBackend> for WinitTouchMovedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }

    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.x * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.y * wsize.scale_factor
    }

    fn x_transformed(&self, width: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.logical_size().w;
        f64::max(self.location.x * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.logical_size().h;
        f64::max(self.location.y * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a `TouchUpEvent`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitTouchEndedEvent {
    time: u32,
    id: u64,
}

impl BackendEvent<WinitInputBackend> for WinitTouchEndedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchUpEvent<WinitInputBackend> for WinitTouchEndedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchCancelEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitTouchCancelledEvent {
    time: u32,
    id: u64,
}

impl BackendEvent<WinitInputBackend> for WinitTouchCancelledEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchCancelEvent<WinitInputBackend> for WinitTouchCancelledEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

impl InputBackend for WinitInputBackend {
    type EventError = WinitInputError;

    type Device = WinitVirtualDevice;
    type KeyboardKeyEvent = WinitKeyboardInputEvent;
    type PointerAxisEvent = WinitMouseWheelEvent;
    type PointerButtonEvent = WinitMouseInputEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = WinitMouseMovedEvent;
    type TouchDownEvent = WinitTouchStartedEvent;
    type TouchUpEvent = WinitTouchEndedEvent;
    type TouchMotionEvent = WinitTouchMovedEvent;
    type TouchCancelEvent = WinitTouchCancelledEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;

    type SpecialEvent = WinitEvent;

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
    fn dispatch_new_events<F>(&mut self, mut callback: F) -> ::std::result::Result<(), WinitInputError>
    where
        F: FnMut(InputEvent<Self>),
    {
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

            if !self.initialized {
                callback(InputEvent::DeviceAdded {
                    device: WinitVirtualDevice,
                });
                self.initialized = true;
            }

            self.events_loop
                .run_return(move |event, _target, control_flow| match event {
                    Event::RedrawEventsCleared => {
                        *control_flow = ControlFlow::Exit;
                    }
                    Event::RedrawRequested(_id) => {
                        callback(InputEvent::Special(WinitEvent::Refresh));
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

                                callback(InputEvent::Special(WinitEvent::Resized {
                                    size: wsize.physical_size,
                                    scale_factor,
                                }));
                            }
                            WindowEvent::Focused(focus) => {
                                callback(InputEvent::Special(WinitEvent::Focus(focus)));
                            }

                            WindowEvent::ScaleFactorChanged {
                                scale_factor,
                                new_inner_size: new_psize,
                            } => {
                                let mut wsize = window_size.borrow_mut();
                                wsize.scale_factor = scale_factor;

                                let (pw, ph): (u32, u32) = (*new_psize).into();
                                resize_notification.set(Some((pw as i32, ph as i32).into()));

                                callback(InputEvent::Special(WinitEvent::Resized {
                                    size: (pw as i32, ph as i32).into(),
                                    scale_factor: wsize.scale_factor,
                                }));
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
                                callback(InputEvent::Keyboard {
                                    event: WinitKeyboardInputEvent {
                                        time,
                                        key: scancode,
                                        count: *key_counter,
                                        state,
                                    },
                                });
                            }
                            WindowEvent::CursorMoved { position, .. } => {
                                let lpos = position.to_logical(window_size.borrow().scale_factor);
                                callback(InputEvent::PointerMotionAbsolute {
                                    event: WinitMouseMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        logical_position: lpos,
                                    },
                                });
                            }
                            WindowEvent::MouseWheel { delta, .. } => {
                                let event = WinitMouseWheelEvent { time, delta };
                                callback(InputEvent::PointerAxis { event });
                            }
                            WindowEvent::MouseInput { state, button, .. } => {
                                callback(InputEvent::PointerButton {
                                    event: WinitMouseInputEvent { time, button, state },
                                });
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Started,
                                location,
                                id,
                                ..
                            }) => {
                                let location = location.to_logical(window_size.borrow().scale_factor);
                                callback(InputEvent::TouchDown {
                                    event: WinitTouchStartedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location,
                                        id,
                                    },
                                });
                            }
                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Moved,
                                location,
                                id,
                                ..
                            }) => {
                                let location = location.to_logical(window_size.borrow().scale_factor);
                                callback(InputEvent::TouchMotion {
                                    event: WinitTouchMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location,
                                        id,
                                    },
                                });
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Ended,
                                location,
                                id,
                                ..
                            }) => {
                                let location = location.to_logical(window_size.borrow().scale_factor);
                                callback(InputEvent::TouchMotion {
                                    event: WinitTouchMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location,
                                        id,
                                    },
                                });
                                callback(InputEvent::TouchUp {
                                    event: WinitTouchEndedEvent { time, id },
                                })
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Cancelled,
                                id,
                                ..
                            }) => {
                                callback(InputEvent::TouchCancel {
                                    event: WinitTouchCancelledEvent { time, id },
                                });
                            }
                            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                                callback(InputEvent::DeviceRemoved {
                                    device: WinitVirtualDevice,
                                });
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
            Err(WinitInputError::WindowClosed)
        } else {
            Ok(())
        }
    }
}

impl From<WinitMouseButton> for MouseButton {
    fn from(button: WinitMouseButton) -> MouseButton {
        match button {
            WinitMouseButton::Left => MouseButton::Left,
            WinitMouseButton::Right => MouseButton::Right,
            WinitMouseButton::Middle => MouseButton::Middle,
            WinitMouseButton::Other(num) => MouseButton::Other(num as u8),
        }
    }
}

impl From<ElementState> for KeyState {
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        }
    }
}

impl From<ElementState> for ButtonState {
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => ButtonState::Pressed,
            ElementState::Released => ButtonState::Released,
        }
    }
}
