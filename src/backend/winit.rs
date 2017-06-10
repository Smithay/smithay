//! Implementation of backend traits for types provided by `winit`

use backend::{SeatInternal, TouchSlotInternal};
use backend::graphics::GraphicsBackend;
use backend::graphics::egl::{CreationError, EGLContext, EGLGraphicsBackend, GlAttributes, NativeDisplay,
                             NativeSurface, PixelFormat, PixelFormatRequirements, SwapBuffersError};
use backend::input::{Axis, AxisSource, Event as BackendEvent, InputBackend, InputHandler, KeyState,
                     KeyboardKeyEvent, MouseButton, MouseButtonState, PointerAxisEvent, PointerButtonEvent,
                     PointerMotionAbsoluteEvent, Seat, SeatCapabilities, TouchCancelEvent, TouchDownEvent,
                     TouchMotionEvent, TouchSlot, TouchUpEvent, UnusedEvent};
use nix::c_void;

use std::cmp;
use std::error::Error;
use std::fmt;
use std::rc::Rc;
use wayland_client::egl as wegl;
use winit::{CreationError as WinitCreationError, ElementState, Event, EventsLoop, KeyboardInput,
            MouseButton as WinitMouseButton, MouseCursor, MouseScrollDelta, Touch, TouchPhase, Window,
            WindowBuilder, WindowEvent};
use winit::os::unix::{WindowExt, get_x11_xconnection};

rental! {
    mod egl {
        use std::boxed::Box;
        use ::backend::graphics::egl::{EGLContext, EGLSurface};


        #[rental(deref_suffix)]
        pub struct RentEGL {
            context: Box<EGLContext>,
            surface: EGLSurface<'context>,
        }
    }
}

/// Window with an active EGL Context created by `winit`. Implements the
/// `EGLGraphicsBackend` graphics backend trait
pub struct WinitGraphicsBackend {
    window: Rc<Window>,
    context: egl::RentEGL,
    logger: ::slog::Logger,
}

/// Abstracted event loop of a `winit` `Window` implementing the `InputBackend` trait
///
/// You need to call `dispatch_new_events` periodically to receive any events.
pub struct WinitInputBackend {
    events_loop: EventsLoop,
    window: Rc<Window>,
    surface: Option<wegl::WlEglSurface>,
    time_counter: u32,
    key_counter: u32,
    seat: Seat,
    input_config: (),
    handler: Option<Box<InputHandler<WinitInputBackend> + 'static>>,
    logger: ::slog::Logger,
}

/// Create a new `WinitGraphicsBackend`, which implements the `EGLGraphicsBackend`
/// graphics backend trait and a corresponding `WinitInputBackend`, which implements
/// the `InputBackend` trait
pub fn init<L>(logger: L) -> Result<(WinitGraphicsBackend, WinitInputBackend), CreationError>
    where L: Into<Option<::slog::Logger>>
{
    init_from_builder(WindowBuilder::new()
                          .with_dimensions(1280, 800)
                          .with_title("Smithay")
                          .with_visibility(true),
                      logger)
}

/// Create a new `WinitGraphicsBackend`, which implements the `EGLGraphicsBackend`
/// graphics backend trait, from a given `WindowBuilder` struct and a corresponding
/// `WinitInputBackend`, which implements the `InputBackend` trait
pub fn init_from_builder<L>(builder: WindowBuilder, logger: L)
                            -> Result<(WinitGraphicsBackend, WinitInputBackend), CreationError>
    where L: Into<Option<::slog::Logger>>
{
    init_from_builder_with_gl_attr(builder,
                                   GlAttributes {
                                       version: None,
                                       profile: None,
                                       debug: cfg!(debug_assertions),
                                       vsync: true,
                                   },
                                   logger)
}

/// Create a new `WinitGraphicsBackend`, which implements the `EGLGraphicsBackend`
/// graphics backend trait, from a given `WindowBuilder` struct, as well as given
/// `GlAttributes` for further customization of the rendering pipeline and a
/// corresponding `WinitInputBackend`, which implements the `InputBackend` trait.
pub fn init_from_builder_with_gl_attr<L>
    (builder: WindowBuilder, attributes: GlAttributes, logger: L)
     -> Result<(WinitGraphicsBackend, WinitInputBackend), CreationError>
    where L: Into<Option<::slog::Logger>>
{
    let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_winit"));
    info!(log, "Initializing a winit backend");

    let events_loop = EventsLoop::new();
    let window = Rc::new(builder.build(&events_loop)?);
    debug!(log, "Window created");

    let (native_display, native_surface, surface) = if let (Some(conn), Some(window)) =
        (get_x11_xconnection(), window.get_xlib_window()) {
        debug!(log, "Window is backed by X11");
        (NativeDisplay::X11(conn.display as *const _), NativeSurface::X11(window), None)
    } else if let (Some(display), Some(surface)) =
        (window.get_wayland_display(), window.get_wayland_client_surface()) {
        debug!(log, "Window is backed by Wayland");
        let (w, h) = window.get_inner_size().unwrap();
        let egl_surface = wegl::WlEglSurface::new(surface, w as i32, h as i32);
        (NativeDisplay::Wayland(display),
         NativeSurface::Wayland(egl_surface.ptr() as *const _),
         Some(egl_surface))
    } else {
        error!(log, "Window is backed by an unsupported graphics framework");
        return Err(CreationError::NotSupported);
    };

    let context = unsafe {
        match EGLContext::new(native_display,
                              attributes,
                              PixelFormatRequirements {
                                  hardware_accelerated: Some(true),
                                  color_bits: Some(24),
                                  alpha_bits: Some(8),
                                  ..Default::default()
                              },
                              log.clone()) {
            Ok(context) => context,
            Err(err) => {
                error!(log, "EGLContext creation failed:\n {}", err);
                return Err(err);
            }
        }
    };

    Ok((WinitGraphicsBackend {
            window: window.clone(),
            context: match egl::RentEGL::try_new(Box::new(context), move |context| unsafe {
        context.create_surface(native_surface)
    }) {
                Ok(x) => x,
                Err(::rental::TryNewError(err, _)) => return Err(err),
            },
            logger: log.new(o!("smithay_winit_component" => "graphics")),
        },
        WinitInputBackend {
            events_loop: events_loop,
            window: window,
            surface: surface,
            time_counter: 0,
            key_counter: 0,
            seat: Seat::new(0,
                            SeatCapabilities {
                                pointer: true,
                                keyboard: true,
                                touch: true,
                            }),
            input_config: (),
            handler: None,
            logger: log.new(o!("smithay_winit_component" => "input")),
        }))
}

impl GraphicsBackend for WinitGraphicsBackend {
    type CursorFormat = MouseCursor;

    fn set_cursor_position(&mut self, x: u32, y: u32) -> Result<(), ()> {
        debug!(self.logger, "Setting cursor position to {:?}", (x, y));
        self.window.set_cursor_position(x as i32, y as i32)
    }

    fn set_cursor_representation(&mut self, cursor: Self::CursorFormat) {
        // Cannot log this one, as `CursorFormat` is not `Debug` and should not be
        debug!(self.logger, "Changing cursor representation");
        self.window.set_cursor(cursor)
    }
}

impl EGLGraphicsBackend for WinitGraphicsBackend {
    fn swap_buffers(&self) -> Result<(), SwapBuffersError> {
        trace!(self.logger, "Swapping buffers");
        self.context.rent(|surface| surface.swap_buffers())
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        trace!(self.logger, "Getting symbol for {:?}", symbol);
        self.context.get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        self.window
            .get_inner_size_pixels()
            .expect("Window does not exist anymore")
    }

    fn is_current(&self) -> bool {
        self.context.is_current()
    }

    unsafe fn make_current(&self) -> Result<(), SwapBuffersError> {
        debug!(self.logger, "Setting EGL context to be the current context");
        self.context.rent(|surface| surface.make_current())
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.context.get_pixel_format()
    }
}

/// Errors that may happen when driving the event loop of `WinitInputBackend`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WinitInputError {
    /// The underlying `winit` `Window` was closed. No further events can be processed.
    ///
    /// See `WinitInputBackend::dispatch_new_events`.
    WindowClosed,
}

impl Error for WinitInputError {
    fn description(&self) -> &str {
        match *self {
            WinitInputError::WindowClosed => "Glutin Window was closed",
        }
    }
}

impl fmt::Display for WinitInputError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Winit-Backend internal event wrapping winit's types into a `KeyboardKeyEvent`
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
/// Winit-Backend internal event wrapping winit's types into a `PointerMotionAbsoluteEvent`
pub struct WinitMouseMovedEvent {
    window: Rc<Window>,
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
        self.x
    }

    fn y(&self) -> f64 {
        self.y
    }

    fn x_transformed(&self, width: u32) -> u32 {
        cmp::min((self.x * width as f64 /
                  self.window.get_inner_size_points().unwrap_or((width, 0)).0 as f64) as u32,
                 0)
    }

    fn y_transformed(&self, height: u32) -> u32 {
        cmp::min((self.y * height as f64 /
                  self.window.get_inner_size_points().unwrap_or((0, height)).1 as f64) as
                 u32,
                 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Winit-Backend internal event wrapping winit's types into a `PointerAxisEvent`
pub struct WinitMouseWheelEvent {
    axis: Axis,
    time: u32,
    delta: MouseScrollDelta,
}

impl BackendEvent for WinitMouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerAxisEvent for WinitMouseWheelEvent {
    fn axis(&self) -> Axis {
        self.axis
    }

    fn source(&self) -> AxisSource {
        match self.delta {
            MouseScrollDelta::LineDelta(_, _) => AxisSource::Wheel,
            MouseScrollDelta::PixelDelta(_, _) => AxisSource::Continuous,
        }
    }

    fn amount(&self) -> f64 {
        match (self.axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::LineDelta(x, _)) |
            (Axis::Horizontal, MouseScrollDelta::PixelDelta(x, _)) => x as f64,
            (Axis::Vertical, MouseScrollDelta::LineDelta(_, y)) |
            (Axis::Vertical, MouseScrollDelta::PixelDelta(_, y)) => y as f64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Winit-Backend internal event wrapping winit's types into a `PointerButtonEvent`
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
/// Winit-Backend internal event wrapping winit's types into a `TouchDownEvent`
pub struct WinitTouchStartedEvent {
    window: Rc<Window>,
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
        self.location.0
    }

    fn y(&self) -> f64 {
        self.location.1
    }

    fn x_transformed(&self, width: u32) -> u32 {
        cmp::min(self.location.0 as i32 * width as i32 /
                 self.window.get_inner_size_points().unwrap_or((width, 0)).0 as i32,
                 0) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        cmp::min(self.location.1 as i32 * height as i32 /
                 self.window.get_inner_size_points().unwrap_or((0, height)).1 as i32,
                 0) as u32
    }
}

#[derive(Clone)]
/// Winit-Backend internal event wrapping winit's types into a `TouchMotionEvent`
pub struct WinitTouchMovedEvent {
    window: Rc<Window>,
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
        self.location.0
    }

    fn y(&self) -> f64 {
        self.location.1
    }

    fn x_transformed(&self, width: u32) -> u32 {
        self.location.0 as u32 * width / self.window.get_inner_size_points().unwrap_or((width, 0)).0
    }

    fn y_transformed(&self, height: u32) -> u32 {
        self.location.1 as u32 * height / self.window.get_inner_size_points().unwrap_or((0, height)).1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Winit-Backend internal event wrapping winit's types into a `TouchUpEvent`
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
/// Winit-Backend internal event wrapping winit's types into a `TouchCancelEvent`
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

    fn get_handler(&mut self) -> Option<&mut InputHandler<Self>> {
        self.handler
            .as_mut()
            .map(|handler| handler as &mut InputHandler<Self>)
    }

    fn clear_handler(&mut self) {
        if let Some(mut handler) = self.handler.take() {
            trace!(self.logger,
                   "Calling on_seat_destroyed with {:?}",
                   self.seat);
            handler.on_seat_destroyed(&self.seat);
        }
        info!(self.logger, "Removing input handler");
    }

    fn input_config(&mut self) -> &mut Self::InputConfig {
        &mut self.input_config
    }

    /// Processes new events of the underlying event loop to drive the set `InputHandler`.
    ///
    /// You need to periodically call this function to keep the underlying event loop and
    /// `Window` active. Otherwise the window may no respond to user interaction and no
    /// input events will be received by a set `InputHandler`.
    ///
    /// Returns an error if the `Window` the window has been closed. Calling
    /// `dispatch_new_events` again after the `Window` has been closed is considered an
    /// application error and unspecified baviour may occur.
    ///
    /// The linked `WinitGraphicsBackend` will error with a lost Context and should
    /// not be used anymore as well.
    fn dispatch_new_events(&mut self) -> Result<(), WinitInputError> {
        let mut closed = false;

        {
            // NOTE: This ugly pile of references is here, because rustc could
            // figure out how to reference all these objects correctly into the
            // upcoming closure, which is why all are borrowed manually and the
            // assignments are then moved into the closure to avoid rustc's
            // wrong interference.
            let mut closed_ptr = &mut closed;
            let mut key_counter = &mut self.key_counter;
            let mut time_counter = &mut self.time_counter;
            let seat = &self.seat;
            let window = &self.window;
            let surface = &self.surface;
            let mut handler = self.handler.as_mut();
            let logger = &self.logger;

            self.events_loop
                .poll_events(move |event| match event {
                                 Event::WindowEvent { event, .. } => {
                                     match (event, handler.as_mut()) {
                                         (WindowEvent::Resized(x, y), _) => {
                                             trace!(logger, "Resizing window to {:?}", (x, y));
                                             window.set_inner_size(x, y);
                                             if let Some(wl_egl_surface) = surface.as_ref() {
                                                 wl_egl_surface.resize(x as i32, y as i32, 0, 0);
                                             }
                                         }
                                         (WindowEvent::KeyboardInput {
                                              input: KeyboardInput { scancode, state, .. }, ..
                                          },
                                          Some(handler)) => {
                                             match state {
                                                 ElementState::Pressed => *key_counter += 1,
                                                 ElementState::Released => {
                                                     *key_counter = key_counter.checked_sub(1).unwrap_or(0)
                                                 }
                                             };
                                             trace!(logger,
                                                    "Calling on_keyboard_key with {:?}",
                                                    (scancode, state));
                                             handler.on_keyboard_key(seat,
                                                                     WinitKeyboardInputEvent {
                                                                         time: *time_counter,
                                                                         key: scancode,
                                                                         count: *key_counter,
                                                                         state: state,
                                                                     })
                                         }
                                         (WindowEvent::MouseMoved { position: (x, y), .. },
                                          Some(handler)) => {
                                             trace!(logger,
                                                    "Calling on_pointer_move_absolute with {:?}",
                                                    (x, y));
                                             handler.on_pointer_move_absolute(seat,
                                                                              WinitMouseMovedEvent {
                                                                                  window: window.clone(),
                                                                                  time: *time_counter,
                                                                                  x: x,
                                                                                  y: y,
                                                                              })
                                         }
                                         (WindowEvent::MouseWheel { delta, .. }, Some(handler)) => {
                                             match delta {
                                                 MouseScrollDelta::LineDelta(x, y) |
                                                 MouseScrollDelta::PixelDelta(x, y) => {
                                                     if x != 0.0 {
                            let event = WinitMouseWheelEvent {
                                axis: Axis::Horizontal,
                                time: *time_counter,
                                delta: delta,
                            };
                            trace!(logger,
                                   "Calling on_pointer_axis for Axis::Horizontal with {:?}",
                                   x);
                            handler.on_pointer_axis(seat, event);
                        }
                                                     if y != 0.0 {
                            let event = WinitMouseWheelEvent {
                                axis: Axis::Vertical,
                                time: *time_counter,
                                delta: delta,
                            };
                            trace!(logger,
                                   "Calling on_pointer_axis for Axis::Vertical with {:?}",
                                   y);
                            handler.on_pointer_axis(seat, event);
                        }
                                                 }
                                             }
                                         }
                                         (WindowEvent::MouseInput { state, button, .. }, Some(handler)) => {
                                             trace!(logger,
                                                    "Calling on_pointer_button with {:?}",
                                                    (button, state));
                                             handler.on_pointer_button(seat,
                                                                       WinitMouseInputEvent {
                                                                           time: *time_counter,
                                                                           button: button,
                                                                           state: state,
                                                                       })
                                         }
                                         (WindowEvent::Touch(Touch {
                                                                 phase: TouchPhase::Started,
                                                                 location: (x, y),
                                                                 id,
                                                                 ..
                                                             }),
                                          Some(handler)) => {
                                             trace!(logger, "Calling on_touch_down at {:?}", (x, y));
                                             handler.on_touch_down(seat,
                                                                   WinitTouchStartedEvent {
                                                                       window: window.clone(),
                                                                       time: *time_counter,
                                                                       location: (x, y),
                                                                       id: id,
                                                                   })
                                         }
                                         (WindowEvent::Touch(Touch {
                                                                 phase: TouchPhase::Moved,
                                                                 location: (x, y),
                                                                 id,
                                                                 ..
                                                             }),
                                          Some(handler)) => {
                                             trace!(logger, "Calling on_touch_motion at {:?}", (x, y));
                                             handler.on_touch_motion(seat,
                                                                     WinitTouchMovedEvent {
                                                                         window: window.clone(),
                                                                         time: *time_counter,
                                                                         location: (x, y),
                                                                         id: id,
                                                                     })
                                         }
                                         (WindowEvent::Touch(Touch {
                                                                 phase: TouchPhase::Ended,
                                                                 location: (x, y),
                                                                 id,
                                                                 ..
                                                             }),
                                          Some(handler)) => {
                                             trace!(logger, "Calling on_touch_motion at {:?}", (x, y));
                                             handler.on_touch_motion(seat,
                                                                     WinitTouchMovedEvent {
                                                                         window: window.clone(),
                                                                         time: *time_counter,
                                                                         location: (x, y),
                                                                         id: id,
                                                                     });
                                             trace!(logger, "Calling on_touch_up");
                                             handler.on_touch_up(seat,
                                                                 WinitTouchEndedEvent {
                                                                     time: *time_counter,
                                                                     id: id,
                                                                 });
                                         }
                                         (WindowEvent::Touch(Touch {
                                                                 phase: TouchPhase::Cancelled,
                                                                 id,
                                                                 ..
                                                             }),
                                          Some(handler)) => {
                                             trace!(logger, "Calling on_touch_cancel");
                                             handler.on_touch_cancel(seat,
                                                                     WinitTouchCancelledEvent {
                                                                         time: *time_counter,
                                                                         id: id,
                                                                     })
                                         }
                                         (WindowEvent::Closed, _) => {
                                             warn!(logger, "Window closed");
                                             *closed_ptr = true;
                                         }
                                         _ => {}
                                     }
                                     *time_counter += 1;
                                 }
                                 Event::DeviceEvent { .. } => {}
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

impl From<WinitCreationError> for CreationError {
    fn from(error: WinitCreationError) -> Self {
        match error {
            WinitCreationError::OsError(x) => CreationError::OsError(x),
            WinitCreationError::NotSupported => CreationError::NotSupported,
        }
    }
}
