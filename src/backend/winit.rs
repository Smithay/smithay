//! Implementation of backend traits for types provided by `winit`

use crate::backend::{
    egl::{
        context::GlAttributes, error::Result as EGLResult, native, EGLContext, EGLDisplay,
        EGLGraphicsBackend, EGLSurface,
    },
    graphics::{gl::GLGraphicsBackend, CursorBackend, PixelFormat, SwapBuffersError},
    input::{
        Axis, AxisSource, Event as BackendEvent, InputBackend, InputHandler, KeyState, KeyboardKeyEvent,
        MouseButton, MouseButtonState, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent,
        Seat, SeatCapabilities, TouchCancelEvent, TouchDownEvent, TouchMotionEvent, TouchSlot, TouchUpEvent,
        UnusedEvent,
    },
};
use nix::libc::c_void;
use std::{
    cell::{Ref, RefCell},
    cmp, fmt,
    rc::Rc,
    time::Instant,
};
use wayland_client::egl as wegl;
use wayland_server::Display;
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    ElementState, Event, EventsLoop, KeyboardInput, MouseButton as WinitMouseButton, MouseCursor,
    MouseScrollDelta, Touch, TouchPhase, Window as WinitWindow, WindowBuilder, WindowEvent,
};

/// Errors thrown by the `winit` backends
pub mod errors {
    use crate::backend::egl::error as egl_error;

    error_chain! {
        errors {
            #[doc = "Failed to initialize a window"]
            InitFailed {
                description("Failed to initialize a window")
            }

            #[doc = "Context creation is not supported on the current window system"]
            NotSupported {
                description("Context creation is not supported on the current window system.")
            }
        }

        links {
            EGL(egl_error::Error, egl_error::ErrorKind) #[doc = "EGL error"];
        }
    }
}
use self::errors::*;

enum Window {
    Wayland {
        context: EGLContext<native::Wayland, WinitWindow>,
        surface: EGLSurface<wegl::WlEglSurface>,
    },
    X11 {
        context: EGLContext<native::X11, WinitWindow>,
        surface: EGLSurface<native::XlibWindow>,
    },
}

impl Window {
    fn window(&self) -> Ref<'_, WinitWindow> {
        match *self {
            Window::Wayland { ref context, .. } => context.borrow(),
            Window::X11 { ref context, .. } => context.borrow(),
        }
    }
}

struct WindowSize {
    logical_size: LogicalSize,
    dpi_factor: f64,
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
    events_loop: EventsLoop,
    events_handler: Option<Box<dyn WinitEventsHandler>>,
    window: Rc<Window>,
    time: Instant,
    key_counter: u32,
    seat: Seat,
    input_config: (),
    handler: Option<Box<dyn InputHandler<WinitInputBackend> + 'static>>,
    logger: ::slog::Logger,
    size: Rc<RefCell<WindowSize>>,
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`EGLGraphicsBackend`]
/// and [`GLGraphicsBackend`] graphics backend trait and a corresponding [`WinitInputBackend`],
/// which implements the [`InputBackend`] trait
pub fn init<L>(logger: L) -> Result<(WinitGraphicsBackend, WinitInputBackend)>
where
    L: Into<Option<::slog::Logger>>,
{
    init_from_builder(
        WindowBuilder::new()
            .with_dimensions(LogicalSize::new(1280.0, 800.0))
            .with_title("Smithay")
            .with_visibility(true),
        logger,
    )
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`EGLGraphicsBackend`]
/// and [`GLGraphicsBackend`] graphics backend trait, from a given [`WindowBuilder`]
/// struct and a corresponding [`WinitInputBackend`], which implements the [`InputBackend`] trait
pub fn init_from_builder<L>(
    builder: WindowBuilder,
    logger: L,
) -> Result<(WinitGraphicsBackend, WinitInputBackend)>
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
) -> Result<(WinitGraphicsBackend, WinitInputBackend)>
where
    L: Into<Option<::slog::Logger>>,
{
    let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_winit"));
    info!(log, "Initializing a winit backend");

    let events_loop = EventsLoop::new();
    let winit_window = builder.build(&events_loop).chain_err(|| ErrorKind::InitFailed)?;
    debug!(log, "Window created");

    let reqs = Default::default();
    let window = Rc::new(
        if native::NativeDisplay::<native::Wayland>::is_backend(&winit_window) {
            let context =
                EGLContext::<native::Wayland, WinitWindow>::new(winit_window, attributes, reqs, log.clone())?;
            let surface = context.create_surface(())?;
            Window::Wayland { context, surface }
        } else if native::NativeDisplay::<native::X11>::is_backend(&winit_window) {
            let context =
                EGLContext::<native::X11, WinitWindow>::new(winit_window, attributes, reqs, log.clone())?;
            let surface = context.create_surface(())?;
            Window::X11 { context, surface }
        } else {
            bail!(ErrorKind::NotSupported);
        },
    );

    let size = Rc::new(RefCell::new(WindowSize {
        logical_size: window
            .window()
            .get_inner_size()
            .expect("Winit window was killed during init."),
        dpi_factor: window.window().get_hidpi_factor(),
    }));

    Ok((
        WinitGraphicsBackend {
            window: window.clone(),
            size: size.clone(),
            logger: log.new(o!("smithay_winit_component" => "graphics")),
        },
        WinitInputBackend {
            events_loop,
            events_handler: None,
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
            input_config: (),
            handler: None,
            logger: log.new(o!("smithay_winit_component" => "input")),
            size,
        },
    ))
}

/// Handler trait to receive window-related events to provide a better *nested* experience.
pub trait WinitEventsHandler {
    /// The window was resized, can be used to adjust the associated [`Output`](::wayland::output::Output)s mode.
    ///
    /// Here are provided the new size (in physical pixels) and the new scale factor provided by `winit`.
    fn resized(&mut self, size: (f64, f64), scale: f64);
    /// The window gained or lost focus
    fn focus_changed(&mut self, focused: bool);
    /// The window needs to be redrawn
    fn refresh(&mut self);
}

impl WinitGraphicsBackend {
    /// Get a reference to the internally used [`WinitWindow`]
    pub fn winit_window(&self) -> Ref<'_, WinitWindow> {
        self.window.window()
    }
}

impl<'a> CursorBackend<'a> for WinitGraphicsBackend {
    type CursorFormat = &'a MouseCursor;
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

    fn set_cursor_representation<'b>(
        &'b self,
        cursor: Self::CursorFormat,
        _hotspot: (u32, u32),
    ) -> ::std::result::Result<(), ()>
    where
        'a: 'b,
    {
        // Cannot log this one, as `CursorFormat` is not `Debug` and should not be
        debug!(self.logger, "Changing cursor representation");
        self.window.window().set_cursor(*cursor);
        Ok(())
    }
}

impl GLGraphicsBackend for WinitGraphicsBackend {
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        trace!(self.logger, "Swapping buffers");
        match *self.window {
            Window::Wayland { ref surface, .. } => surface.swap_buffers(),
            Window::X11 { ref surface, .. } => surface.swap_buffers(),
        }
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        trace!(self.logger, "Getting symbol for {:?}", symbol);
        match *self.window {
            Window::Wayland { ref context, .. } => context.get_proc_address(symbol),
            Window::X11 { ref context, .. } => context.get_proc_address(symbol),
        }
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let size = self.size.borrow();
        size.logical_size.to_physical(size.dpi_factor).into()
    }

    fn is_current(&self) -> bool {
        match *self.window {
            Window::Wayland {
                ref context,
                ref surface,
            } => context.is_current() && surface.is_current(),
            Window::X11 {
                ref context,
                ref surface,
            } => context.is_current() && surface.is_current(),
        }
    }

    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        trace!(self.logger, "Setting EGL context to be the current context");
        match *self.window {
            Window::Wayland { ref surface, .. } => surface.make_current(),
            Window::X11 { ref surface, .. } => surface.make_current(),
        }
    }

    fn get_pixel_format(&self) -> PixelFormat {
        match *self.window {
            Window::Wayland { ref context, .. } => context.get_pixel_format(),
            Window::X11 { ref context, .. } => context.get_pixel_format(),
        }
    }
}

impl EGLGraphicsBackend for WinitGraphicsBackend {
    fn bind_wl_display(&self, display: &Display) -> EGLResult<EGLDisplay> {
        match *self.window {
            Window::Wayland { ref context, .. } => context.bind_wl_display(display),
            Window::X11 { ref context, .. } => context.bind_wl_display(display),
        }
    }
}

/// Errors that may happen when driving the event loop of [`WinitInputBackend`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WinitInputError {
    /// The underlying [`WinitWindow`] was closed. No further events can be processed.
    ///
    /// See `dispatch_new_events`.
    WindowClosed,
}

impl ::std::error::Error for WinitInputError {
    fn description(&self) -> &str {
        match *self {
            WinitInputError::WindowClosed => "Glutin Window was closed",
        }
    }
}

impl fmt::Display for WinitInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::error::Error;
        write!(f, "{}", self.description())
    }
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
    x: f64,
    y: f64,
}

impl BackendEvent for WinitMouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerMotionAbsoluteEvent for WinitMouseMovedEvent {
    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.x * wsize.dpi_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.y * wsize.dpi_factor
    }

    fn x_transformed(&self, width: u32) -> u32 {
        let wsize = self.size.borrow();
        cmp::max((self.x * width as f64 / wsize.logical_size.width) as i32, 0) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        let wsize = self.size.borrow();
        cmp::max((self.y * height as f64 / wsize.logical_size.height) as i32, 0) as u32
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
        self.location.0 * wsize.dpi_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.1 * wsize.dpi_factor
    }

    fn x_transformed(&self, width: u32) -> u32 {
        let wsize = self.size.borrow();
        cmp::min(
            self.location.0 as i32 * width as i32 / wsize.logical_size.width as i32,
            0,
        ) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        let wsize = self.size.borrow();
        cmp::min(
            self.location.1 as i32 * height as i32 / wsize.logical_size.height as i32,
            0,
        ) as u32
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
        self.location.0 * wsize.dpi_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.1 * wsize.dpi_factor
    }

    fn x_transformed(&self, width: u32) -> u32 {
        let wsize = self.size.borrow();
        self.location.0 as u32 * width / wsize.logical_size.width as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        let wsize = self.size.borrow();
        self.location.1 as u32 * height / wsize.logical_size.height as u32
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

impl WinitInputBackend {
    /// Set the events handler
    pub fn set_events_handler<H: WinitEventsHandler + 'static>(&mut self, handler: H) {
        self.events_handler = Some(Box::new(handler));
        info!(self.logger, "New events handler set.");
    }

    /// Get a reference to the set events handler, if any
    pub fn get_events_handler(&mut self) -> Option<&mut dyn WinitEventsHandler> {
        self.events_handler
            .as_mut()
            .map(|handler| &mut **handler as &mut dyn WinitEventsHandler)
    }

    /// Clear out the currently set events handler
    pub fn clear_events_handler(&mut self) {
        self.events_handler = None;
        info!(self.logger, "Events handler unset.");
    }
}

impl InputBackend for WinitInputBackend {
    type InputConfig = ();
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

    fn set_handler<H: InputHandler<Self> + 'static>(&mut self, mut handler: H) {
        if self.handler.is_some() {
            self.clear_handler();
        }
        info!(self.logger, "New input handler set.");
        trace!(self.logger, "Calling on_seat_created with {:?}", self.seat);
        handler.on_seat_created(&self.seat);
        self.handler = Some(Box::new(handler));
    }

    fn get_handler(&mut self) -> Option<&mut dyn InputHandler<Self>> {
        self.handler
            .as_mut()
            .map(|handler| handler as &mut dyn InputHandler<Self>)
    }

    fn clear_handler(&mut self) {
        if let Some(mut handler) = self.handler.take() {
            trace!(self.logger, "Calling on_seat_destroyed with {:?}", self.seat);
            handler.on_seat_destroyed(&self.seat);
        }
        info!(self.logger, "Removing input handler");
    }

    fn input_config(&mut self) -> &mut Self::InputConfig {
        &mut self.input_config
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
    fn dispatch_new_events(&mut self) -> ::std::result::Result<(), WinitInputError> {
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
            let mut handler = self.handler.as_mut();
            let mut events_handler = self.events_handler.as_mut();
            let logger = &self.logger;
            let window_size = &self.size;

            self.events_loop.poll_events(move |event| {
                if let Event::WindowEvent { event, .. } = event {
                    let duration = Instant::now().duration_since(*time);
                    let nanos = duration.subsec_nanos() as u64;
                    let time = ((1000 * duration.as_secs()) + (nanos / 1_000_000)) as u32;
                    match (event, handler.as_mut(), events_handler.as_mut()) {
                        (WindowEvent::Resized(size), _, events_handler) => {
                            trace!(logger, "Resizing window to {:?}", size);
                            window.window().set_inner_size(size);
                            let mut wsize = window_size.borrow_mut();
                            wsize.logical_size = size;
                            let physical_size = size.to_physical(wsize.dpi_factor);
                            if let Window::Wayland { ref surface, .. } = **window {
                                surface.resize(physical_size.width as i32, physical_size.height as i32, 0, 0);
                            }
                            if let Some(events_handler) = events_handler {
                                events_handler.resized(physical_size.into(), wsize.dpi_factor);
                            }
                        }
                        (WindowEvent::Focused(focus), _, Some(events_handler)) => {
                            events_handler.focus_changed(focus)
                        }
                        (WindowEvent::Refresh, _, Some(events_handler)) => events_handler.refresh(),
                        (WindowEvent::HiDpiFactorChanged(factor), _, events_handler) => {
                            let mut wsize = window_size.borrow_mut();
                            wsize.dpi_factor = factor;
                            let physical_size = wsize.logical_size.to_physical(factor);
                            if let Window::Wayland { ref surface, .. } = **window {
                                surface.resize(physical_size.width as i32, physical_size.height as i32, 0, 0);
                            }
                            if let Some(events_handler) = events_handler {
                                events_handler.resized(physical_size.into(), wsize.dpi_factor);
                            }
                        }
                        (
                            WindowEvent::KeyboardInput {
                                input: KeyboardInput { scancode, state, .. },
                                ..
                            },
                            Some(handler),
                            _,
                        ) => {
                            match state {
                                ElementState::Pressed => *key_counter += 1,
                                ElementState::Released => {
                                    *key_counter = key_counter.checked_sub(1).unwrap_or(0)
                                }
                            };
                            trace!(logger, "Calling on_keyboard_key with {:?}", (scancode, state));
                            handler.on_keyboard_key(
                                seat,
                                WinitKeyboardInputEvent {
                                    time,
                                    key: scancode,
                                    count: *key_counter,
                                    state,
                                },
                            )
                        }
                        (WindowEvent::CursorMoved { position, .. }, Some(handler), _) => {
                            trace!(logger, "Calling on_pointer_move_absolute with {:?}", position);
                            handler.on_pointer_move_absolute(
                                seat,
                                WinitMouseMovedEvent {
                                    size: window_size.clone(),
                                    time,
                                    x: position.x,
                                    y: position.y,
                                },
                            )
                        }
                        (WindowEvent::MouseWheel { delta, .. }, Some(handler), _) => {
                            let event = WinitMouseWheelEvent { time, delta };
                            trace!(logger, "Calling on_pointer_axis with {:?}", delta);
                            handler.on_pointer_axis(seat, event);
                        }
                        (WindowEvent::MouseInput { state, button, .. }, Some(handler), _) => {
                            trace!(logger, "Calling on_pointer_button with {:?}", (button, state));
                            handler.on_pointer_button(seat, WinitMouseInputEvent { time, button, state })
                        }
                        (
                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Started,
                                location,
                                id,
                                ..
                            }),
                            Some(handler),
                            _,
                        ) => {
                            trace!(logger, "Calling on_touch_down at {:?}", location);
                            handler.on_touch_down(
                                seat,
                                WinitTouchStartedEvent {
                                    size: window_size.clone(),
                                    time,
                                    location: location.into(),
                                    id,
                                },
                            )
                        }
                        (
                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Moved,
                                location,
                                id,
                                ..
                            }),
                            Some(handler),
                            _,
                        ) => {
                            trace!(logger, "Calling on_touch_motion at {:?}", location);
                            handler.on_touch_motion(
                                seat,
                                WinitTouchMovedEvent {
                                    size: window_size.clone(),
                                    time,
                                    location: location.into(),
                                    id,
                                },
                            )
                        }
                        (
                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Ended,
                                location,
                                id,
                                ..
                            }),
                            Some(handler),
                            _,
                        ) => {
                            trace!(logger, "Calling on_touch_motion at {:?}", location);
                            handler.on_touch_motion(
                                seat,
                                WinitTouchMovedEvent {
                                    size: window_size.clone(),
                                    time,
                                    location: location.into(),
                                    id,
                                },
                            );
                            trace!(logger, "Calling on_touch_up");
                            handler.on_touch_up(seat, WinitTouchEndedEvent { time, id });
                        }
                        (
                            WindowEvent::Touch(Touch {
                                phase: TouchPhase::Cancelled,
                                id,
                                ..
                            }),
                            Some(handler),
                            _,
                        ) => {
                            trace!(logger, "Calling on_touch_cancel");
                            handler.on_touch_cancel(seat, WinitTouchCancelledEvent { time, id })
                        }
                        (WindowEvent::CloseRequested, _, _) | (WindowEvent::Destroyed, _, _) => {
                            warn!(logger, "Window closed");
                            *closed_ptr = true;
                        }
                        _ => {}
                    }
                }
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
