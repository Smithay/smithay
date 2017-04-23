//! Implementation of backend traits for types provided by `glutin`


use backend::{SeatInternal, TouchSlotInternal};
use backend::graphics::GraphicsBackend;
use backend::graphics::opengl::{Api, OpenglGraphicsBackend, PixelFormat, SwapBuffersError};
use backend::input::{Axis, AxisSource, InputBackend, InputHandler, KeyState, MouseButton, MouseButtonState,
                     Seat, SeatCapabilities, TouchSlot, Event as BackendEvent, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent, TouchDownEvent, TouchUpEvent, TouchMotionEvent, TouchCancelEvent};
use glutin::{Api as GlutinApi, MouseButton as GlutinMouseButton, PixelFormat as GlutinPixelFormat, MouseCursor};
use glutin::{ContextError, CreationError, ElementState, Event, GlContext, HeadlessContext,
             HeadlessRendererBuilder, MouseScrollDelta, Touch, TouchPhase, Window, WindowBuilder};
use nix::c_void;
use std::cmp;
use std::error::Error;
use std::fmt;
use std::rc::Rc;

/// Create a new `GlutinHeadlessRenderer` which implements the `OpenglRenderer` graphics
/// backend trait
pub fn init_headless_renderer() -> Result<GlutinHeadlessRenderer, CreationError> {
    init_headless_renderer_from_builder(HeadlessRendererBuilder::new(1024, 600))
}

/// Create a new `GlutinHeadlessRenderer`, which implements the `OpenglRenderer` graphics
/// backend trait, with a given already configured `HeadlessRendererBuilder` for
/// customization
pub fn init_headless_renderer_from_builder(builder: HeadlessRendererBuilder)
                                           -> Result<GlutinHeadlessRenderer, CreationError> {
    let (w, h) = builder.dimensions;
    let context = builder.build_strict()?;

    Ok(GlutinHeadlessRenderer::new(context, w, h))
}

/// Create a new `GlutinWindowedRenderer`, which implements the `OpenglRenderer` graphics
/// backend trait
pub fn init_windowed_renderer() -> Result<GlutinWindowedRenderer, CreationError> {
    init_windowed_renderer_from_builder(WindowBuilder::new())
}

/// Create a new `GlutinWindowedRenderer`, which implements the `OpenglRenderer` graphics
/// backend trait, with a given already configured `WindowBuilder` for customization.
pub fn init_windowed_renderer_from_builder(builder: WindowBuilder)
                                           -> Result<GlutinWindowedRenderer, CreationError> {
    let window = Rc::new(builder.build_strict()?);
    Ok(GlutinWindowedRenderer::new(window))
}

/// Create a new `glutin` `Window`. Returns a `GlutinWindowedRenderer` implementing
/// the `OpenglRenderer` graphics backend trait and a `GlutinInputBackend` implementing
/// the `InputBackend` trait.
pub fn init_windowed() -> Result<(GlutinWindowedRenderer, GlutinInputBackend), CreationError> {
    init_windowed_from_builder(WindowBuilder::new())
}

/// Create a new `glutin` `Window` with a given already configured `WindowBuilder` for
/// customization. Returns a `GlutinWindowedRenderer` implementing
/// the `OpenglRenderer` graphics backend trait and a `GlutinInputBackend` implementing
/// the `InputBackend` trait.
pub fn init_windowed_from_builder(builder: WindowBuilder)
                                  -> Result<(GlutinWindowedRenderer, GlutinInputBackend), CreationError> {
    let window = Rc::new(builder.build_strict()?);
    Ok((GlutinWindowedRenderer::new(window.clone()), GlutinInputBackend::new(window)))
}

/// Headless Opengl Context created by `glutin`. Implements the `OpenglGraphicsBackend` graphics
/// backend trait.
pub struct GlutinHeadlessRenderer {
    context: HeadlessContext,
    w: u32,
    h: u32,
}

impl GlutinHeadlessRenderer {
    fn new(context: HeadlessContext, w: u32, h: u32) -> GlutinHeadlessRenderer {
        GlutinHeadlessRenderer {
            context: context,
            w: w,
            h: h,
        }
    }
}

impl GraphicsBackend for GlutinHeadlessRenderer {
    type CursorFormat = ();

    fn set_cursor_position(&mut self, _x: u32, _y: u32) -> Result<(), ()> {
        //FIXME: Maybe save position? Is it of any use?
        Ok(())
    }

    fn set_cursor_representation(&mut self, _cursor: ()) {}
}

impl OpenglGraphicsBackend for GlutinHeadlessRenderer {
    #[inline]
    fn swap_buffers(&self) -> Result<(), SwapBuffersError> {
        match self.context.swap_buffers() {
            Ok(()) => Ok(()),
            Err(ContextError::IoError(e)) => panic!("Error while swapping buffers: {:?}", e),
            Err(ContextError::ContextLost) => Err(SwapBuffersError::ContextLost),
        }
    }

    #[inline]
    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.context.get_proc_address(symbol) as *const _
    }

    #[inline]
    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    #[inline]
    fn is_current(&self) -> bool {
        self.context.is_current()
    }

    #[inline]
    unsafe fn make_current(&self) {
        self.context.make_current().unwrap();
    }

    fn get_api(&self) -> Api {
        self.context.get_api().into()
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.context.get_pixel_format().into()
    }
}

/// Window with an active Opengl Context created by `glutin`. Implements the
/// `OpenglGraphicsBackend` graphics backend trait.
pub struct GlutinWindowedRenderer {
    window: Rc<Window>,
}

impl GlutinWindowedRenderer {
    fn new(window: Rc<Window>) -> GlutinWindowedRenderer {
        GlutinWindowedRenderer { window: window }
    }
}

impl GraphicsBackend for GlutinWindowedRenderer {
    type CursorFormat = MouseCursor;

    fn set_cursor_position(&mut self, x: u32, y: u32) -> Result<(), ()> {
        if let Some((win_x, win_y)) = self.window.get_position() {
            self.window
                .set_cursor_position(win_x + x as i32, win_y + y as i32)
        } else {
            Err(())
        }
    }

    fn set_cursor_representation(&mut self, cursor: MouseCursor) {
        self.window.set_cursor(cursor);
    }
}

impl OpenglGraphicsBackend for GlutinWindowedRenderer {
    #[inline]
    fn swap_buffers(&self) -> Result<(), SwapBuffersError> {
        match self.window.swap_buffers() {
            Ok(()) => Ok(()),
            Err(ContextError::IoError(e)) => panic!("Error while swapping buffers: {:?}", e),
            Err(ContextError::ContextLost) => Err(SwapBuffersError::ContextLost),
        }
    }

    #[inline]
    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.window.get_proc_address(symbol) as *const _
    }

    #[inline]
    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let (width, height) = self.window.get_inner_size().unwrap_or((800, 600)); // TODO: 800x600 ?
        let scale = self.window.hidpi_factor();
        ((width as f32 * scale) as u32, (height as f32 * scale) as u32)
    }

    #[inline]
    fn is_current(&self) -> bool {
        self.window.is_current()
    }

    #[inline]
    unsafe fn make_current(&self) {
        self.window.make_current().unwrap();
    }

    fn get_api(&self) -> Api {
        self.window.get_api().into()
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.window.get_pixel_format().into()
    }
}

/// Errors that may happen when driving the event loop of `GlutinInputBackend`
#[derive(Debug)]
pub enum GlutinInputError {
    /// The underlying `glutin` `Window` was closed. No further events can be processed.
    ///
    /// See `GlutinInputBackend::process_new_events`.
    WindowClosed,
}

impl Error for GlutinInputError {
    fn description(&self) -> &str {
        match *self {
            GlutinInputError::WindowClosed => "Glutin Window was closed",
        }
    }
}

impl fmt::Display for GlutinInputError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// Abstracted event loop of a `glutin` `Window` implementing the `InputBackend` trait
///
/// You need to call `process_new_events` periodically to receive any events.
pub struct GlutinInputBackend {
    window: Rc<Window>,
    time_counter: u32,
    key_counter: u32,
    seat: Seat,
    input_config: (),
    handler: Option<Box<InputHandler<GlutinInputBackend> + 'static>>,
}

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `KeyboardKeyEvent`
pub struct GlutinKeyboardInputEvent {
    time: u32,
    key: u8,
    count: u32,
    state: ElementState,
}

impl BackendEvent for GlutinKeyboardInputEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl KeyboardKeyEvent for GlutinKeyboardInputEvent {
    fn key_code(&self) -> u32 {
        self.key as u32
    }

    fn state(&self) -> KeyState {
        self.state.into()
    }

    fn count(&self) -> u32 {
        self.count
    }
}

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `PointerMotionAbsoluteEvent`
pub struct GlutinMouseMovedEvent {
    window: Rc<Window>,
    time: u32,
    x: i32,
    y: i32,
}

impl BackendEvent for GlutinMouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerMotionAbsoluteEvent for GlutinMouseMovedEvent {
    fn x(&self) -> f64 {
        self.x as f64
    }

    fn y(&self) -> f64 {
        self.y as f64
    }

    fn x_transformed(&self, width: u32) -> u32 {
        cmp::min(self.x * width as i32 / self.window.get_inner_size_points().unwrap_or((width, 0)).0 as i32, 0) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        cmp::min(self.y * height as i32 / self.window.get_inner_size_points().unwrap_or((0, height)).1 as i32, 0) as u32
    }
}

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `PointerAxisEvent`
pub struct GlutinMouseWheelEvent {
    axis: Axis,
    time: u32,
    delta: MouseScrollDelta,
}

impl BackendEvent for GlutinMouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerAxisEvent for GlutinMouseWheelEvent {
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
            (Axis::Horizontal, MouseScrollDelta::LineDelta(x, _)) | (Axis::Horizontal, MouseScrollDelta::PixelDelta(x, _)) => x as f64,
            (Axis::Vertical, MouseScrollDelta::LineDelta(_, y)) | (Axis::Vertical, MouseScrollDelta::PixelDelta(_, y)) => y as f64,
        }
    }
}

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `PointerButtonEvent`
pub struct GlutinMouseInputEvent {
    time: u32,
    button: GlutinMouseButton,
    state: ElementState,
}

impl BackendEvent for GlutinMouseInputEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl PointerButtonEvent for GlutinMouseInputEvent {
    fn button(&self) -> MouseButton {
        self.button.into()
    }

    fn state(&self) -> MouseButtonState {
        self.state.into()
    }
}

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `TouchDownEvent`
pub struct GlutinTouchStartedEvent {
    window: Rc<Window>,
    time: u32,
    location: (f64, f64),
    id: u64,
}

impl BackendEvent for GlutinTouchStartedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchDownEvent for GlutinTouchStartedEvent {
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
        cmp::min(self.location.0 as i32 * width as i32 / self.window.get_inner_size_points().unwrap_or((width, 0)).0 as i32, 0) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        cmp::min(self.location.1 as i32 * height as i32 / self.window.get_inner_size_points().unwrap_or((0, height)).1 as i32, 0) as u32
    }
}

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `TouchMotionEvent`
pub struct GlutinTouchMovedEvent {
    window: Rc<Window>,
    time: u32,
    location: (f64, f64),
    id: u64,
}

impl BackendEvent for GlutinTouchMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchMotionEvent for GlutinTouchMovedEvent {
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

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `TouchUpEvent`
pub struct GlutinTouchEndedEvent {
    time: u32,
    id: u64,
}

impl BackendEvent for GlutinTouchEndedEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchUpEvent for GlutinTouchEndedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

#[derive(Clone)]
/// Glutin-Backend internal event wrapping glutin's types into a `TouchCancelEvent`
pub struct GlutinTouchCancelledEvent {
    time: u32,
    id: u64,
}

impl BackendEvent for GlutinTouchCancelledEvent {
    fn time(&self) -> u32 {
        self.time
    }
}

impl TouchCancelEvent for GlutinTouchCancelledEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

impl InputBackend for GlutinInputBackend {
    type InputConfig = ();
    type EventError = GlutinInputError;

    type KeyboardKeyEvent = GlutinKeyboardInputEvent;
    type PointerAxisEvent = GlutinMouseWheelEvent;
    type PointerButtonEvent = GlutinMouseInputEvent;
    type PointerMotionEvent = ();
    type PointerMotionAbsoluteEvent = GlutinMouseMovedEvent;
    type TouchDownEvent = GlutinTouchStartedEvent;
    type TouchUpEvent = GlutinTouchEndedEvent;
    type TouchMotionEvent = GlutinTouchMovedEvent;
    type TouchCancelEvent = GlutinTouchCancelledEvent;
    type TouchFrameEvent = ();

    fn set_handler<H: InputHandler<Self> + 'static>(&mut self, mut handler: H) {
        if self.handler.is_some() {
            self.clear_handler();
        }
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
            handler.on_seat_destroyed(&self.seat);
        }
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
    /// `process_new_events` again after the `Window` has been closed is considered an
    /// application error and unspecified baviour may occur.
    ///
    /// The linked `GlutinWindowedRenderer` will error with a lost Context and should
    /// not be used anymore as well.
    fn dispatch_new_events(&mut self) -> Result<(), GlutinInputError> {
        for event in self.window.poll_events() {
            if let Some(ref mut handler) = self.handler {
                match event {
                    Event::KeyboardInput(state, key_code, _) => {
                        match state {
                            ElementState::Pressed => self.key_counter += 1,
                            ElementState::Released => self.key_counter = self.key_counter.checked_sub(1).unwrap_or(0),
                        };
                        handler.on_keyboard_key(&self.seat,
                                                GlutinKeyboardInputEvent {
                                                    time: self.time_counter,
                                                    key: key_code,
                                                    count: self.key_counter,
                                                    state: state,
                                                })
                    }
                    Event::MouseMoved(x, y) => {
                        handler.on_pointer_move_absolute(&self.seat, GlutinMouseMovedEvent {
                            window: self.window.clone(),
                            time: self.time_counter,
                            x: x,
                            y: y,
                        })
                    }
                    Event::MouseWheel(delta, _) => {
                        let event = GlutinMouseWheelEvent {
                                                            axis: Axis::Horizontal,
                                                            time: self.time_counter,
                                                            delta: delta,
                                                          };
                        match delta {
                            MouseScrollDelta::LineDelta(x, y) | MouseScrollDelta::PixelDelta(x, y) => {
                                if x != 0.0 {
                                    handler.on_pointer_axis(&self.seat, event.clone());
                                }
                                if y != 0.0 {
                                    handler.on_pointer_axis(&self.seat, event);
                                }
                            }
                        }
                    }
                    Event::MouseInput(state, button) => {
                        handler.on_pointer_button(&self.seat, GlutinMouseInputEvent {
                            time: self.time_counter,
                            button: button,
                            state: state,
                        })
                    }
                    Event::Touch(Touch {
                                     phase: TouchPhase::Started,
                                     location: (x, y),
                                     id,
                                 }) => {
                        handler.on_touch_down(&self.seat,
                                        GlutinTouchStartedEvent {
                                            window: self.window.clone(),
                                            time: self.time_counter,
                                            location: (x, y),
                                            id: id,
                                        })
                    }
                    Event::Touch(Touch {
                                     phase: TouchPhase::Moved,
                                     location: (x, y),
                                     id,
                                 }) => {
                        handler.on_touch_motion(&self.seat,
                                        GlutinTouchMovedEvent {
                                            window: self.window.clone(),
                                            time: self.time_counter,
                                            location: (x, y),
                                            id: id,
                                        })
                    }
                    Event::Touch(Touch {
                                     phase: TouchPhase::Ended,
                                     location: (x, y),
                                     id,
                                 }) => {
                        handler.on_touch_motion(&self.seat,
                                        GlutinTouchMovedEvent {
                                            window: self.window.clone(),
                                            time: self.time_counter,
                                            location: (x, y),
                                            id: id,
                                        });
                        handler.on_touch_up(&self.seat,
                                        GlutinTouchEndedEvent {
                                            time: self.time_counter,
                                            id: id,
                                        });
                    }
                    Event::Touch(Touch {
                                     phase: TouchPhase::Cancelled,
                                     id,
                                     ..
                                 }) => {
                        handler.on_touch_cancel(&self.seat,
                                        GlutinTouchCancelledEvent {
                                            time: self.time_counter,
                                            id: id,
                                        })
                    }
                    Event::Closed => return Err(GlutinInputError::WindowClosed),
                    _ => {}
                }
                self.time_counter += 1;
            }
        }
        Ok(())
    }
}

impl GlutinInputBackend {
    fn new(window: Rc<Window>) -> GlutinInputBackend {
        GlutinInputBackend {
            window: window,
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
        }
    }
}

impl From<GlutinApi> for Api {
    fn from(api: GlutinApi) -> Self {
        match api {
            GlutinApi::OpenGl => Api::OpenGl,
            GlutinApi::OpenGlEs => Api::OpenGlEs,
            GlutinApi::WebGl => Api::WebGl,
        }
    }
}

impl From<GlutinPixelFormat> for PixelFormat {
    fn from(format: GlutinPixelFormat) -> Self {
        PixelFormat {
            hardware_accelerated: format.hardware_accelerated,
            color_bits: format.color_bits,
            alpha_bits: format.alpha_bits,
            depth_bits: format.depth_bits,
            stencil_bits: format.stencil_bits,
            stereoscopy: format.stereoscopy,
            double_buffer: format.double_buffer,
            multisampling: format.multisampling,
            srgb: format.srgb,
        }
    }
}

impl From<GlutinMouseButton> for MouseButton {
    fn from(button: GlutinMouseButton) -> MouseButton {
        match button {
            GlutinMouseButton::Left => MouseButton::Left,
            GlutinMouseButton::Right => MouseButton::Right,
            GlutinMouseButton::Middle => MouseButton::Middle,
            GlutinMouseButton::Other(num) => MouseButton::Other(num),
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
