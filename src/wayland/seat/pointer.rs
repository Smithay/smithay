use std::{cell::RefCell, fmt, ops::Deref as _, rc::Rc, sync::Mutex};

use wayland_server::{
    protocol::{
        wl_pointer::{self, Axis, AxisSource, ButtonState, Request, WlPointer},
        wl_surface::WlSurface,
    },
    DispatchData, Filter, Main,
};

use crate::{
    utils::{Logical, Point},
    wayland::{compositor, Serial},
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

struct PointerInternal {
    known_pointers: Vec<WlPointer>,
    focus: Option<(WlSurface, Point<i32, Logical>)>,
    pending_focus: Option<(WlSurface, Point<i32, Logical>)>,
    location: Point<f64, Logical>,
    grab: GrabStatus,
    pressed_buttons: Vec<u32>,
    image_callback: Box<dyn FnMut(CursorImageStatus)>,
}

// image_callback does not implement debug, so we have to impl Debug manually
impl fmt::Debug for PointerInternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointerInternal")
            .field("known_pointers", &self.known_pointers)
            .field("focus", &self.focus)
            .field("pending_focus", &self.pending_focus)
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
            known_pointers: Vec::new(),
            focus: None,
            pending_focus: None,
            location: (0.0, 0.0).into(),
            grab: GrabStatus::None,
            pressed_buttons: Vec::new(),
            image_callback: Box::new(cb) as Box<_>,
        }
    }

    fn with_focused_pointers<F>(&self, mut f: F)
    where
        F: FnMut(&WlPointer, &WlSurface),
    {
        if let Some((ref focus, _)) = self.focus {
            if !focus.as_ref().is_alive() {
                return;
            }
            for ptr in &self.known_pointers {
                if ptr.as_ref().same_client_as(focus.as_ref()) {
                    f(ptr, focus)
                }
            }
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
                // If this grab is associated with a surface that is no longer alive, discard it
                if let Some((ref surface, _)) = handler.start_data().focus {
                    if !surface.as_ref().is_alive() {
                        self.grab = GrabStatus::None;
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
    pub(crate) fn new_pointer(&self, pointer: WlPointer) {
        let mut guard = self.inner.borrow_mut();
        guard.known_pointers.push(pointer);
    }

    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: PointerGrab + 'static>(&self, grab: G, serial: Serial) {
        self.inner.borrow_mut().grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this pointer, resetting it to the default behavior
    pub fn unset_grab(&self) {
        self.inner.borrow_mut().grab = GrabStatus::None;
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
    pub fn grab_start_data(&self) -> Option<GrabStartData> {
        let guard = self.inner.borrow();
        match &guard.grab {
            GrabStatus::Active(_, g) => Some(g.start_data().clone()),
            _ => None,
        }
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
    pub fn motion<D: std::any::Any>(
        &self,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
        ddata: &mut D,
    ) {
        let mut inner = self.inner.borrow_mut();
        inner.pending_focus = focus.clone();
        inner.with_grab(move |mut handle, grab| {
            grab.motion(
                &mut handle,
                location,
                focus,
                serial,
                time,
                DispatchData::wrap(ddata),
            );
        });
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button<D: std::any::Any>(
        &self,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
        ddata: &mut D,
    ) {
        let mut inner = self.inner.borrow_mut();
        match state {
            ButtonState::Pressed => {
                inner.pressed_buttons.push(button);
            }
            ButtonState::Released => {
                inner.pressed_buttons.retain(|b| *b != button);
            }
            _ => unreachable!(),
        }
        inner.with_grab(|mut handle, grab| {
            grab.button(
                &mut handle,
                button,
                state,
                serial,
                time,
                DispatchData::wrap(ddata),
            );
        });
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple scroll events as if they happened in the same instance.
    pub fn axis<D: std::any::Any>(&self, details: AxisFrame, ddata: &mut D) {
        let ddata = DispatchData::wrap(ddata);
        self.inner.borrow_mut().with_grab(|mut handle, grab| {
            grab.axis(&mut handle, details, ddata);
        });
    }

    /// Access the current location of this pointer in the global space
    pub fn current_location(&self) -> Point<f64, Logical> {
        self.inner.borrow().location
    }
}

/// Data about the event that started the grab.
#[derive(Debug, Clone)]
pub struct GrabStartData {
    /// The focused surface and its location, if any, at the start of the grab.
    ///
    /// The location coordinates are in the global compositor space.
    pub focus: Option<(WlSurface, Point<i32, Logical>)>,
    /// The button that initiated the grab.
    pub button: u32,
    /// The location of the click that initiated the grab, in the global compositor space.
    pub location: Point<f64, Logical>,
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
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
        ddata: DispatchData<'_>,
    );
    /// A button press was reported
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
        ddata: DispatchData<'_>,
    );
    /// An axis scroll was reported
    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame, ddata: DispatchData<'_>);
    /// The data about the event that started the grab.
    fn start_data(&self) -> &GrabStartData;
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
    pub fn set_grab<G: PointerGrab + 'static>(&mut self, serial: Serial, grab: G) {
        self.inner.grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this pointer, resetting it to the default behavior
    ///
    /// This will also restore the focus of the underlying pointer
    pub fn unset_grab(&mut self, serial: Serial, time: u32) {
        self.inner.grab = GrabStatus::None;
        // restore the focus
        let location = self.current_location();
        let focus = self.inner.pending_focus.clone();
        self.motion(location, focus, serial, time);
    }

    /// Access the current focus of this pointer
    pub fn current_focus(&self) -> Option<&(WlSurface, Point<i32, Logical>)> {
        self.inner.focus.as_ref()
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
    /// - The surface on top of which the cursor is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the pointer is not
    ///   on top of a client surface).
    ///
    /// This will internally take care of notifying the appropriate client objects
    /// of enter/motion/leave events.
    pub fn motion(
        &mut self,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        // do we leave a surface ?
        let mut leave = true;
        self.inner.location = location;
        if let Some((ref current_focus, _)) = self.inner.focus {
            if let Some((ref surface, _)) = focus {
                if current_focus.as_ref().equals(surface.as_ref()) {
                    leave = false;
                }
            }
        }
        if leave {
            self.inner.with_focused_pointers(|pointer, surface| {
                pointer.leave(serial.into(), surface);
                if pointer.as_ref().version() >= 5 {
                    pointer.frame();
                }
            });
            self.inner.focus = None;
            (self.inner.image_callback)(CursorImageStatus::Default);
        }

        // do we enter one ?
        if let Some((surface, surface_location)) = focus {
            let entered = self.inner.focus.is_none();
            // in all cases, update the focus, the coordinates of the surface
            // might have changed
            self.inner.focus = Some((surface, surface_location));
            let (x, y) = (location - surface_location.to_f64()).into();
            if entered {
                self.inner.with_focused_pointers(|pointer, surface| {
                    pointer.enter(serial.into(), surface, x, y);
                    if pointer.as_ref().version() >= 5 {
                        pointer.frame();
                    }
                })
            } else {
                // we were on top of a surface and remained on it
                self.inner.with_focused_pointers(|pointer, _| {
                    pointer.motion(time, x, y);
                    if pointer.as_ref().version() >= 5 {
                        pointer.frame();
                    }
                })
            }
        }
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        self.inner.with_focused_pointers(|pointer, _| {
            pointer.button(serial.into(), time, button, state);
            if pointer.as_ref().version() >= 5 {
                pointer.frame();
            }
        })
    }

    /// Notify that an axis was scrolled
    ///
    /// This will internally send the appropriate axis events to the client
    /// objects matching with the currently focused surface.
    pub fn axis(&mut self, details: AxisFrame) {
        self.inner.with_focused_pointers(|pointer, _| {
            // axis
            if details.axis.0 != 0.0 {
                pointer.axis(details.time, Axis::HorizontalScroll, details.axis.0);
            }
            if details.axis.1 != 0.0 {
                pointer.axis(details.time, Axis::VerticalScroll, details.axis.1);
            }
            if pointer.as_ref().version() >= 5 {
                // axis source
                if let Some(source) = details.source {
                    pointer.axis_source(source);
                }
                // axis discrete
                if details.discrete.0 != 0 {
                    pointer.axis_discrete(Axis::HorizontalScroll, details.discrete.0);
                }
                if details.discrete.1 != 0 {
                    pointer.axis_discrete(Axis::VerticalScroll, details.discrete.1);
                }
                // stop
                if details.stop.0 {
                    pointer.axis_stop(details.time, Axis::HorizontalScroll);
                }
                if details.stop.1 {
                    pointer.axis_stop(details.time, Axis::VerticalScroll);
                }
                // frame
                pointer.frame();
            }
        });
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
            Axis::HorizontalScroll => {
                self.discrete.0 = steps;
            }
            Axis::VerticalScroll => {
                self.discrete.1 = steps;
            }
            _ => unreachable!(),
        };
        self
    }

    /// The actual scroll value. This event is the only required one, but can also
    /// be send multiple times. The values off one frame will be accumulated by the client.
    pub fn value(mut self, axis: Axis, value: f64) -> Self {
        match axis {
            Axis::HorizontalScroll => {
                self.axis.0 = value;
            }
            Axis::VerticalScroll => {
                self.axis.1 = value;
            }
            _ => unreachable!(),
        };
        self
    }

    /// Notification of stop of scrolling on an axis.
    ///
    /// This event is required for sources of the [`AxisSource::Finger`] type
    /// and otherwise optional.
    pub fn stop(mut self, axis: Axis) -> Self {
        match axis {
            Axis::HorizontalScroll => {
                self.stop.0 = true;
            }
            Axis::VerticalScroll => {
                self.stop.1 = true;
            }
            _ => unreachable!(),
        };
        self
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

pub(crate) fn implement_pointer(pointer: Main<WlPointer>, handle: Option<&PointerHandle>) -> WlPointer {
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
                        if focus.as_ref().same_client_as(pointer.as_ref()) {
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

    if let Some(h) = handle {
        let inner = h.inner.clone();
        pointer.assign_destructor(Filter::new(move |pointer: WlPointer, _, _| {
            inner
                .borrow_mut()
                .known_pointers
                .retain(|p| !p.as_ref().equals(pointer.as_ref()))
        }))
    }

    pointer.deref().clone()
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
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
        _ddata: DispatchData<'_>,
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
        _ddata: DispatchData<'_>,
    ) {
        handle.button(button, state, serial, time);
        handle.set_grab(
            serial,
            ClickGrab {
                start_data: GrabStartData {
                    focus: handle.current_focus().cloned(),
                    button,
                    location: handle.current_location(),
                },
            },
        );
    }
    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame, _ddata: DispatchData<'_>) {
        handle.axis(details);
    }
    fn start_data(&self) -> &GrabStartData {
        unreachable!()
    }
}

// A click grab, basic grab started when an user clicks a surface
// to maintain it focused until the user releases the click.
//
// In case the user maintains several simultaneous clicks, release
// the grab once all are released.
struct ClickGrab {
    start_data: GrabStartData,
}

impl PointerGrab for ClickGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        _focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
        _ddata: DispatchData<'_>,
    ) {
        handle.motion(location, self.start_data.focus.clone(), serial, time);
    }
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
        _ddata: DispatchData<'_>,
    ) {
        handle.button(button, state, serial, time);
        if handle.current_pressed().is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab(serial, time);
        }
    }
    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame, _ddata: DispatchData<'_>) {
        handle.axis(details);
    }
    fn start_data(&self) -> &GrabStartData {
        &self.start_data
    }
}
