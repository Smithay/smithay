use std::{
    fmt,
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::{
        wl_pointer::{self, Axis, ButtonState, Request, WlPointer},
        wl_surface::WlSurface,
    },
    DelegateDispatch, Dispatch, DisplayHandle, Resource,
};

use crate::{
    utils::{Logical, Point},
    wayland::{compositor, Serial},
};

use super::{SeatHandler, SeatState};

mod grab;
use grab::{DefaultGrab, GrabStatus};
pub use grab::{GrabStartData, PointerGrab};

mod cursor_image;
pub use cursor_image::{CursorImageAttributes, CursorImageStatus, CURSOR_IMAGE_ROLE};

mod events;
pub use events::{AxisFrame, ButtonEvent, MotionEvent};

struct PointerInternal<D> {
    known_pointers: Vec<WlPointer>,
    focus: Option<(WlSurface, Point<i32, Logical>)>,
    pending_focus: Option<(WlSurface, Point<i32, Logical>)>,
    location: Point<f64, Logical>,
    grab: GrabStatus<D>,
    pressed_buttons: Vec<u32>,
    image_callback: Box<dyn FnMut(CursorImageStatus) + Send + Sync>,
}

// image_callback does not implement debug, so we have to impl Debug manually
impl<D> fmt::Debug for PointerInternal<D> {
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

impl<D> PointerInternal<D> {
    fn new(image_callback: Box<dyn FnMut(CursorImageStatus) + Send + Sync>) -> Self {
        Self {
            known_pointers: Vec::new(),
            focus: None,
            pending_focus: None,
            location: (0.0, 0.0).into(),
            grab: GrabStatus::None,
            pressed_buttons: Vec::new(),
            image_callback,
        }
    }

    fn set_grab<G: PointerGrab<D> + 'static>(&mut self, serial: Serial, grab: G, time: u32) {
        self.grab = GrabStatus::Active(serial, Box::new(grab));
        // generate a move to let the grab change the focus or move the pointer as result of its initialization
        let location = self.location;
        let focus = self.focus.clone();
        self.motion(location, focus, serial, time);
    }

    fn unset_grab(&mut self, serial: Serial, time: u32) {
        self.grab = GrabStatus::None;
        // restore the focus
        let location = self.location;
        let focus = self.pending_focus.clone();
        self.motion(location, focus, serial, time);
    }

    fn motion(
        &mut self,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        // do we leave a surface ?
        let mut leave = true;
        self.location = location;
        if let Some((ref current_focus, _)) = self.focus {
            if let Some((ref surface, _)) = focus {
                if current_focus == surface {
                    leave = false;
                }
            }
        }
        if leave {
            self.with_focused_pointers(|pointer, surface| {
                pointer.leave(serial.into(), surface);
                if pointer.version() >= 5 {
                    pointer.frame();
                }
            });
            self.focus = None;
            (self.image_callback)(CursorImageStatus::Default);
        }

        // do we enter one ?
        if let Some((surface, surface_location)) = focus {
            let entered = self.focus.is_none();
            // in all cases, update the focus, the coordinates of the surface
            // might have changed
            self.focus = Some((surface, surface_location));
            let (x, y) = (location - surface_location.to_f64()).into();
            if entered {
                self.with_focused_pointers(|pointer, surface| {
                    pointer.enter(serial.into(), surface, x, y);
                    if pointer.version() >= 5 {
                        pointer.frame();
                    }
                })
            } else {
                // we were on top of a surface and remained on it
                self.with_focused_pointers(|pointer, _| {
                    pointer.motion(time, x, y);
                    if pointer.version() >= 5 {
                        pointer.frame();
                    }
                })
            }
        }
    }

    fn with_focused_pointers<F>(&self, mut f: F)
    where
        F: FnMut(&WlPointer, &WlSurface),
    {
        use crate::utils::IsAlive;
        if let Some((ref focus, _)) = self.focus {
            if !focus.alive() {
                return;
            }
            for ptr in &self.known_pointers {
                if ptr.id().same_client_as(&focus.id()) {
                    f(ptr, focus)
                }
            }
        }
    }

    fn with_grab<F>(&mut self, dh: &DisplayHandle, f: F)
    where
        F: FnOnce(&DisplayHandle, PointerInnerHandle<'_, D>, &mut dyn PointerGrab<D>),
    {
        let mut grab = ::std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => panic!("Accessed a pointer grab from within a pointer grab access."),
            GrabStatus::Active(_, ref mut handler) => {
                // If this grab is associated with a surface that is no longer alive, discard it
                if let Some((ref surface, _)) = handler.start_data().focus {
                    // TODO: is this alive check still needed?
                    // This is is_alive check
                    if dh.object_info(surface.id()).is_err() {
                        self.grab = GrabStatus::None;
                        f(dh, PointerInnerHandle { inner: self }, &mut DefaultGrab);
                        return;
                    }
                }
                f(dh, PointerInnerHandle { inner: self }, &mut **handler);
            }
            GrabStatus::None => {
                f(dh, PointerInnerHandle { inner: self }, &mut DefaultGrab);
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
#[derive(Debug)]
pub struct PointerHandle<D> {
    inner: Arc<Mutex<PointerInternal<D>>>,
}

impl<D> Clone for PointerHandle<D> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<D> PointerHandle<D> {
    pub(crate) fn new<F>(cb: F) -> PointerHandle<D>
    where
        F: FnMut(CursorImageStatus) + Send + Sync + 'static,
    {
        PointerHandle {
            inner: Arc::new(Mutex::new(PointerInternal::new(Box::new(cb)))),
        }
    }

    pub(crate) fn new_pointer(&self, pointer: WlPointer) {
        let mut guard = self.inner.lock().unwrap();
        guard.known_pointers.push(pointer);
    }

    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: PointerGrab<D> + 'static>(&self, grab: G, serial: Serial, time: u32) {
        self.inner.lock().unwrap().set_grab(serial, grab, time);
    }

    /// Remove any current grab on this pointer, resetting it to the default behavior
    pub fn unset_grab(&self, serial: Serial, time: u32) {
        self.inner.lock().unwrap().unset_grab(serial, time);
    }

    /// Check if this pointer is currently grabbed with this serial
    pub fn has_grab(&self, serial: Serial) -> bool {
        let guard = self.inner.lock().unwrap();
        match guard.grab {
            GrabStatus::Active(s, _) => s == serial,
            _ => false,
        }
    }

    /// Check if this pointer is currently being grabbed
    pub fn is_grabbed(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        !matches!(guard.grab, GrabStatus::None)
    }

    /// Returns the start data for the grab, if any.
    pub fn grab_start_data(&self) -> Option<GrabStartData> {
        let guard = self.inner.lock().unwrap();
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
    pub fn motion(&self, data: &mut D, dh: &DisplayHandle, event: &MotionEvent) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending_focus = event.focus.clone();
        inner.with_grab(dh, move |dh, mut handle, grab| {
            grab.motion(data, dh, &mut handle, event);
        });
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&self, data: &mut D, dh: &DisplayHandle, event: &ButtonEvent) {
        let mut inner = self.inner.lock().unwrap();
        match event.state {
            ButtonState::Pressed => {
                inner.pressed_buttons.push(event.button);
            }
            ButtonState::Released => {
                inner.pressed_buttons.retain(|b| *b != event.button);
            }
            _ => unreachable!(),
        }
        inner.with_grab(dh, |dh, mut handle, grab| {
            grab.button(data, dh, &mut handle, event);
        });
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple scroll events as if they happened in the same instance.
    pub fn axis(&self, data: &mut D, dh: &DisplayHandle, details: AxisFrame) {
        self.inner.lock().unwrap().with_grab(dh, |dh, mut handle, grab| {
            grab.axis(data, dh, &mut handle, details);
        });
    }

    /// Access the current location of this pointer in the global space
    pub fn current_location(&self) -> Point<f64, Logical> {
        self.inner.lock().unwrap().location
    }
}

/// This inner handle is accessed from inside a pointer grab logic, and directly
/// sends event to the client
#[derive(Debug)]
pub struct PointerInnerHandle<'a, D> {
    inner: &'a mut PointerInternal<D>,
}

impl<'a, D> PointerInnerHandle<'a, D> {
    /// Change the current grab on this pointer to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: PointerGrab<D> + 'static>(&mut self, serial: Serial, time: u32, grab: G) {
        self.inner.set_grab(serial, grab, time);
    }

    /// Remove any current grab on this pointer, resetting it to the default behavior
    ///
    /// This will also restore the focus of the underlying pointer
    pub fn unset_grab(&mut self, serial: Serial, time: u32) {
        self.inner.unset_grab(serial, time);
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
        self.inner.motion(location, focus, serial, time);
    }

    /// Notify that a button was pressed
    ///
    /// This will internally send the appropriate button event to the client
    /// objects matching with the currently focused surface.
    pub fn button(&self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        self.inner.with_focused_pointers(|pointer, _| {
            pointer.button(serial.into(), time, button, state);
            if pointer.version() >= 5 {
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
            if pointer.version() >= 5 {
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

/*
 * Grabs definition
 */

/// User data for pointer
#[derive(Debug)]
pub struct PointerUserData<D> {
    pub(crate) handle: Option<PointerHandle<D>>,
}

impl<D> DelegateDispatch<WlPointer, PointerUserData<D>, D> for SeatState<D>
where
    D: Dispatch<WlPointer, PointerUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        pointer: &WlPointer,
        request: wl_pointer::Request,
        data: &PointerUserData<D>,
        dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            Request::SetCursor {
                surface,
                hotspot_x,
                hotspot_y,
                ..
            } => {
                if let Some(ref handle) = data.handle {
                    let mut guard = handle.inner.lock().unwrap();
                    // only allow setting the cursor icon if the current pointer focus
                    // is of the same client
                    let PointerInternal { ref focus, .. } = *guard;
                    if let Some((ref focus, _)) = *focus {
                        if focus.id().same_client_as(&pointer.id()) {
                            match surface {
                                Some(surface) => {
                                    // tolerate re-using the same surface
                                    if compositor::give_role(&surface, CURSOR_IMAGE_ROLE).is_err()
                                        && compositor::get_role(&surface) != Some(CURSOR_IMAGE_ROLE)
                                    {
                                        pointer.post_error(
                                            dh,
                                            wl_pointer::Error::Role,
                                            "Given wl_surface has another role.",
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
                                    });

                                    (guard.image_callback)(CursorImageStatus::Image(surface));
                                }
                                None => {
                                    (guard.image_callback)(CursorImageStatus::Hidden);
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
    }

    fn destroyed(_state: &mut D, _: ClientId, object_id: ObjectId, data: &PointerUserData<D>) {
        if let Some(ref handle) = data.handle {
            handle
                .inner
                .lock()
                .unwrap()
                .known_pointers
                .retain(|p| p.id() != object_id);
        }
    }
}
