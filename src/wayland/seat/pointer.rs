use std::sync::{Arc, Mutex};
use wayland_server::{
    protocol::{
        wl_pointer::{Axis, AxisSource, ButtonState, Event, Request, WlPointer},
        wl_surface::WlSurface,
    },
    NewResource, Resource,
};

// TODO: handle pointer surface role

enum GrabStatus {
    None,
    Active(u32, Box<PointerGrab>),
    Borrowed,
}

struct PointerInternal {
    known_pointers: Vec<Resource<WlPointer>>,
    focus: Option<(Resource<WlSurface>, (f64, f64))>,
    location: (f64, f64),
    grab: GrabStatus,
}

impl PointerInternal {
    fn new() -> PointerInternal {
        PointerInternal {
            known_pointers: Vec::new(),
            focus: None,
            location: (0.0, 0.0),
            grab: GrabStatus::None,
        }
    }

    fn with_focused_pointers<F>(&self, mut f: F)
    where
        F: FnMut(&Resource<WlPointer>, &Resource<WlSurface>),
    {
        if let Some((ref focus, _)) = self.focus {
            for ptr in &self.known_pointers {
                if ptr.same_client_as(focus) {
                    f(ptr, focus)
                }
            }
        }
    }

    fn with_grab<F>(&mut self, f: F)
    where
        F: FnOnce(PointerInnerHandle, &mut PointerGrab),
    {
        let mut grab = ::std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => panic!("Accessed a pointer grab from within a pointer grab access."),
            GrabStatus::Active(_, ref mut handler) => {
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
/// It can be cloned and all clones manipulate the same internal state. Clones
/// can also be sent across threads.
///
/// This handle gives you access to an interface to send pointer events to your
/// clients.
///
/// When sending events using this handle, they will be intercepted by a pointer
/// grab if any is active. See the `PointerGrab` trait for details.
#[derive(Clone)]
pub struct PointerHandle {
    inner: Arc<Mutex<PointerInternal>>,
}

impl PointerHandle {
    pub(crate) fn new_pointer(&self, pointer: Resource<WlPointer>) {
        let mut guard = self.inner.lock().unwrap();
        guard.known_pointers.push(pointer);
    }

    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: PointerGrab + 'static>(&self, grab: G, serial: u32) {
        self.inner.lock().unwrap().grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this pointer, reseting it to the default behavior
    pub fn unset_grab(&self) {
        self.inner.lock().unwrap().grab = GrabStatus::None;
    }

    /// Check if this pointer is currently grabbed with this serial
    pub fn has_grab(&self, serial: u32) -> bool {
        let guard = self.inner.lock().unwrap();
        match guard.grab {
            GrabStatus::Active(s, _) => s == serial,
            _ => false,
        }
    }

    /// Check if this pointer is currently being grabbed
    pub fn is_grabbed(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        match guard.grab {
            GrabStatus::None => false,
            _ => true,
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
    pub fn motion(
        &self,
        location: (f64, f64),
        focus: Option<(Resource<WlSurface>, (f64, f64))>,
        serial: u32,
        time: u32,
    ) {
        self.inner.lock().unwrap().with_grab(move |mut handle, grab| {
            grab.motion(&mut handle, location, focus, serial, time);
        });
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&self, button: u32, state: ButtonState, serial: u32, time: u32) {
        self.inner.lock().unwrap().with_grab(|mut handle, grab| {
            grab.button(&mut handle, button, state, serial, time);
        });
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple scroll events as if they happended in the same instance.
    /// Dropping the returned `PointerAxisHandle` will group the events together.
    pub fn axis(&self, details: AxisFrame) {
        self.inner.lock().unwrap().with_grab(|mut handle, grab| {
            grab.axis(&mut handle, details);
        });
    }
}

/// A trait to implement a pointer grab
///
/// In some context, it is necessary to temporarily change the behavior of the pointer. This is
/// typically known as a pointer grab. A typicall example would be, during a drag'n'drop operation,
/// the underlying surfaces will no longer receive classic pointer event, but rather special events.
///
/// This trait is the interface to intercept regular pointer events and change them as needed, its
/// interface mimicks the `PointerHandle` interface.
///
/// If your logic decides that the grab should end, both `PointerInnerHandle` and `PointerHandle` have
/// a method to change it.
///
/// When your grab ends (either as you requested it or if it was forcefully cancelled by the server),
/// the struct implementing this trait will be dropped. As such you should put clean-up logic in the destructor,
/// rather than trying to guess when the grab will end.
pub trait PointerGrab: Send + Sync {
    /// A motion was reported
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle,
        location: (f64, f64),
        focus: Option<(Resource<WlSurface>, (f64, f64))>,
        serial: u32,
        time: u32,
    );
    /// A button press was reported
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle,
        button: u32,
        state: ButtonState,
        serial: u32,
        time: u32,
    );
    /// An axis scroll was reported
    fn axis(&mut self, handle: &mut PointerInnerHandle, details: AxisFrame);
}

/// This inner handle is accessed from inside a pointer grab logic, and directly
/// sends event to the client
pub struct PointerInnerHandle<'a> {
    inner: &'a mut PointerInternal,
}

impl<'a> PointerInnerHandle<'a> {
    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: PointerGrab + 'static>(&mut self, serial: u32, grab: G) {
        self.inner.grab = GrabStatus::Active(serial, Box::new(grab));
    }

    /// Remove any current grab on this pointer, reseting it to the default behavior
    pub fn unset_grab(&mut self) {
        self.inner.grab = GrabStatus::None;
    }

    /// Access the current focus of this pointer
    pub fn current_focus(&self) -> Option<&(Resource<WlSurface>, (f64, f64))> {
        self.inner.focus.as_ref()
    }

    /// Access the current location of this pointer in the global space
    pub fn current_location(&self) -> (f64, f64) {
        self.inner.location
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
        (x, y): (f64, f64),
        focus: Option<(Resource<WlSurface>, (f64, f64))>,
        serial: u32,
        time: u32,
    ) {
        // do we leave a surface ?
        let mut leave = true;
        self.inner.location = (x, y);
        if let Some((ref current_focus, _)) = self.inner.focus {
            if let Some((ref surface, _)) = focus {
                if current_focus.equals(surface) {
                    leave = false;
                }
            }
        }
        if leave {
            self.inner.with_focused_pointers(|pointer, surface| {
                pointer.send(Event::Leave {
                    serial,
                    surface: surface.clone(),
                });
                if pointer.version() >= 5 {
                    pointer.send(Event::Frame);
                }
            });
            self.inner.focus = None;
        }

        // do we enter one ?
        if let Some((surface, (sx, sy))) = focus {
            let entered = self.inner.focus.is_none();
            // in all cases, update the focus, the coordinates of the surface
            // might have changed
            self.inner.focus = Some((surface.clone(), (sx, sy)));
            if entered {
                self.inner.with_focused_pointers(|pointer, surface| {
                    pointer.send(Event::Enter {
                        serial,
                        surface: surface.clone(),
                        surface_x: x - sx,
                        surface_y: y - sy,
                    });
                    if pointer.version() >= 5 {
                        pointer.send(Event::Frame);
                    }
                })
            } else {
                // we were on top of a surface and remained on it
                self.inner.with_focused_pointers(|pointer, _| {
                    pointer.send(Event::Motion {
                        time,
                        surface_x: x - sx,
                        surface_y: y - sy,
                    });
                    if pointer.version() >= 5 {
                        pointer.send(Event::Frame);
                    }
                })
            }
        }
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&self, button: u32, state: ButtonState, serial: u32, time: u32) {
        self.inner.with_focused_pointers(|pointer, _| {
            pointer.send(Event::Button {
                serial,
                time,
                button,
                state,
            });
            if pointer.version() >= 5 {
                pointer.send(Event::Frame);
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
                pointer.send(Event::Axis {
                    time: details.time,
                    axis: Axis::HorizontalScroll,
                    value: details.axis.0,
                });
            }
            if details.axis.1 != 0.0 {
                pointer.send(Event::Axis {
                    time: details.time,
                    axis: Axis::VerticalScroll,
                    value: details.axis.1,
                });
            }
            if pointer.version() >= 5 {
                // axis source
                if let Some(source) = details.source {
                    pointer.send(Event::AxisSource { axis_source: source });
                }
                // axis discrete
                if details.discrete.0 != 0 {
                    pointer.send(Event::AxisDiscrete {
                        axis: Axis::HorizontalScroll,
                        discrete: details.discrete.0,
                    });
                }
                if details.discrete.1 != 0 {
                    pointer.send(Event::AxisDiscrete {
                        axis: Axis::VerticalScroll,
                        discrete: details.discrete.1,
                    });
                }
                // stop
                if details.stop.0 {
                    pointer.send(Event::AxisStop {
                        time: details.time,
                        axis: Axis::HorizontalScroll,
                    });
                }
                if details.stop.1 {
                    pointer.send(Event::AxisStop {
                        time: details.time,
                        axis: Axis::VerticalScroll,
                    });
                }
                // frame
                pointer.send(Event::Frame);
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
    /// Using the `AxisSource::Finger` requires a stop event to be send,
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
        };
        self
    }

    /// Notification of stop of scrolling on an axis.
    ///
    /// This event is required for sources of the `AxisSource::Finger` type
    /// and otherwise optional.
    pub fn stop(mut self, axis: Axis) -> Self {
        match axis {
            Axis::HorizontalScroll => {
                self.stop.0 = true;
            }
            Axis::VerticalScroll => {
                self.stop.1 = true;
            }
        };
        self
    }
}

pub(crate) fn create_pointer_handler() -> PointerHandle {
    PointerHandle {
        inner: Arc::new(Mutex::new(PointerInternal::new())),
    }
}

pub(crate) fn implement_pointer(
    new_pointer: NewResource<WlPointer>,
    handle: Option<&PointerHandle>,
) -> Resource<WlPointer> {
    let destructor = match handle {
        Some(h) => {
            let inner = h.inner.clone();
            Some(move |pointer: Resource<_>| {
                inner
                    .lock()
                    .unwrap()
                    .known_pointers
                    .retain(|p| !p.equals(&pointer))
            })
        }
        None => None,
    };
    new_pointer.implement(
        |request, _pointer| {
            match request {
                Request::SetCursor { .. } => {
                    // TODO
                }
                Request::Release => {
                    // Our destructors already handle it
                }
            }
        },
        destructor,
        (),
    )
}

/*
 * Grabs definition
 */

// The default grab, the behavior when no particular grab is in progress
struct DefaultGrab;

impl PointerGrab for DefaultGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle,
        location: (f64, f64),
        focus: Option<(Resource<WlSurface>, (f64, f64))>,
        serial: u32,
        time: u32,
    ) {
        handle.motion(location, focus, serial, time);
    }
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle,
        button: u32,
        state: ButtonState,
        serial: u32,
        time: u32,
    ) {
        handle.button(button, state, serial, time);
        let current_focus = handle.current_focus().cloned();
        handle.set_grab(
            serial,
            ClickGrab {
                buttons: vec![button],
                current_focus,
                pending_focus: None,
            },
        );
    }
    fn axis(&mut self, handle: &mut PointerInnerHandle, details: AxisFrame) {
        handle.axis(details);
    }
}

// A click grab, basic grab started when an user clicks a surface
// to maintain it focused until the user releases the click.
//
// In case the user maintains several simultaneous clicks, release
// the grab once all are released.
struct ClickGrab {
    buttons: Vec<u32>,
    current_focus: Option<(Resource<WlSurface>, (f64, f64))>,
    pending_focus: Option<(Resource<WlSurface>, (f64, f64))>,
}

impl PointerGrab for ClickGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle,
        location: (f64, f64),
        focus: Option<(Resource<WlSurface>, (f64, f64))>,
        serial: u32,
        time: u32,
    ) {
        // buffer the future focus, but maintain the current one
        self.pending_focus = focus;
        handle.motion(location, self.current_focus.clone(), serial, time);
    }
    fn button(
        &mut self,
        handle: &mut PointerInnerHandle,
        button: u32,
        state: ButtonState,
        serial: u32,
        time: u32,
    ) {
        match state {
            ButtonState::Pressed => self.buttons.push(button),
            ButtonState::Released => self.buttons.retain(|b| *b != button),
        }
        handle.button(button, state, serial, time);
        if self.buttons.is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab();
            // restore the focus
            let location = handle.current_location();
            handle.motion(location, self.pending_focus.clone(), serial, time);
        }
    }
    fn axis(&mut self, handle: &mut PointerInnerHandle, details: AxisFrame) {
        handle.axis(details);
    }
}
