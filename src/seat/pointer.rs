use std::sync::{Arc, Mutex};
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

#[derive(Clone)]
pub struct PointerHandle {
    inner: Arc<Mutex<PointerInternal>>,
}

impl PointerHandle {
    pub(crate) fn new_pointer(&self, pointer: wl_pointer::WlPointer) {
        let mut guard = self.inner.lock().unwrap();
        guard.known_pointers.push(pointer);
    }

    pub fn motion(&self, location: Option<(&wl_surface::WlSurface, f64, f64)>, serial: u32, time: u32) {
        let mut guard = self.inner.lock().unwrap();
        // do we leave a surface ?
        let mut leave = true;
        if let Some(ref focus) = guard.focus {
            if let Some((ref surface, _, _)) = location {
                if focus.equals(surface) {
                    leave = false;
                }
            }
        }
        if leave {
            guard.with_focused_pointers(|pointer, surface| {
                pointer.leave(serial, surface);
            });
            guard.focus = None;
        }

        // do we enter one ?
        if let Some((surface, x, y)) = location {
            if guard.focus.is_none() {
                guard.focus = surface.clone();
                guard.with_focused_pointers(|pointer, surface| {
                    pointer.enter(serial, surface, x, y);
                })
            } else {
                // we were on top of a surface and remained on it
                guard.with_focused_pointers(|pointer, surface| {
                    pointer.motion(time, x, y);
                })
            }
        }
    }

    pub fn button(&self, button: u32, state: wl_pointer::ButtonState, serial: u32, time: u32) {
        let mut guard = self.inner.lock().unwrap();
        guard.with_focused_pointers(|pointer, _| {
            pointer.button(serial, time, button, state);
        })
    }

    // TODO: handle axis

    pub(crate) fn cleanup_old_pointers(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard
            .known_pointers
            .retain(|p| p.status() != Liveness::Dead);
    }
}

pub(crate) fn create_pointer_handler() -> PointerHandle {
    PointerHandle {
        inner: Arc::new(Mutex::new(PointerInternal::new())),
    }
}
