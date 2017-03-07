//! Implementation of backend traits for types provided by `glutin`

use glutin::{ContextError, CreationError, Event, ElementState, MouseScrollDelta, Touch, TouchPhase, GlContext, HeadlessRendererBuilder, HeadlessContext, WindowBuilder, Window};
use glutin::{Api as GlutinApi, PixelFormat as GlutinPixelFormat, MouseButton as GlutinMouseButton};
use nix::c_void;
use std::rc::Rc;

use backend::NewIdType;
use backend::graphics::opengl::{Api, OpenglRenderer, PixelFormat, SwapBuffersError};
use backend::input::{InputBackend, InputHandler, Seat, KeyState, MouseButton, MouseButtonState, Axis, AxisSource, TouchEvent, TouchSlot};

/// Create a new `GlutinHeadlessRenderer` which implements the `OpenglRenderer` graphics
/// backend trait
pub fn init_headless_renderer() -> Result<GlutinHeadlessRenderer, CreationError>
{
    init_headless_renderer_from_builder(HeadlessRendererBuilder::new(1024, 600))
}

/// Create a new `GlutinHeadlessRenderer`, which implements the `OpenglRenderer` graphics
/// backend trait, with a given already configured `HeadlessRendererBuilder` for
/// customization
pub fn init_headless_renderer_from_builder(builder: HeadlessRendererBuilder) -> Result<GlutinHeadlessRenderer, CreationError>
{
    let (w, h) = builder.dimensions;
    let context = builder.build_strict()?;

    Ok(GlutinHeadlessRenderer::new(context, w, h))
}

/// Create a new `GlutinWindowedRenderer`, which implements the `OpenglRenderer` graphics
/// backend trait
pub fn init_windowed_renderer() -> Result<GlutinWindowedRenderer, CreationError>
{
    init_windowed_renderer_from_builder(WindowBuilder::new())
}

/// Create a new `GlutinWindowedRenderer`, which implements the `OpenglRenderer` graphics
/// backend trait, with a given already configured `WindowBuilder` for customization.
pub fn init_windowed_renderer_from_builder(builder: WindowBuilder) -> Result<GlutinWindowedRenderer, CreationError>
{
    let window = Rc::new(builder.build_strict()?);
    Ok(GlutinWindowedRenderer::new(window))
}

/// Create a new `glutin` `Window`. Returns a `GlutinWindowedRenderer` implementing
/// the `OpenglRenderer` graphics backend trait and a `GlutinInputBackend` implementing
/// the `InputBackend` trait.
pub fn init_windowed() -> Result<(GlutinWindowedRenderer, GlutinInputBackend), CreationError>
{
    init_windowed_from_builder(WindowBuilder::new())
}

/// Create a new `glutin` `Window` with a given already configured `WindowBuilder` for
/// customization. Returns a `GlutinWindowedRenderer` implementing
/// the `OpenglRenderer` graphics backend trait and a `GlutinInputBackend` implementing
/// the `InputBackend` trait.
pub fn init_windowed_from_builder(builder: WindowBuilder) -> Result<(GlutinWindowedRenderer, GlutinInputBackend), CreationError>
{
    let window = Rc::new(builder.build_strict()?);
    Ok((
        GlutinWindowedRenderer::new(window.clone()),
        GlutinInputBackend::new(window)
    ))
}

/// Headless Opengl Context created by `glutin`. Implements the `OpenglRenderer` graphics
/// backend trait.
pub struct GlutinHeadlessRenderer
{
    context: HeadlessContext,
    w: u32,
    h: u32,
}

impl GlutinHeadlessRenderer
{
    fn new(context: HeadlessContext, w: u32, h: u32) -> GlutinHeadlessRenderer {
        GlutinHeadlessRenderer {
            context: context,
            w: w,
            h: h,
        }
    }
}

impl OpenglRenderer for GlutinHeadlessRenderer
{
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
/// `OpenglRenderer` graphics backend trait.
pub struct GlutinWindowedRenderer
{
    window: Rc<Window>
}

impl GlutinWindowedRenderer
{
    fn new(window: Rc<Window>) -> GlutinWindowedRenderer {
        GlutinWindowedRenderer {
            window: window,
        }
    }
}

impl OpenglRenderer for GlutinWindowedRenderer
{
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
        let (width, height) = self.window.get_inner_size().unwrap_or((800, 600));      // TODO: 800x600 ?
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
pub enum GlutinInputError
{
    /// The underlying `glutin` `Window` was closed. No further events can be processed.
    ///
    /// See `GlutinInputBackend::process_new_events`.
    WindowClosed
}

/// Abstracted event loop of a `glutin` `Window` implementing the `InputBackend` trait
///
/// You need to call `process_new_events` periodically to receive any events.
pub struct GlutinInputBackend
{
    window: Rc<Window>,
    time_counter: u32,
    seat: Seat,
    handler: Option<Box<InputHandler + 'static>>,
}

impl InputBackend for GlutinInputBackend
{
    fn set_handler<H: InputHandler + 'static>(&mut self, mut handler: H) {
        if self.handler.is_some() {
            self.clear_handler();
        }
        handler.on_seat_created(&self.seat);
        self.handler = Some(Box::new(handler));
    }

    fn get_handler(&mut self) -> Option<&mut InputHandler>
    {
        self.handler.as_mut().map(|handler| handler as &mut InputHandler)
    }

    fn clear_handler(&mut self) {
        if let Some(ref mut handler) = self.handler {
            handler.on_seat_destroyed(&self.seat);
        }
        self.handler = None;
    }

    fn set_cursor_position(&mut self, x: u32, y: u32) -> Result<(), ()> {
        if let Some((win_x, win_y)) = self.window.get_position() {
            self.window.set_cursor_position(win_x + x as i32, win_y + y as i32)
        } else {
            Err(())
        }
    }
}

impl GlutinInputBackend
{
    fn new(window: Rc<Window>) -> GlutinInputBackend
    {
        GlutinInputBackend {
            window: window,
            time_counter: 0,
            seat: Seat::new(0),
            handler: None,
        }
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
    pub fn process_new_events(&mut self) -> Result<(), GlutinInputError>
    {
        for event in self.window.poll_events()
        {
            if let Some(ref mut handler) = self.handler {
                match event {
                    Event::KeyboardInput(state, key_code/*TODO: Is this really the keycode? glutins docs don't tell*/, _) => handler.on_keyboard_key(&self.seat, self.time_counter, key_code as u32, state.into(), 1),
                    Event::MouseMoved(x, y) => handler.on_pointer_move(&self.seat, self.time_counter, (x as u32, y as u32)),
                    Event::MouseWheel(delta, _) => match delta {
                        MouseScrollDelta::LineDelta(x, y) => {
                            if x != 0.0 {
                                handler.on_pointer_scroll(&self.seat, self.time_counter, Axis::Horizontal, AxisSource::Wheel, x as f64);
                            }
                            if y != 0.0 {
                                handler.on_pointer_scroll(&self.seat, self.time_counter, Axis::Vertical, AxisSource::Wheel, y as f64);
                            }
                        },
                        MouseScrollDelta::PixelDelta(x, y) => {
                            if x != 0.0 {
                                handler.on_pointer_scroll(&self.seat, self.time_counter, Axis::Vertical, AxisSource::Continous, x as f64);
                            }
                            if y != 0.0 {
                                handler.on_pointer_scroll(&self.seat, self.time_counter, Axis::Horizontal, AxisSource::Continous, y as f64);
                            }
                        },
                    },
                    Event::MouseInput(state, button) => handler.on_pointer_button(&self.seat, self.time_counter, button.into(), state.into()),
                    Event::Touch(Touch { phase: TouchPhase::Started, location: (x, y), id}) => handler.on_touch(&self.seat, self.time_counter, TouchEvent::Down { slot: Some(TouchSlot::new(id as u32)), x: x, y: y }),
                    Event::Touch(Touch { phase: TouchPhase::Moved, location: (x, y), id}) => handler.on_touch(&self.seat, self.time_counter, TouchEvent::Motion { slot: Some(TouchSlot::new(id as u32)), x: x, y: y }),
                    Event::Touch(Touch { phase: TouchPhase::Ended, location: (x, y), id }) => {
                        handler.on_touch(&self.seat, self.time_counter, TouchEvent::Motion { slot: Some(TouchSlot::new(id as u32)), x: x, y: y });
                        handler.on_touch(&self.seat, self.time_counter, TouchEvent::Up { slot: Some(TouchSlot::new(id as u32)) });
                    }
                    Event::Touch(Touch { phase: TouchPhase::Cancelled, id, ..}) => handler.on_touch(&self.seat, self.time_counter, TouchEvent::Cancel { slot: Some(TouchSlot::new(id as u32)) }),
                    Event::Closed => return Err(GlutinInputError::WindowClosed),
                    _ => {},
                }
                self.time_counter += 1;
            }
        }
        Ok(())
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

impl From<GlutinMouseButton> for MouseButton
{
    fn from(button: GlutinMouseButton) -> MouseButton {
        match button {
            GlutinMouseButton::Left => MouseButton::Left,
            GlutinMouseButton::Right => MouseButton::Right,
            GlutinMouseButton::Middle => MouseButton::Middle,
            GlutinMouseButton::Other(num) => MouseButton::Other(num),
        }
    }
}

impl From<ElementState> for KeyState
{
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        }
    }
}

impl From<ElementState> for MouseButtonState
{
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => MouseButtonState::Pressed,
            ElementState::Released => MouseButtonState::Released,
        }
    }
}
