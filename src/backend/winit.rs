//! Implementation of backend traits for types provided by `winit`

use crate::backend::egl::display::EGLDisplay;
use crate::backend::egl::get_proc_address;
use crate::backend::{
    egl::{context::GlAttributes, native, EGLContext, EGLSurface, Error as EGLError, SurfaceCreationError},
    graphics::{gl::GLGraphicsBackend, CursorBackend, PixelFormat, SwapBuffersError},
    input::{
        Axis, AxisSource, Event as BackendEvent, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
        MouseButton, MouseButtonState, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent,
        Seat, SeatCapabilities, TouchCancelEvent, TouchDownEvent, TouchMotionEvent, TouchSlot, TouchUpEvent,
        UnusedEvent,
    },
};
use nix::libc::c_void;
use std::{
    cell::{Ref, RefCell},
    cmp,
    convert::TryInto,
    rc::Rc,
    time::Instant,
};
use wayland_egl as wegl;
use wayland_server::Display;
use winit::{
    dpi::{LogicalPosition, LogicalSize, PhysicalSize},
    event::{
        ElementState, Event, KeyboardInput, MouseButton as WinitMouseButton, MouseScrollDelta, Touch,
        TouchPhase, WindowEvent,
    },
    event_loop::{ControlFlow, EventLoop},
    platform::desktop::EventLoopExtDesktop,
    window::{CursorIcon, Window as WinitWindow, WindowBuilder},
};

#[cfg(feature = "use_system_lib")]
use crate::backend::egl::{display::EGLBufferReader, EGLGraphicsBackend};

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
    EGL(#[from] EGLError),
    /// Surface Creation failed
    #[error("Surface creation failed: {0}")]
    SurfaceCreationError(#[from] SurfaceCreationError<EGLError>),
}

enum Window {
    Wayland {
        display: EGLDisplay<native::Wayland, WinitWindow>,
        context: EGLContext,
        surface: EGLSurface<wegl::WlEglSurface>,
    },
    X11 {
        display: EGLDisplay<native::X11, WinitWindow>,
        context: EGLContext,
        surface: EGLSurface<native::XlibWindow>,
    },
}

impl Window {
    fn window(&self) -> Ref<'_, WinitWindow> {
        match *self {
            Window::Wayland { ref display, .. } => display.borrow(),
            Window::X11 { ref display, .. } => display.borrow(),
        }
    }
}

struct WindowSize {
    physical_size: PhysicalSize<u32>,
    scale_factor: f64,
}

/// Window with an active EGL Context created by `winit`. Implements the
/// [`EGLGraphicsBackend`] and [`GLGraphicsBackend`] graphics backend trait
pub struct WinitGraphicsBackend {
    window: Rc<Window>,
    size: Rc<RefCell<WindowSize>>,
    logger: ::slog::Logger,
}

/// Abstracted event loop of a [`WinitWindow`] implementing the [`InputBackend`] trait
///
/// You need to call [`dispatch_new_events`](InputBackend::dispatch_new_events)
/// periodically to receive any events.
pub struct WinitInputBackend {
    events_loop: EventLoop<()>,
    window: Rc<Window>,
    time: Instant,
    key_counter: u32,
    seat: Seat,
    logger: ::slog::Logger,
    size: Rc<RefCell<WindowSize>>,
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`EGLGraphicsBackend`]
/// and [`GLGraphicsBackend`] graphics backend trait and a corresponding [`WinitInputBackend`],
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

/// Create a new [`WinitGraphicsBackend`], which implements the [`EGLGraphicsBackend`]
/// and [`GLGraphicsBackend`] graphics backend trait, from a given [`WindowBuilder`]
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
            version: None,
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: true,
        },
        logger,
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`EGLGraphicsBackend`]
/// and [`GLGraphicsBackend`] graphics backend trait, from a given [`WindowBuilder`]
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
    let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_winit"));
    info!(log, "Initializing a winit backend");

    let events_loop = EventLoop::new();
    let winit_window = builder.build(&events_loop).map_err(Error::InitFailed)?;

    debug!(log, "Window created");

    let reqs = Default::default();
    let window = Rc::new(
        if native::NativeDisplay::<native::Wayland>::is_backend(&winit_window) {
            let display = EGLDisplay::<native::Wayland, WinitWindow>::new(winit_window, log.clone())?;
            let context = display.create_context(attributes, reqs)?;
            let surface = display.create_surface(
                context.get_pixel_format(),
                reqs.double_buffer,
                context.get_config_id(),
                (),
            )?;
            Window::Wayland {
                display,
                context,
                surface,
            }
        } else if native::NativeDisplay::<native::X11>::is_backend(&winit_window) {
            let display = EGLDisplay::<native::X11, WinitWindow>::new(winit_window, log.clone())?;
            let context = display.create_context(attributes, reqs)?;
            let surface = display.create_surface(
                context.get_pixel_format(),
                reqs.double_buffer,
                context.get_config_id(),
                (),
            )?;
            Window::X11 {
                display,
                context,
                surface,
            }
        } else {
            return Err(Error::NotSupported);
        },
    );

    let size = Rc::new(RefCell::new(WindowSize {
        physical_size: window.window().inner_size(), // TODO: original code check if window is alive or not using inner_size().expect()
        scale_factor: window.window().scale_factor(),
    }));

    Ok((
        WinitGraphicsBackend {
            window: window.clone(),
            size: size.clone(),
            logger: log.new(o!("smithay_winit_component" => "graphics")),
        },
        WinitInputBackend {
            events_loop,
            window,
            time: Instant::now(),
            key_counter: 0,
            seat: Seat::new(
                0,
                "winit",
                SeatCapabilities {
                    pointer: true,
                    keyboard: true,
                    touch: true,
                },
            ),
            logger: log.new(o!("smithay_winit_component" => "input")),
            size,
        },
    ))
}

/// Specific events generated by Winit
pub enum WinitEvent {
    /// The window has been resized
    Resized {
        /// The new physical size (in pixels)
        size: (f64, f64),
        /// The new scale factor
        scale_factor: f64,
    },
    /// The focus state of the window changed
    Focus(bool),
    /// A redraw was requested
    Refresh,
}

impl WinitGraphicsBackend {
    /// Get a reference to the internally used [`WinitWindow`]
    pub fn winit_window(&self) -> Ref<'_, WinitWindow> {
        self.window.window()
    }
}

impl CursorBackend for WinitGraphicsBackend {
    type CursorFormat = CursorIcon;
    type Error = ();

    fn set_cursor_position(&self, x: u32, y: u32) -> ::std::result::Result<(), ()> {
        debug!(self.logger, "Setting cursor position to {:?}", (x, y));
        self.window
            .window()
            .set_cursor_position(LogicalPosition::new(x as f64, y as f64))
            .map_err(|err| {
                debug!(self.logger, "{}", err);
            })
    }

    fn set_cursor_representation(
        &self,
        cursor: &Self::CursorFormat,
        _hotspot: (u32, u32),
    ) -> ::std::result::Result<(), ()> {
        // Cannot log this one, as `CursorFormat` is not `Debug` and should not be
        debug!(self.logger, "Changing cursor representation");
        self.window.window().set_cursor_icon(*cursor);
        Ok(())
    }
}

impl GLGraphicsBackend for WinitGraphicsBackend {
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        trace!(self.logger, "Swapping buffers");
        match *self.window {
            Window::Wayland { ref surface, .. } => {
                surface.swap_buffers().map_err(|err| err.try_into().unwrap())
            }
            Window::X11 { ref surface, .. } => surface.swap_buffers().map_err(|err| err.try_into().unwrap()),
        }
    }

    fn get_proc_address(&self, symbol: &str) -> *const c_void {
        trace!(self.logger, "Getting symbol for {:?}", symbol);
        get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let size = self.size.borrow();
        size.physical_size.into()
    }

    fn is_current(&self) -> bool {
        match *self.window {
            Window::Wayland {
                ref context,
                ref surface,
                ..
            } => context.is_current() && surface.is_current(),
            Window::X11 {
                ref context,
                ref surface,
                ..
            } => context.is_current() && surface.is_current(),
        }
    }

    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        trace!(self.logger, "Setting EGL context to be the current context");
        match *self.window {
            Window::Wayland {
                ref surface,
                ref context,
                ..
            } => context.make_current_with_surface(surface).map_err(Into::into),
            Window::X11 {
                ref surface,
                ref context,
                ..
            } => context.make_current_with_surface(surface).map_err(Into::into),
        }
    }

    fn get_pixel_format(&self) -> PixelFormat {
        match *self.window {
            Window::Wayland { ref surface, .. } => surface.get_pixel_format(),
            Window::X11 { ref surface, .. } => surface.get_pixel_format(),
        }
    }
}

#[cfg(feature = "use_system_lib")]
impl EGLGraphicsBackend for WinitGraphicsBackend {
    fn bind_wl_display(&self, wl_display: &Display) -> Result<EGLBufferReader, EGLError> {
        match *self.window {
            Window::Wayland { ref display, .. } => display.bind_wl_display(wl_display),
            Window::X11 { ref display, .. } => display.bind_wl_display(wl_display),
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Winit-Backend internal event wrapping `winit`'s types into a [`KeyboardKeyEvent`]
pub struct WinitKeyboardInputEvent {
    time: u32,
    key: u32,
    count: u32,
    state: ElementState,
}

impl BackendEvent for WinitKeyboardInputEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl KeyboardKeyEvent for WinitKeyboardInputEvent {
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

#[derive(Clone)]
/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerMotionAbsoluteEvent`]
pub struct WinitMouseMovedEvent {
    size: Rc<RefCell<WindowSize>>,
    time: u32,
    logical_position: LogicalPosition<f64>,
}

impl BackendEvent for WinitMouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerMotionAbsoluteEvent for WinitMouseMovedEvent {
    // TODO: maybe use {Logical, Physical}Position from winit?
    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.logical_position.x * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.logical_position.y * wsize.scale_factor
    }

    fn x_transformed(&self, width: u32) -> u32 {
        let wsize = self.size.borrow();
        let w_width = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).width;
        cmp::max((self.logical_position.x * width as f64 / w_width) as i32, 0) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        let wsize = self.size.borrow();
        let w_height = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).height;
        cmp::max((self.logical_position.y * height as f64 / w_height) as i32, 0) as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerAxisEvent`]
pub struct WinitMouseWheelEvent {
    time: u32,
    delta: MouseScrollDelta,
}

impl BackendEvent for WinitMouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerAxisEvent for WinitMouseWheelEvent {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerButtonEvent`]
pub struct WinitMouseInputEvent {
    time: u32,
    button: WinitMouseButton,
    state: ElementState,
}

impl BackendEvent for WinitMouseInputEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerButtonEvent for WinitMouseInputEvent {
    fn button(&self) -> MouseButton {
        self.button.into()
    }

    fn state(&self) -> MouseButtonState {
        self.state.into()
    }
}

#[derive(Clone)]
/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchDownEvent`]
pub struct WinitTouchStartedEvent {
    size: Rc<RefCell<WindowSize>>,
    time: u32,
    location: (f64, f64),
    id: u64,
}

impl BackendEvent for WinitTouchStartedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchDownEvent for WinitTouchStartedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }

    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.0 * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.1 * wsize.scale_factor
    }

    fn x_transformed(&self, width: u32) -> u32 {
        let wsize = self.size.borrow();
        let w_width = wsize.physical_size.to_logical::<i32>(wsize.scale_factor).width;
        cmp::min(self.location.0 as i32 * width as i32 / w_width, 0) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        let wsize = self.size.borrow();
        let w_height = wsize.physical_size.to_logical::<i32>(wsize.scale_factor).height;
        cmp::min(self.location.1 as i32 * height as i32 / w_height, 0) as u32
    }
}

#[derive(Clone)]
/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchMotionEvent`]
pub struct WinitTouchMovedEvent {
    size: Rc<RefCell<WindowSize>>,
    time: u32,
    location: (f64, f64),
    id: u64,
}

impl BackendEvent for WinitTouchMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchMotionEvent for WinitTouchMovedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }

    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.0 * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.1 * wsize.scale_factor
    }

    fn x_transformed(&self, width: u32) -> u32 {
        let wsize = self.size.borrow();
        let w_width = wsize.physical_size.to_logical::<u32>(wsize.scale_factor).width;
        self.location.0 as u32 * width / w_width
    }

    fn y_transformed(&self, height: u32) -> u32 {
        let wsize = self.size.borrow();
        let w_height = wsize.physical_size.to_logical::<u32>(wsize.scale_factor).height;
        self.location.1 as u32 * height / w_height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Winit-Backend internal event wrapping `winit`'s types into a `TouchUpEvent`
pub struct WinitTouchEndedEvent {
    time: u32,
    id: u64,
}

impl BackendEvent for WinitTouchEndedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchUpEvent for WinitTouchEndedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchCancelEvent`]
pub struct WinitTouchCancelledEvent {
    time: u32,
    id: u64,
}

impl BackendEvent for WinitTouchCancelledEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchCancelEvent for WinitTouchCancelledEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

/// Input config for Winit
///
/// This backend does not allow any input configuration, so this type does nothing.
pub struct WinitInputConfig;

impl InputBackend for WinitInputBackend {
    type EventError = WinitInputError;

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

    type SpecialEvent = WinitEvent;
    type InputConfig = WinitInputConfig;

    fn seats(&self) -> Vec<Seat> {
        vec![self.seat.clone()]
    }

    fn input_config(&mut self) -> &mut Self::InputConfig {
        /// So much effort to return a useless singleton!
        static mut CONFIG: WinitInputConfig = WinitInputConfig;
        unsafe { &mut CONFIG }
    }

    /// Processes new events of the underlying event loop to drive the set [`InputHandler`].
    ///
    /// You need to periodically call this function to keep the underlying event loop and
    /// [`WinitWindow`] active. Otherwise the window may no respond to user interaction and no
    /// input events will be received by a set [`InputHandler`].
    ///
    /// Returns an error if the [`WinitWindow`] the window has been closed. Calling
    /// `dispatch_new_events` again after the [`WinitWindow`] has been closed is considered an
    /// application error and unspecified behaviour may occur.
    ///
    /// The linked [`WinitGraphicsBackend`] will error with a lost context and should
    /// not be used anymore as well.
    fn dispatch_new_events<F>(&mut self, mut callback: F) -> ::std::result::Result<(), WinitInputError>
    where
        F: FnMut(InputEvent<Self>, &mut WinitInputConfig),
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
            let seat = &self.seat;
            let window = &self.window;
            let logger = &self.logger;
            let window_size = &self.size;
            let mut callback = move |event| callback(event, &mut WinitInputConfig);

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
                                let scale_factor = window.window().scale_factor();
                                let mut wsize = window_size.borrow_mut();
                                wsize.physical_size = psize;
                                wsize.scale_factor = scale_factor;
                                if let Window::Wayland { ref surface, .. } = **window {
                                    surface.resize(psize.width as i32, psize.height as i32, 0, 0);
                                }
                                callback(InputEvent::Special(WinitEvent::Resized {
                                    size: psize.into(),
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
                                if let Window::Wayland { ref surface, .. } = **window {
                                    surface.resize(new_psize.width as i32, new_psize.height as i32, 0, 0);
                                }
                                let psize_f64: (f64, f64) = (new_psize.width.into(), new_psize.height.into());
                                callback(InputEvent::Special(WinitEvent::Resized {
                                    size: psize_f64,
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
                                    seat: seat.clone(),
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
                                    seat: seat.clone(),
                                    event: WinitMouseMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        logical_position: lpos,
                                    },
                                });
                            }
                            WindowEvent::MouseWheel { delta, .. } => {
                                let event = WinitMouseWheelEvent { time, delta };
                                callback(InputEvent::PointerAxis {
                                    seat: seat.clone(),
                                    event,
                                });
                            }
                            WindowEvent::MouseInput { state, button, .. } => {
                                callback(InputEvent::PointerButton {
                                    seat: seat.clone(),
                                    event: WinitMouseInputEvent { time, button, state },
                                });
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Started,
                                location,
                                id,
                                ..
                            }) => {
                                callback(InputEvent::TouchDown {
                                    seat: seat.clone(),
                                    event: WinitTouchStartedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location: location.into(),
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
                                callback(InputEvent::TouchMotion {
                                    seat: seat.clone(),
                                    event: WinitTouchMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location: location.into(),
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
                                callback(InputEvent::TouchMotion {
                                    seat: seat.clone(),
                                    event: WinitTouchMovedEvent {
                                        size: window_size.clone(),
                                        time,
                                        location: location.into(),
                                        id,
                                    },
                                });
                                callback(InputEvent::TouchUp {
                                    seat: seat.clone(),
                                    event: WinitTouchEndedEvent { time, id },
                                })
                            }

                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Cancelled,
                                id,
                                ..
                            }) => {
                                callback(InputEvent::TouchCancel {
                                    seat: seat.clone(),
                                    event: WinitTouchCancelledEvent { time, id },
                                });
                            }
                            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
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
            WinitMouseButton::Other(num) => MouseButton::Other(num),
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

impl From<ElementState> for MouseButtonState {
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => MouseButtonState::Pressed,
            ElementState::Released => MouseButtonState::Released,
        }
    }
}
