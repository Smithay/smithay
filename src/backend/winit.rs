//! Implementation of backend traits for types provided by `winit`

use crate::backend::egl::display::EGLDisplay;
use crate::backend::{
    egl::{context::GlAttributes, native, EGLContext, EGLSurface, Error as EGLError},
    renderer::{
        Renderer, Bind, Transform,
        gles2::{Gles2Renderer, Gles2Error, Gles2Texture},
    },
    input::{
        Axis, AxisSource, Event as BackendEvent, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
        MouseButton, MouseButtonState, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent,
        Seat, SeatCapabilities, TouchCancelEvent, TouchDownEvent, TouchMotionEvent, TouchSlot, TouchUpEvent,
        UnusedEvent,
    },
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::Instant,
};
use cgmath::Matrix3;
use wayland_egl as wegl;
use wayland_server::Display;
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_shm, wl_buffer};
use winit::{
    dpi::{LogicalPosition, LogicalSize, PhysicalSize},
    event::{
        ElementState, Event, KeyboardInput, MouseButton as WinitMouseButton, MouseScrollDelta, Touch,
        TouchPhase, WindowEvent,
    },
    event_loop::{ControlFlow, EventLoop},
    platform::desktop::EventLoopExtDesktop,
    platform::unix::WindowExtUnix,
    window::{Window as WinitWindow, WindowBuilder},
};

#[cfg(feature = "use_system_lib")]
use crate::backend::egl::{display::EGLBufferReader};

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

#[derive(Debug, Clone)]
pub struct WindowSize {
    pub physical_size: PhysicalSize<u32>,
    pub scale_factor: f64,
}

/// Window with an active EGL Context created by `winit`. Implements the
/// [`EGLGraphicsBackend`] and [`GLGraphicsBackend`] graphics backend trait
#[derive(Debug)]
pub struct WinitGraphicsBackend {
    renderer: Gles2Renderer,
    display: EGLDisplay,
    egl: Rc<EGLSurface>,
    window: Rc<WinitWindow>,
    size: Rc<RefCell<WindowSize>>,
}

/// Abstracted event loop of a [`WinitWindow`] implementing the [`InputBackend`] trait
///
/// You need to call [`dispatch_new_events`](InputBackend::dispatch_new_events)
/// periodically to receive any events.
#[derive(Debug)]
pub struct WinitInputBackend {
    egl: Rc<EGLSurface>,
    window: Rc<WinitWindow>,
    events_loop: EventLoop<()>,
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
            version: (3, 0),
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
            EGLSurface::new(&display, context.pixel_format().unwrap(), reqs.double_buffer, context.config_id(), surface, log.clone())
                .map_err(EGLError::CreationFailed)?
        } else if let Some(xlib_window) = winit_window.xlib_window().map(native::XlibWindow) {
            debug!(log, "Winit backend: X11");
            EGLSurface::new(&display, context.pixel_format().unwrap(), reqs.double_buffer, context.config_id(), xlib_window, log.clone())
                .map_err(EGLError::CreationFailed)?
        } else {
            unreachable!("No backends for winit other then Wayland and X11 are supported")
        };

        context.unbind();
        
        (
            display,
            context,
            surface,
        )
    };

    let size = Rc::new(RefCell::new(WindowSize {
        physical_size: winit_window.inner_size(), // TODO: original code check if window is alive or not using inner_size().expect()
        scale_factor: winit_window.scale_factor(),
    }));

    let window = Rc::new(winit_window);
    let egl = Rc::new(surface);
    let mut renderer = unsafe { Gles2Renderer::new(context, log.clone())? };

    Ok((
        WinitGraphicsBackend {
            window: window.clone(),
            display,
            egl: egl.clone(),
            renderer,
            size: size.clone(),
        },
        WinitInputBackend {
            events_loop,
            window,
            egl,
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
#[derive(Debug)]
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

#[cfg(feature = "use_system_lib")]
impl WinitGraphicsBackend {
    pub fn bind_wl_display(&self, wl_display: &Display) -> Result<EGLBufferReader, EGLError> {
        self.display.bind_wl_display(wl_display)
    }

    pub fn window_size(&self) -> WindowSize {
        self.size.borrow().clone()
    }

    pub fn window(&self) -> &WinitWindow {
        &*self.window
    }

    pub fn begin(&mut self) -> Result<(), Gles2Error> {
        let (width, height) = {
            let size = self.size.borrow();
            size.physical_size.into()
        };
        
        self.renderer.bind(self.egl.clone())?;
        self.renderer.begin(width, height, Transform::Normal)
    }
}

impl Renderer for WinitGraphicsBackend {
    type Error = Gles2Error;
    type Texture = Gles2Texture;

    #[cfg(feature = "image")]
    fn import_bitmap<C: std::ops::Deref<Target=[u8]>>(&mut self, image: &image::ImageBuffer<image::Rgba<u8>, C>) -> Result<Self::Texture, Self::Error> {
        self.renderer.import_bitmap(image)
    }
     
    #[cfg(feature = "wayland_frontend")]
    fn shm_formats(&self) -> &[wl_shm::Format] {
        Renderer::shm_formats(&self.renderer)
    }

    #[cfg(feature = "wayland_frontend")]
    fn import_shm(&mut self, buffer: &wl_buffer::WlBuffer) -> Result<Self::Texture, Self::Error> {
        self.renderer.import_shm(buffer)
    }

    fn begin(&mut self, width: u32, height: u32, transform: Transform) -> Result<(), <Self as Renderer>::Error> {
        self.renderer.bind(self.egl.clone())?;
        self.renderer.begin(width, height, transform)
    }

    fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error> {
        self.renderer.clear(color)
    }

    fn render_texture(&mut self, texture: &Self::Texture, matrix: Matrix3<f32>, alpha: f32) -> Result<(), Self::Error> {
        self.renderer.render_texture(texture, matrix, alpha)
    }

    fn finish(&mut self) -> Result<(), crate::backend::SwapBuffersError> {
        self.renderer.finish()?;
        self.egl.swap_buffers()?;
        Ok(())
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

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerMotionAbsoluteEvent`]
#[derive(Debug, Clone)]
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

    fn x_transformed(&self, width: u32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).width;
        f64::max(self.logical_position.x * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: u32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).height;
        f64::max(self.logical_position.y * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerAxisEvent`]
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerButtonEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchDownEvent`]
#[derive(Debug, Clone)]
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

    fn x_transformed(&self, width: u32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).width;
        f64::max(self.location.0 * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: u32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).height;
        f64::max(self.location.1 * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchMotionEvent`]
#[derive(Debug, Clone)]
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

    fn x_transformed(&self, width: u32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).width;
        f64::max(self.location.0 * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: u32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.physical_size.to_logical::<f64>(wsize.scale_factor).height;
        f64::max(self.location.1 * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a `TouchUpEvent`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchCancelEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug)]
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
            let egl = &self.egl;
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
                                let scale_factor = window.scale_factor();
                                let mut wsize = window_size.borrow_mut();
                                wsize.physical_size = psize;
                                wsize.scale_factor = scale_factor;
                                egl.resize(psize.width as i32, psize.height as i32, 0, 0);
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
                                egl.resize(new_psize.width as i32, new_psize.height as i32, 0, 0);
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

impl From<ElementState> for MouseButtonState {
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => MouseButtonState::Pressed,
            ElementState::Released => MouseButtonState::Released,
        }
    }
}
