use crate::{
    utils::{Logical, Point},
    wayland::Serial,
};
use std::{
    cell::{Ref, RefCell},
    fmt,
    rc::Rc,
};

#[cfg(feature = "wayland_frontend")]
use std::convert::TryFrom;
#[cfg(feature = "wayland_frontend")]
use wayland_server::{
    protocol::{
        wl_pointer::{
            self, Axis as WlAxis, AxisSource as WlAxisSource, ButtonState as WlButtonState, Request,
            WlPointer,
        },
        wl_surface::WlSurface,
    },
    Filter, Main,
};

static CURSOR_IMAGE_ROLE: &str = "cursor_image";

/// The role representing a surface set as the pointer cursor
#[derive(Debug, Default, Copy, Clone)]
pub struct CursorImageAttributes {
    /// Location of the hotspot of the pointer in the surface
    pub hotspot: Point<i32, Logical>,
}

/// Possible status of a cursor as requested by clients
#[derive(Debug, Clone, PartialEq)]
pub enum CursorImageStatus {
    /// The cursor should be hidden
    Hidden,
    /// The compositor should draw its cursor
    Default,
    /// The cursor should be drawn using this surface as an image
    #[cfg(feature = "wayland_frontend")]
    Image(WlSurface),
}

enum GrabStatus {
    None,
    Active(Serial, Box<dyn PointerGrab>),
    Borrowed,
}

// PointerGrab is a trait, so we have to impl Debug manually
impl fmt::Debug for GrabStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrabStatus::None => f.debug_tuple("GrabStatus::None").finish(),
            GrabStatus::Active(serial, _) => f.debug_tuple("GrabStatus::Active").field(&serial).finish(),
            GrabStatus::Borrowed => f.debug_tuple("GrabStatus::Borrowed").finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[cfg(feature = "wayland_frontend")]
impl From<Axis> for WlAxis {
    fn from(axis: Axis) -> WlAxis {
        match axis {
            Axis::Horizontal => WlAxis::HorizontalScroll,
            Axis::Vertical => WlAxis::VerticalScroll,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unknown Axis {0:?}")]
pub struct UnknownAxis(WlAxis);

#[cfg(feature = "wayland_frontend")]
impl TryFrom<WlAxis> for Axis {
    type Error = UnknownAxis;
    fn try_from(value: WlAxis) -> Result<Self, Self::Error> {
        match value {
            WlAxis::HorizontalScroll => Ok(Axis::Horizontal),
            WlAxis::VerticalScroll => Ok(Axis::Vertical),
            x => Err(UnknownAxis(x)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisSource {
    Wheel,
    Finger,
    Continuous,
    WheelTilt,
}

impl From<crate::backend::input::AxisSource> for AxisSource {
    fn from(source: crate::backend::input::AxisSource) -> AxisSource {
        use crate::backend::input::AxisSource::*;
        match source {
            Continuous => AxisSource::Continuous,
            Finger => AxisSource::Finger,
            Wheel => AxisSource::Wheel,
            WheelTilt => AxisSource::WheelTilt,
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl From<AxisSource> for WlAxisSource {
    fn from(axis: AxisSource) -> WlAxisSource {
        match axis {
            AxisSource::Wheel => WlAxisSource::Wheel,
            AxisSource::Finger => WlAxisSource::Finger,
            AxisSource::Continuous => WlAxisSource::Continuous,
            AxisSource::WheelTilt => WlAxisSource::WheelTilt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonState {
    Pressed,
    Released,
}

impl From<crate::backend::input::ButtonState> for ButtonState {
    fn from(state: crate::backend::input::ButtonState) -> ButtonState {
        use crate::backend::input::ButtonState::*;
        match state {
            Pressed => ButtonState::Pressed,
            Released => ButtonState::Released,
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl From<ButtonState> for WlButtonState {
    fn from(state: ButtonState) -> WlButtonState {
        match state {
            ButtonState::Pressed => WlButtonState::Pressed,
            ButtonState::Released => WlButtonState::Released,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unknown ButtonState {0:?}")]
pub struct UnknownButtonState(WlButtonState);

#[cfg(feature = "wayland_frontend")]
impl TryFrom<WlButtonState> for ButtonState {
    type Error = UnknownButtonState;
    fn try_from(value: WlButtonState) -> Result<Self, Self::Error> {
        match value {
            WlButtonState::Pressed => Ok(ButtonState::Pressed),
            WlButtonState::Released => Ok(ButtonState::Released),
            x => Err(UnknownButtonState(x)),
        }
    }
}

pub trait PointerHandler
where
    Self: std::any::Any + 'static,
{
    fn enter(&mut self, x: f64, y: f64, serial: Serial, time: u32);
    fn leave(&mut self, serial: Serial, time: u32);
    fn motion(&mut self, x: f64, y: f64, serial: Serial, time: u32);
    fn button(&mut self, button: u32, state: ButtonState, serial: Serial, time: u32);
    fn axis(&mut self, frame: AxisFrame);

    fn is_alive(&self) -> bool;
    fn same_handler_as(&self, other: &dyn PointerHandler) -> bool;
    fn as_any(&self) -> &dyn std::any::Any;
}

impl PointerHandler for Box<dyn PointerHandler> {
    fn enter(&mut self, x: f64, y: f64, serial: Serial, time: u32) {
        PointerHandler::enter(&mut **self, x, y, serial, time)
    }
    fn leave(&mut self, serial: Serial, time: u32) {
        PointerHandler::leave(&mut **self, serial, time)
    }
    fn motion(&mut self, x: f64, y: f64, serial: Serial, time: u32) {
        PointerHandler::motion(&mut **self, x, y, serial, time);
    }
    fn button(&mut self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        PointerHandler::button(&mut **self, button, state, serial, time)
    }
    fn axis(&mut self, frame: AxisFrame) {
        PointerHandler::axis(&mut **self, frame);
    }

    fn is_alive(&self) -> bool {
        PointerHandler::is_alive(&**self)
    }
    fn same_handler_as(&self, other: &dyn PointerHandler) -> bool {
        PointerHandler::same_handler_as(&**self, other)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        (**self).as_any()
    }
}

struct DummyHandler;
impl PointerHandler for DummyHandler {
    fn enter(&mut self, _x: f64, _y: f64, _serial: Serial, _time: u32) {
        unimplemented!()
    }
    fn leave(&mut self, _serial: Serial, _time: u32) {
        unimplemented!()
    }
    fn motion(&mut self, _x: f64, _y: f64, _serial: Serial, _time: u32) {
        unimplemented!()
    }
    fn button(&mut self, _button: u32, _state: ButtonState, _serial: Serial, _time: u32) {
        unimplemented!()
    }
    fn axis(&mut self, _frame: AxisFrame) {
        unimplemented!()
    }

    fn is_alive(&self) -> bool {
        unimplemented!()
    }
    fn same_handler_as(&self, _other: &dyn PointerHandler) -> bool {
        unimplemented!()
    }
    fn as_any<'a>(&'a self) -> &dyn std::any::Any {
        unimplemented!()
    }
}

struct PointerInternal {
    focus: Option<(Box<dyn PointerHandler>, Point<i32, Logical>)>,
    pending_focus: Option<(Box<dyn PointerHandler>, Point<i32, Logical>)>,
    location: Point<f64, Logical>,
    grab: GrabStatus,
    pressed_buttons: Vec<u32>,
    image_callback: Box<dyn FnMut(CursorImageStatus)>,
}

// image_callback does not implement debug, so we have to impl Debug manually
impl fmt::Debug for PointerInternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerInternal")
            .field("focus", &"...")
            .field("pending_focus", &"...")
            .field("location", &self.location)
            .field("grab", &self.grab)
            .field("pressed_buttons", &self.pressed_buttons)
            .field("image_callback", &"...")
            .finish()
    }
}

impl PointerInternal {
    fn new<F>(cb: F) -> PointerInternal
    where
        F: FnMut(CursorImageStatus) + 'static,
    {
        PointerInternal {
            focus: None,
            pending_focus: None,
            location: (0.0, 0.0).into(),
            grab: GrabStatus::None,
            pressed_buttons: Vec::new(),
            image_callback: Box::new(cb) as Box<_>,
        }
    }

    fn set_grab<G: PointerGrab + 'static>(&mut self, serial: Serial, grab: G, _time: u32) {
        self.grab = GrabStatus::Active(serial, Box::new(grab));
    }

    fn unset_grab(&mut self, serial: Serial, time: u32) {
        self.grab = GrabStatus::None;
        let new_focus = self.pending_focus.take();
        self.motion(self.location, new_focus, serial, time)
    }

    fn motion(
        &mut self,
        location: Point<f64, Logical>,
        focus: Option<(impl PointerHandler, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        let focus = focus.map(|(h, l)| (Box::new(h), l));
        // do we leave a surface ?
        let mut leave = true;
        self.location = location;
        if let Some((ref current_focus, _)) = self.focus {
            if let Some((ref elem, _)) = focus {
                if current_focus.same_handler_as(elem.as_ref()) {
                    leave = false;
                }
            }
        }
        if leave {
            if let Some((mut elem, _)) = self.focus.take() {
                elem.leave(serial.into(), time);
            }
            (self.image_callback)(CursorImageStatus::Default);
        }

        // do we enter one ?
        if let Some((mut elem, elem_location)) = focus {
            let entered = self.focus.is_none();
            // in all cases, update the focus, the coordinates of the surface
            // might have changed
            let (x, y) = (location - elem_location.to_f64()).into();
            if entered {
                elem.enter(x, y, serial.into(), time);
            } else {
                elem.motion(x, y, serial.into(), time);
            }
            self.focus = Some((elem, elem_location));
        }
    }

    fn button(&mut self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        if let Some((ref mut handler, _loc)) = self.focus.as_mut() {
            handler.button(button, state, serial, time);
        }
    }

    fn axis(&mut self, details: AxisFrame) {
        if let Some((ref mut handler, _loc)) = self.focus.as_mut() {
            handler.axis(details);
        }
    }

    fn with_grab<F>(&mut self, f: F)
    where
        F: FnOnce(PointerInnerHandle<'_>, &mut dyn PointerGrab),
    {
        let mut grab = ::std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => panic!("Accessed a pointer grab from within a pointer grab access."),
            GrabStatus::Active(_, ref mut handler) => {
                // If this grab is associated with an object that is no longer alive, discard it
                if let Some((ref elem, _)) = handler.start_data().focus {
                    if !elem.is_alive() {
                        self.grab = GrabStatus::None;
                        if let Some(restore) = self.pending_focus.take() {
                            //self.motion(self.location, Some(restore), serial, time)
                        }
                        f(PointerInnerHandle { inner: self }, &mut DefaultGrab);
                        return;
                    }
                }
                f(PointerInnerHandle { inner: self }, &mut **handler);
            }
            GrabStatus::None => {
                f(PointerInnerHandle { inner: self }, &mut DefaultGrab);
            }
        }

        if let GrabStatus::Borrowed = self.grab {
            // the grab has not been ended nor replaced, put it back in place
            self.grab = grab;
        }
    }
}

/// An handle to a pointer handler
///
/// It can be cloned and all clones manipulate the same internal state.
///
/// This handle gives you access to an interface to send pointer events to your
/// clients.
///
/// When sending events using this handle, they will be intercepted by a pointer
/// grab if any is active. See the [`PointerGrab`] trait for details.
#[derive(Debug, Clone)]
pub struct PointerHandle {
    inner: Rc<RefCell<PointerInternal>>,
}

impl PointerHandle {
    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: PointerGrab + 'static>(&self, grab: G, serial: Serial, time: u32) {
        self.inner.borrow_mut().set_grab(serial, grab, time);
    }

    /// Remove any current grab on this pointer, resetting it to the default behavior
    pub fn unset_grab(&self, serial: Serial, time: u32) {
        self.inner.borrow_mut().unset_grab(serial, time);
    }

    /// Check if this pointer is currently grabbed with this serial
    pub fn has_grab(&self, serial: Serial) -> bool {
        let guard = self.inner.borrow_mut();
        match guard.grab {
            GrabStatus::Active(s, _) => s == serial,
            _ => false,
        }
    }

    /// Check if this pointer is currently being grabbed
    pub fn is_grabbed(&self) -> bool {
        let guard = self.inner.borrow_mut();
        !matches!(guard.grab, GrabStatus::None)
    }

    /// Returns the start data for the grab, if any.
    pub fn grab_start_data<'a>(&'a self) -> Option<Ref<'a, GrabStartData>> {
        let guard = self.inner.borrow();
        if matches!(guard.grab, GrabStatus::Active(_, _)) {
            Some(Ref::map(guard, |g| match &g.grab {
                GrabStatus::Active(_, ref g) => g.start_data(),
                _ => unreachable!(),
            }))
        } else {
            None
        }
    }

    /// Returns the start data for the grab, if any, removing it from the current grab
    pub fn take_grab_start_data<'a>(&'a self) -> Option<GrabStartData> {
        let mut guard = self.inner.borrow_mut();
        match guard.grab {
            GrabStatus::Active(_, ref mut g) => Some(g.take_start_data()),
            _ => None,
        }
    }

    /// Notify that the pointer moved
    ///
    /// You provide the new location of the pointer, in the form of:
    ///
    /// - The coordinates of the pointer in the global compositor space
    /// - The element on top of which the cursor is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the pointer is not
    ///   on top of a client surface).
    pub fn motion(
        &self,
        location: Point<f64, Logical>,
        focus: Option<(impl PointerHandler, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        let mut inner = self.inner.borrow_mut();
        inner.with_grab(move |mut handle, grab| {
            grab.motion(
                &mut handle,
                location,
                focus.map(|(h, l)| (Box::new(h) as Box<dyn PointerHandler>, l)),
                serial,
                time,
            );
        });
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        let mut inner = self.inner.borrow_mut();
        match state {
            ButtonState::Pressed => {
                inner.pressed_buttons.push(button);
            }
            ButtonState::Released => {
                inner.pressed_buttons.retain(|b| *b != button);
            }
        }
        inner.with_grab(|mut handle, grab| {
            grab.button(&mut handle, button, state, serial, time);
        });
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple scroll events as if they happened in the same instance.
    pub fn axis(&self, details: AxisFrame) {
        self.inner.borrow_mut().with_grab(|mut handle, grab| {
            grab.axis(&mut handle, details);
        });
    }

    /// Access the current location of this pointer in the global space
    pub fn current_location(&self) -> Point<f64, Logical> {
        self.inner.borrow().location
    }
}

/// Data about the event that started the grab.
pub struct GrabStartData {
    /// The focused surface and its location, if any, at the start of the grab.
    ///
    /// The location coordinates are in the global compositor space.
    pub focus: Option<(Box<dyn PointerHandler>, Point<i32, Logical>)>,
    /// The button that initiated the grab.
    pub button: u32,
    /// The location of the click that initiated the grab, in the global compositor space.
    pub location: Point<f64, Logical>,
}

impl fmt::Debug for GrabStartData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrabStartData")
            .field("focus", &"...")
            .field("button", &self.button)
            .field("location", &self.location)
            .finish()
    }
}

/// A trait to implement a pointer grab
///
/// In some context, it is necessary to temporarily change the behavior of the pointer. This is
/// typically known as a pointer grab. A typical example would be, during a drag'n'drop operation,
/// the underlying surfaces will no longer receive classic pointer event, but rather special events.
///
/// This trait is the interface to intercept regular pointer events and change them as needed, its
/// interface mimics the [`PointerHandle`] interface.
///
/// If your logic decides that the grab should end, both [`PointerInnerHandle`] and [`PointerHandle`] have
/// a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the server),
/// the struct implementing this trait will be dropped. As such you should put clean-up logic in the destructor,
/// rather than trying to guess when the grab will end.
pub trait PointerGrab {
    /// A motion was reported
    ///
    /// This method allows you attach additional behavior to a motion event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::motion()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the motion event never occurred.
    ///
    /// Some grabs (such as drag'n'drop, shell resize and motion) unset the focus while they are active,
    /// this is achieved by just setting the focus to `None` when invoking `PointerInnerHandle::motion()`.
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        focus: Option<(Box<dyn PointerHandler>, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    );
    /// A button press was reported
    ///
    /// This method allows you attach additional behavior to a button event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::button()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the button event never occurred.
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    );
    /// An axis scroll was reported
    ///
    /// This method allows you attach additional behavior to an axis event, possibly altering it.
    /// You generally will want to invoke `PointerInnerHandle::axis()` as part of your processing. If you
    /// don't, the rest of the compositor will behave as if the axis event never occurred.
    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame);
    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData;
    /// The data about the event that started the grab.
    fn take_start_data(&mut self) -> GrabStartData;
}

/// This inner handle is accessed from inside a pointer grab logic, and directly
/// sends event to the client
#[derive(Debug)]
pub struct PointerInnerHandle<'a> {
    inner: &'a mut PointerInternal,
}

impl<'a> PointerInnerHandle<'a> {
    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: PointerGrab + 'static>(&mut self, serial: Serial, grab: G, time: u32) {
        self.inner.set_grab(serial, grab, time);
    }

    /// Remove any current grab on this pointer, resetting it to the default behavior
    ///
    /// This will also restore the focus of the underlying pointer
    pub fn unset_grab(&mut self, serial: Serial, time: u32) {
        self.inner.unset_grab(serial, time);
    }

    /// Access the current focus of this pointer
    pub fn current_focus(&self) -> Option<(&dyn PointerHandler, Point<i32, Logical>)> {
        self.inner
            .focus
            .as_ref()
            .map(|&(ref focus, loc)| (focus.as_ref(), loc))
    }

    /// Removes and returns the current focus of this pointer
    pub fn take_current_focus(&mut self) -> Option<(Box<dyn PointerHandler>, Point<i32, Logical>)> {
        self.inner.focus.take()
    }

    /// Sets a pending focus to be restored after the currently active grab ends
    pub fn set_pending_focus(&mut self, focus: Option<(Box<dyn PointerHandler>, Point<i32, Logical>)>) {
        if matches!(&self.inner.grab, GrabStatus::Active(_, _)) {
            self.inner.pending_focus = focus;
        }
    }

    /// Access the current location of this pointer in the global space
    pub fn current_location(&self) -> Point<f64, Logical> {
        self.inner.location
    }

    /// A list of the currently physically pressed buttons
    ///
    /// This still includes buttons that your grab have intercepted and not sent
    /// to the client.
    pub fn current_pressed(&self) -> &[u32] {
        &self.inner.pressed_buttons
    }

    /// Notify that the pointer moved
    ///
    /// You provide the new location of the pointer, in the form of:
    ///
    /// - The coordinates of the pointer in the global compositor space
    ///
    /// This will internally take care of notifying the appropriate client objects
    /// of enter/motion/leave events.
    pub fn motion_no_focus(&mut self, location: Point<f64, Logical>, serial: Serial, time: u32) {
        self.inner
            .motion(location, Option::<(DummyHandler, _)>::None, serial, time);
    }

    /// Notify that the pointer moved
    ///
    /// You provide the new location of the pointer, in the form of:
    ///
    /// - The coordinates of the pointer in the global compositor space
    /// - The surface on top of which the cursor is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the pointer is not
    ///   on top of a client surface).
    ///
    /// This will internally take care of notifying the appropriate client objects
    /// of enter/motion/leave events.
    pub fn motion(
        &mut self,
        location: Point<f64, Logical>,
        focus: Option<(impl PointerHandler, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        self.inner.motion(location, focus, serial, time);
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&mut self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        self.inner.button(button, state, serial, time);
    }

    /// Notify that an axis was scrolled
    ///
    /// This will internally send the appropriate axis events to the client
    /// objects matching with the currently focused surface.
    pub fn axis(&mut self, details: AxisFrame) {
        self.inner.axis(details)
    }
}

/// A frame of pointer axis events.
///
/// Can be used with the builder pattern, e.g.:
///
/// ```ignore
/// AxisFrame::new()
///     .source(AxisSource::Wheel)
///     .discrete(Axis::Vertical, 6)
///     .value(Axis::Vertical, 30, time)
///     .stop(Axis::Vertical);
/// ```
#[derive(Copy, Clone, Debug)]
pub struct AxisFrame {
    source: Option<AxisSource>,
    time: u32,
    axis: (f64, f64),
    discrete: (i32, i32),
    stop: (bool, bool),
}

impl AxisFrame {
    /// Create a new frame of axis events
    pub fn new(time: u32) -> Self {
        AxisFrame {
            source: None,
            time,
            axis: (0.0, 0.0),
            discrete: (0, 0),
            stop: (false, false),
        }
    }

    /// Specify the source of the axis events
    ///
    /// This event is optional, if no source is known, you can ignore this call.
    /// Only one source event is allowed per frame.
    ///
    /// Using the [`AxisSource::Finger`] requires a stop event to be send,
    /// when the user lifts off the finger (not necessarily in the same frame).
    pub fn source(mut self, source: AxisSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Specify discrete scrolling steps additionally to the computed value.
    ///
    /// This event is optional and gives the client additional information about
    /// the nature of the axis event. E.g. a scroll wheel might issue separate steps,
    /// while a touchpad may never issue this event as it has no steps.
    pub fn discrete(mut self, axis: Axis, steps: i32) -> Self {
        match axis {
            Axis::Horizontal => {
                self.discrete.0 = steps;
            }
            Axis::Vertical => {
                self.discrete.1 = steps;
            }
        };
        self
    }

    /// The actual scroll value. This event is the only required one, but can also
    /// be send multiple times. The values off one frame will be accumulated by the client.
    pub fn value(mut self, axis: Axis, value: f64) -> Self {
        match axis {
            Axis::Horizontal => {
                self.axis.0 = value;
            }
            Axis::Vertical => {
                self.axis.1 = value;
            }
        };
        self
    }

    /// Notification of stop of scrolling on an axis.
    ///
    /// This event is required for sources of the [`AxisSource::Finger`] type
    /// and otherwise optional.
    pub fn stop(mut self, axis: Axis) -> Self {
        match axis {
            Axis::Horizontal => {
                self.stop.0 = true;
            }
            Axis::Vertical => {
                self.stop.1 = true;
            }
        };
        self
    }
}

/*
 * Grabs definition
 */

// The default grab, the behavior when no particular grab is in progress
struct DefaultGrab;

impl PointerGrab for DefaultGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        focus: Option<(Box<dyn PointerHandler>, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        handle.motion(location, focus, serial, time);
    }
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        handle.button(button, state, serial, time);
        if state == ButtonState::Pressed {
            if let Some(focus) = handle.current_focus().and_then(|(s, l)| {
                s.as_any()
                    .downcast_ref::<WlSurface>()
                    .cloned()
                    .map(|s| (Box::new(s) as Box<dyn PointerHandler>, l))
            }) {
                handle.set_grab(
                    serial,
                    ClickGrab::new(GrabStartData {
                        focus: Some(focus),
                        button,
                        location: handle.current_location(),
                    })
                    .unwrap(),
                    time,
                );
            }
        }
    }
    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame) {
        handle.axis(details);
    }
    fn start_data(&self) -> &GrabStartData {
        unreachable!()
    }
    fn take_start_data(&mut self) -> GrabStartData {
        unreachable!()
    }
}

// A click grab, basic grab started when an user clicks a surface
// to maintain it focused until the user releases the click.
//
// In case the user maintains several simultaneous clicks, release
// the grab once all are released.
struct ClickGrab {
    focus: Option<(WlSurface, Point<i32, Logical>)>,
    start: GrabStartData,
}

impl ClickGrab {
    fn new(mut start: GrabStartData) -> Option<ClickGrab> {
        if let Some(focus) = start
            .focus
            .take()
            .and_then(|(s, l)| s.as_any().downcast_ref::<WlSurface>().cloned().map(|s| (s, l)))
        {
            Some(ClickGrab {
                focus: Some(focus.clone()),
                start: GrabStartData {
                    focus: Some(focus).map(|(s, l)| (Box::new(s) as Box<dyn PointerHandler>, l)),
                    ..start
                },
            })
        } else {
            None
        }
    }
}

impl PointerGrab for ClickGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        focus: Option<(Box<dyn PointerHandler>, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        self.start.location = location;
        handle.set_pending_focus(focus);
        handle.motion(
            location,
            self.focus
                .clone()
                .map(|(s, l)| (Box::new(s) as Box<dyn PointerHandler>, l)),
            serial,
            time,
        );
    }
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        handle.button(button, state, serial, time);
        if handle.current_pressed().is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab(serial, time);
        }
    }
    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame) {
        handle.axis(details);
    }
    fn start_data(&self) -> &GrabStartData {
        &self.start
    }
    fn take_start_data(&mut self) -> GrabStartData {
        GrabStartData {
            focus: self.start.focus.take(),
            ..self.start
        }
    }
}

pub(crate) fn create_pointer_handler<F>(cb: F) -> PointerHandle
where
    F: FnMut(CursorImageStatus) + 'static,
{
    PointerHandle {
        inner: Rc::new(RefCell::new(PointerInternal::new(cb))),
    }
}

#[cfg(feature = "wayland_frontend")]
struct KnownPointers(RefCell<Vec<WlPointer>>);

#[cfg(feature = "wayland_frontend")]
impl PointerHandler for WlSurface {
    fn enter(&mut self, x: f64, y: f64, serial: Serial, _time: u32) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_pointers) = client.data_map().get::<KnownPointers>() {
                for ptr in &*known_pointers.0.borrow() {
                    if ptr.as_ref().same_client_as(self.as_ref()) {
                        ptr.enter(serial.into(), &self, x, y);
                        if ptr.as_ref().version() >= 5 {
                            ptr.frame();
                        }
                    }
                }
            }
        }
    }
    fn leave(&mut self, serial: Serial, _time: u32) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_pointers) = client.data_map().get::<KnownPointers>() {
                for ptr in &*known_pointers.0.borrow() {
                    if ptr.as_ref().same_client_as(self.as_ref()) {
                        ptr.leave(serial.into(), &self);
                        if ptr.as_ref().version() >= 5 {
                            ptr.frame();
                        }
                    }
                }
            }
        }
    }
    fn motion(&mut self, x: f64, y: f64, _serial: Serial, time: u32) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_pointers) = client.data_map().get::<KnownPointers>() {
                for ptr in &*known_pointers.0.borrow() {
                    if ptr.as_ref().same_client_as(self.as_ref()) {
                        ptr.motion(time, x, y);
                        if ptr.as_ref().version() >= 5 {
                            ptr.frame();
                        }
                    }
                }
            }
        }
    }
    fn button(&mut self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_pointers) = client.data_map().get::<KnownPointers>() {
                for ptr in &*known_pointers.0.borrow() {
                    if ptr.as_ref().same_client_as(self.as_ref()) {
                        ptr.button(serial.into(), time, button, state.into());
                        if ptr.as_ref().version() >= 5 {
                            ptr.frame();
                        }
                    }
                }
            }
        }
    }
    fn axis(&mut self, details: AxisFrame) {
        if let Some(client) = self.as_ref().client() {
            if let Some(known_pointers) = client.data_map().get::<KnownPointers>() {
                for ptr in &*known_pointers.0.borrow() {
                    if ptr.as_ref().same_client_as(self.as_ref()) {
                        // axis
                        if details.axis.0 != 0.0 {
                            ptr.axis(details.time, WlAxis::HorizontalScroll, details.axis.0);
                        }
                        if details.axis.1 != 0.0 {
                            ptr.axis(details.time, WlAxis::VerticalScroll, details.axis.1);
                        }
                        if ptr.as_ref().version() >= 5 {
                            // axis source
                            if let Some(source) = details.source {
                                ptr.axis_source(source.into());
                            }
                            // axis discrete
                            if details.discrete.0 != 0 {
                                ptr.axis_discrete(WlAxis::HorizontalScroll, details.discrete.0);
                            }
                            if details.discrete.1 != 0 {
                                ptr.axis_discrete(WlAxis::VerticalScroll, details.discrete.1);
                            }
                            // stop
                            if details.stop.0 {
                                ptr.axis_stop(details.time, WlAxis::HorizontalScroll);
                            }
                            if details.stop.1 {
                                ptr.axis_stop(details.time, WlAxis::VerticalScroll);
                            }
                            // frame
                            ptr.frame();
                        }
                    }
                }
            }
        }
    }

    fn is_alive(&self) -> bool {
        self.as_ref().is_alive()
    }
    fn same_handler_as(&self, other: &dyn PointerHandler) -> bool {
        if let Some(other_surface) = other.as_any().downcast_ref::<WlSurface>() {
            self == other_surface
        } else {
            false
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(feature = "wayland_frontend")]
pub(crate) fn implement_pointer(pointer: Main<WlPointer>, handle: Option<&PointerHandle>) {
    use crate::wayland::compositor;
    use std::{ops::Deref, sync::Mutex};

    let client = pointer.as_ref().client().unwrap();
    {
        let client_data_map = client.data_map();
        client_data_map.insert_if_missing(|| KnownPointers(RefCell::new(Vec::new())));
        client_data_map
            .get::<KnownPointers>()
            .unwrap()
            .0
            .borrow_mut()
            .push(pointer.deref().clone());
    }

    let inner = handle.map(|h| h.inner.clone());
    pointer.quick_assign(move |pointer, request, _data| {
        match request {
            Request::SetCursor {
                surface,
                hotspot_x,
                hotspot_y,
                ..
            } => {
                if let Some(ref inner) = inner {
                    let mut guard = inner.borrow_mut();
                    // only allow setting the cursor icon if the current pointer focus
                    // is of the same client
                    let PointerInternal {
                        ref mut image_callback,
                        ref focus,
                        ..
                    } = *guard;
                    if let Some((ref focus, _)) = *focus {
                        if focus
                            .as_any()
                            .downcast_ref::<WlSurface>()
                            .map(|surf| surf.as_ref().same_client_as(pointer.as_ref()))
                            .unwrap_or(false)
                        {
                            match surface {
                                Some(surface) => {
                                    // tolerate re-using the same surface
                                    if compositor::give_role(&surface, CURSOR_IMAGE_ROLE).is_err()
                                        && compositor::get_role(&surface) != Some(CURSOR_IMAGE_ROLE)
                                    {
                                        pointer.as_ref().post_error(
                                            wl_pointer::Error::Role as u32,
                                            "Given wl_surface has another role.".into(),
                                        );
                                        return;
                                    }
                                    compositor::with_states(&surface, |states| {
                                        states.data_map.insert_if_missing_threadsafe(|| {
                                            Mutex::new(CursorImageAttributes {
                                                hotspot: (0, 0).into(),
                                            })
                                        });
                                        states
                                            .data_map
                                            .get::<Mutex<CursorImageAttributes>>()
                                            .unwrap()
                                            .lock()
                                            .unwrap()
                                            .hotspot = (hotspot_x, hotspot_y).into();
                                    })
                                    .unwrap();

                                    image_callback(CursorImageStatus::Image(surface));
                                }
                                None => {
                                    image_callback(CursorImageStatus::Hidden);
                                }
                            }
                        }
                    }
                }
            }
            Request::Release => {
                // Our destructors already handle it
            }
            _ => unreachable!(),
        }
    });

    pointer.assign_destructor(Filter::new(move |pointer: WlPointer, _, _| {
        client
            .data_map()
            .get::<KnownPointers>()
            .unwrap()
            .0
            .borrow_mut()
            .retain(|p| !p.as_ref().equals(pointer.as_ref()))
    }))
}
