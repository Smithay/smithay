use std::sync::{Arc, Mutex, MutexGuard};
use wayland_server::{Liveness, Resource};
use wayland_server::protocol::{wl_pointer, wl_surface};

// TODO: handle pointer surface role

struct PointerInternal {
    known_pointers: Vec<wl_pointer::WlPointer>,
    focus: Option<wl_surface::WlSurface>,
}

impl PointerInternal {
    fn new() -> PointerInternal {
        PointerInternal {
            known_pointers: Vec::new(),
            focus: None,
        }
    }

    fn with_focused_pointers<F>(&self, mut f: F)
    where
        F: FnMut(&wl_pointer::WlPointer, &wl_surface::WlSurface),
    {
        if let Some(ref focus) = self.focus {
            for ptr in &self.known_pointers {
                if ptr.same_client_as(focus) {
                    f(ptr, focus)
                }
            }
        }
    }
}

/// An handle to a keyboard handler
///
/// It can be cloned and all clones manipulate the same internal state. Clones
/// can also be sent across threads.
///
/// This handle gives you access to an interface to send pointer events to your
/// clients.
#[derive(Clone)]
pub struct PointerHandle {
    inner: Arc<Mutex<PointerInternal>>,
}

impl PointerHandle {
    pub(crate) fn new_pointer(&self, pointer: wl_pointer::WlPointer) {
        let mut guard = self.inner.lock().unwrap();
        guard.known_pointers.push(pointer);
    }

    /// Notify that the pointer moved
    ///
    /// You provide the new location of the pointer, in the form of:
    ///
    /// - `None` if the pointer is not on top of a client surface
    /// - `Some(surface, x, y)` if the pointer is focusing surface `surface`,
    ///   at location `(x, y)` relative to this surface
    ///
    /// This will internally take care of notifying the appropriate client objects
    /// of enter/motion/leave events.
    pub fn motion(&self, location: Option<(&wl_surface::WlSurface, f64, f64)>, serial: u32, time: u32) {
        let mut guard = self.inner.lock().unwrap();
        // do we leave a surface ?
        let mut leave = true;
        if let Some(ref focus) = guard.focus {
            if let Some((surface, _, _)) = location {
                if focus.equals(surface) {
                    leave = false;
                }
            }
        }
        if leave {
            guard.with_focused_pointers(|pointer, surface| {
                pointer.leave(serial, surface);
                if pointer.version() >= 5 {
                    pointer.frame();
                }
            });
            guard.focus = None;
        }

        // do we enter one ?
        if let Some((surface, x, y)) = location {
            if guard.focus.is_none() {
                guard.focus = surface.clone();
                guard.with_focused_pointers(|pointer, surface| {
                    pointer.enter(serial, surface, x, y);
                    if pointer.version() >= 5 {
                        pointer.frame();
                    }
                })
            } else {
                // we were on top of a surface and remained on it
                guard.with_focused_pointers(|pointer, _| {
                    pointer.motion(time, x, y);
                    if pointer.version() >= 5 {
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
    pub fn button(&self, button: u32, state: wl_pointer::ButtonState, serial: u32, time: u32) {
        let guard = self.inner.lock().unwrap();
        guard.with_focused_pointers(|pointer, _| {
            pointer.button(serial, time, button, state);
            if pointer.version() >= 5 {
                pointer.frame();
            }
        })
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple scroll events as if they happended in the same instance.
    /// Dropping the returned `PointerAxisHandle` will group the events together.
    pub fn axis<'a>(&'a self) -> PointerAxisHandle<'a> {
        PointerAxisHandle {
            inner: self.inner.lock().unwrap(),
        }
    }

    pub(crate) fn cleanup_old_pointers(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard
            .known_pointers
            .retain(|p| p.status() != Liveness::Dead);
    }
}

/// A frame of pointer axis events.
///
/// Can be used with the builder pattern, e.g.:
/// ```ignore
/// pointer.axis()
///     .source(AxisSource::Wheel)
///     .discrete(Axis::Vertical, 6)
///     .value(Axis::Vertical, 30, time)
///     .stop(Axis::Vertical);
/// ```
pub struct PointerAxisHandle<'a> {
    inner: MutexGuard<'a, PointerInternal>,
}

impl<'a> PointerAxisHandle<'a> {
    /// Specify the source of the axis events
    ///
    /// This event is optional, if no source is known, you can ignore this call.
    /// Only one source event is allowed per frame.
    ///
    /// Using the `AxisSource::Finger` requires a stop event to be send,
    /// when the user lifts off the finger (not necessarily in the same frame).
    pub fn source(&mut self, source: wl_pointer::AxisSource) -> &mut Self {
        self.inner.with_focused_pointers(|pointer, _| {
            if pointer.version() >= 5 {
                pointer.axis_source(source);
            }
        });
        self
    }

    /// Specify discrete scrolling steps additionally to the computed value.
    ///
    /// This event is optional and gives the client additional information about
    /// the nature of the axis event. E.g. a scroll wheel might issue separate steps,
    /// while a touchpad may never issue this event as it has no steps.
    pub fn discrete(&mut self, axis: wl_pointer::Axis, steps: i32) -> &mut Self {
        self.inner.with_focused_pointers(|pointer, _| {
            if pointer.version() >= 5 {
                pointer.axis_discrete(axis, steps);
            }
        });
        self
    }

    /// The actual scroll value. This event is the only required one, but can also
    /// be send multiple times. The values off one frame will be accumulated by the client.
    pub fn value(&mut self, axis: wl_pointer::Axis, value: f64, time: u32) -> &mut Self {
        self.inner.with_focused_pointers(|pointer, _| {
            pointer.axis(time, axis, value);
        });
        self
    }

    /// Notification of stop of scrolling on an axis.
    ///
    /// This event is required for sources of the `AxisSource::Finger` type
    /// and otherwise optional.
    pub fn stop(&mut self, axis: wl_pointer::Axis, time: u32) -> &mut Self {
        self.inner.with_focused_pointers(|pointer, _| {
            if pointer.version() >= 5 {
                pointer.axis_stop(time, axis);
            }
        });
        self
    }

    /// Finish this event
    ///
    /// This will group all axis calls together.
    /// Note: They are already submitted to the client, obmitting this call just
    /// leaves room for additional events.
    pub fn done(&mut self) {
        self.inner.with_focused_pointers(|pointer, _| {
            if pointer.version() >= 5 {
                pointer.frame();
            }
        })
    }
}

pub(crate) fn create_pointer_handler() -> PointerHandle {
    PointerHandle {
        inner: Arc::new(Mutex::new(PointerInternal::new())),
    }
}
