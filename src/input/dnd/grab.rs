use std::{fmt, sync::Arc};

#[cfg(feature = "wayland_frontend")]
use wayland_server::DisplayHandle;

#[cfg(feature = "xwayland")]
use crate::{wayland::seat::WaylandFocus, xwayland::XWaylandClientData};
#[cfg(feature = "xwayland")]
use wayland_server::Resource;

use crate::{
    input::{
        dnd::OfferData,
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
            MotionEvent as PointerMotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{
            DownEvent, GrabStartData as TouchGrabStartData, MotionEvent as TouchMotionEvent, TouchGrab,
            TouchInnerHandle, UpEvent,
        },
        Seat, SeatHandler,
    },
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
};

use super::{DndFocus, Source};

/// Type of interaction that started a DnD grab
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrabType {
    /// Pointer input was validated for the requested grab
    Pointer,
    /// Touch input was validated for the requested grab
    Touch,
}

/// Grab during a client-initiated DnD operation.
pub struct DnDGrab<D: SeatHandler, S: Source, F: DndFocus<D> + 'static> {
    #[cfg(feature = "wayland_frontend")]
    dh: DisplayHandle,
    pointer_start_data: Option<PointerGrabStartData<D>>,
    touch_start_data: Option<TouchGrabStartData<D>>,
    last_position: Point<f64, Logical>,
    data_source: Arc<S>,
    current_focus: Option<F>,
    offer_data: Option<F::OfferData<S>>,
    seat: Seat<D>,
}

impl<D, S, F> fmt::Debug for DnDGrab<D, S, F>
where
    D: SeatHandler + 'static,
    S: Source + fmt::Debug,
    F: DndFocus<D> + fmt::Debug + 'static,
    F::OfferData<S>: fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct("DnDGrab");

        #[cfg(feature = "wayland_frontend")]
        {
            f.field("dh", &self.dh);
        }

        f.field("pointer_start_data", &self.pointer_start_data)
            .field("touch_start_data", &self.touch_start_data)
            .field("last_position", &self.last_position)
            .field("data_source", &self.data_source)
            .field("current_focus", &self.current_focus)
            .field("offer_data", &self.offer_data)
            .field("seat", &self.seat)
            .finish()
    }
}

impl<D: SeatHandler, S: Source> DnDGrab<D, S, D::PointerFocus>
where
    D::PointerFocus: DndFocus<D>,
{
    /// Create a new DnDGrab from an implicit pointer grab for a given source
    pub fn new_pointer(
        #[cfg(feature = "wayland_frontend")] dh: &DisplayHandle,
        start_data: PointerGrabStartData<D>,
        source: S,
        seat: Seat<D>,
    ) -> Self {
        let last_position = start_data.location;
        Self {
            #[cfg(feature = "wayland_frontend")]
            dh: dh.clone(),
            pointer_start_data: Some(start_data),
            touch_start_data: None,
            last_position,
            data_source: Arc::new(source),
            current_focus: None,
            offer_data: None,
            seat,
        }
    }
}

impl<D: SeatHandler, S: Source> DnDGrab<D, S, D::TouchFocus>
where
    D::TouchFocus: DndFocus<D>,
{
    /// Create a new DnDGrab from an implicit touch grab for a given source
    pub fn new_touch(
        #[cfg(feature = "wayland_frontend")] dh: &DisplayHandle,
        start_data: TouchGrabStartData<D>,
        source: S,
        seat: Seat<D>,
    ) -> Self {
        let last_position = start_data.location;
        Self {
            #[cfg(feature = "wayland_frontend")]
            dh: dh.clone(),
            pointer_start_data: None,
            touch_start_data: Some(start_data),
            last_position,
            data_source: Arc::new(source),
            current_focus: None,
            offer_data: None,
            seat,
        }
    }
}

/// Enum over DndFocus candidates receiving a drop from `DnDGrab`
pub enum DndTarget<'a, D: SeatHandler> {
    /// A Pointer-based DnDGrab ended on a `D::PointerFocus`
    Pointer(&'a D::PointerFocus),
    /// A Touch-based DnDGrab ended on a `D::TouchFocus`
    Touch(&'a D::TouchFocus),
}

impl<D: SeatHandler> fmt::Debug for DndTarget<'_, D>
where
    D::PointerFocus: fmt::Debug,
    D::TouchFocus: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pointer(p) => f.debug_tuple("DndTarget::Pointer").field(p).finish(),
            Self::Touch(t) => f.debug_tuple("DndTarget::Touch").field(t).finish(),
        }
    }
}

impl<'a, D: SeatHandler> DndTarget<'a, D> {
    /// Returns the contained `Pointer`-value, consuming `self``.
    ///
    /// ## Panics
    ///
    /// Panics if the self value equals `Touch`.
    pub fn unwrap_pointer(self) -> &'a D::PointerFocus {
        match self {
            DndTarget::Pointer(p) => p,
            DndTarget::Touch(_) => panic!("unwrap_pointer on touch-based dnd grab target"),
        }
    }

    /// Returns the contained `Touch`-value, consuming `self``.
    ///
    /// ## Panics
    ///
    /// Panics if the self value equals `Pointer`.
    pub fn unwrap_touch(self) -> &'a D::TouchFocus {
        match self {
            DndTarget::Pointer(_) => panic!("unwrap_pointer on pointer-based dnd grab target"),
            DndTarget::Touch(t) => t,
        }
    }
}

impl<'a, F, D: SeatHandler<PointerFocus = F, TouchFocus = F>> DndTarget<'a, D> {
    /// Returns the contained value consuming `self`.
    pub fn into_inner(self) -> &'a F {
        match self {
            DndTarget::Pointer(p) => p,
            DndTarget::Touch(t) => t,
        }
    }
}

/// Events that are generated during drag'n'drop
pub trait DndGrabHandler: SeatHandler + Sized {
    /// The drag'n'drop action was finished by the user releasing the pointer button / touch inputs.
    ///
    /// At this point, any icon should be removed.
    ///
    /// * `target` - The target that the contents were dropped on.
    /// * `validated` - Whether the drop offer was negotiated and accepted. If `false`, the drop
    ///   was cancelled or otherwise not successful.
    /// * `seat` - The seat on which the DnD action was finished.
    /// * `location` - The location the drop was finished at
    fn dropped(
        &mut self,
        target: Option<DndTarget<'_, Self>>,
        validated: bool,
        seat: Seat<Self>,
        location: Point<f64, Logical>,
    ) {
        let _ = (target, validated, seat, location);
    }
}

impl<D, S, F> DnDGrab<D, S, F>
where
    D: DndGrabHandler,
    D: SeatHandler,
    D: 'static,
    S: Source,
    F: DndFocus<D> + 'static,
{
    fn update_focus(
        &mut self,
        data: &mut D,
        focus: Option<(F, Point<f64, Logical>)>,
        location: Point<f64, Logical>,
        serial: Serial,
        time: u32,
    ) {
        if self
            .current_focus
            .as_ref()
            .is_some_and(|current| focus.as_ref().is_none_or(|(f, _)| f != current))
        {
            // focus changed, we need to make a leave if appropriate
            if let Some(focus) = self.current_focus.take() {
                // only leave if there is a data source or we are on the original client
                focus.leave(data, self.offer_data.as_mut(), &self.seat);

                // disable the offers
                if let Some(offer_data) = self.offer_data.take() {
                    offer_data.disable();
                }
            }
        }

        if let Some((focus, surface_location)) = focus {
            // early return if the surface is no longer valid
            if !focus.alive() {
                return;
            }

            let (x, y) = (location - surface_location).into();
            if self.current_focus.is_none() {
                if self
                    .data_source
                    .metadata()
                    .is_some_and(|metadata| metadata.mime_types.is_empty())
                {
                    // delay until they have materialized
                    return;
                }

                // We entered a new surface, send the data offer if appropriate
                self.offer_data = focus.enter(
                    data,
                    #[cfg(feature = "wayland_frontend")]
                    &self.dh,
                    self.data_source.clone(),
                    &self.seat,
                    Point::new(x, y),
                    &serial,
                );
                self.current_focus = Some(focus);
            } else {
                // make a move
                focus.motion(data, self.offer_data.as_mut(), &self.seat, Point::new(x, y), time);
            }
        }
    }

    fn drop<'a>(&'a mut self, data: &mut D, into_target: impl Fn(&'a F) -> DndTarget<'a, D>) {
        // the user dropped, proceed to the drop
        let validated = self.offer_data.as_ref().is_some_and(|data| data.validated());
        if let Some(ref focus) = self.current_focus {
            focus.drop(data, self.offer_data.as_mut(), &self.seat);
        }

        if let Some(ref offer_data) = self.offer_data {
            if validated {
                offer_data.drop();
            } else {
                offer_data.disable();
            }
        }

        if !validated {
            self.data_source.cancel();
        } else {
            self.data_source.drop_performed();
        }

        DndGrabHandler::dropped(
            data,
            self.current_focus.as_ref().map(into_target),
            validated,
            self.seat.clone(),
            self.last_position,
        );
        // in all cases abandon the drop
        // no more buttons are pressed, release the grab
        if let Some(ref focus) = self.current_focus {
            focus.leave(data, self.offer_data.as_mut(), &self.seat);
        }
    }
}

impl<D, S> PointerGrab<D> for DnDGrab<D, S, D::PointerFocus>
where
    D: DndGrabHandler,
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: DndFocus<D> + 'static,
    D: 'static,
    S: Source,
{
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &PointerMotionEvent,
    ) {
        handle.motion(data, self.ptr_focus(), event);

        self.last_position = event.location;

        self.update_focus(data, focus, event.location, event.serial, event.time);
    }

    fn relative_motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        _focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, self.ptr_focus(), event);
    }

    fn button(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, event: &ButtonEvent) {
        handle.button(data, event);

        if handle.current_pressed().is_empty() {
            // the user dropped, proceed to the drop
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, details: AxisFrame) {
        // we just forward the axis events as is
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<D> {
        self.pointer_start_data.as_ref().unwrap()
    }

    fn unset(&mut self, data: &mut D) {
        self.drop(data, DndTarget::Pointer);
    }
}

impl<D, S> TouchGrab<D> for DnDGrab<D, S, D::TouchFocus>
where
    D: DndGrabHandler,
    D: SeatHandler,
    <D as SeatHandler>::TouchFocus: DndFocus<D> + 'static,
    D: 'static,
    S: Source,
{
    fn down(
        &mut self,
        _data: &mut D,
        _handle: &mut TouchInnerHandle<'_, D>,
        _focus: Option<(<D as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        _event: &DownEvent,
        _seq: Serial,
    ) {
        // Ignore
    }

    fn up(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, event: &UpEvent, _seq: Serial) {
        if event.slot != self.start_data().slot {
            return;
        }

        handle.unset_grab(self, data);
    }

    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut TouchInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &TouchMotionEvent,
        seq: Serial,
    ) {
        if event.slot != self.start_data().slot {
            return;
        }

        handle.motion(data, self.touch_focus(), event, seq);

        self.last_position = event.location;

        self.update_focus(
            data,
            focus,
            event.location,
            SERIAL_COUNTER.next_serial(),
            event.time,
        );
    }

    fn frame(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, seq: Serial) {
        handle.frame(data, seq);
    }

    fn cancel(&mut self, data: &mut D, handle: &mut TouchInnerHandle<'_, D>, _seq: Serial) {
        // TODO: should we cancel something here?
        handle.unset_grab(self, data);
    }

    fn shape(
        &mut self,
        _data: &mut D,
        _handle: &mut TouchInnerHandle<'_, D>,
        _event: &crate::input::touch::ShapeEvent,
        _seq: Serial,
    ) {
    }

    fn orientation(
        &mut self,
        _data: &mut D,
        _handle: &mut TouchInnerHandle<'_, D>,
        _event: &crate::input::touch::OrientationEvent,
        _seq: Serial,
    ) {
    }

    fn start_data(&self) -> &TouchGrabStartData<D> {
        self.touch_start_data.as_ref().unwrap()
    }

    fn unset(&mut self, data: &mut D) {
        self.drop(data, DndTarget::Touch);
    }
}

#[cfg(not(feature = "xwayland"))]
impl<D, S> DnDGrab<D, S, D::PointerFocus>
where
    D: DndGrabHandler,
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: DndFocus<D> + 'static,
    D: 'static,
    S: Source,
{
    fn ptr_focus(&self) -> Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)> {
        None
    }
}

#[cfg(not(feature = "xwayland"))]
impl<D, S> DnDGrab<D, S, D::TouchFocus>
where
    D: DndGrabHandler,
    D: SeatHandler,
    <D as SeatHandler>::TouchFocus: DndFocus<D> + 'static,
    D: 'static,
    S: Source,
{
    fn touch_focus(&self) -> Option<(<D as SeatHandler>::TouchFocus, Point<f64, Logical>)> {
        None
    }
}

#[cfg(feature = "xwayland")]
impl<D, S> DnDGrab<D, S, D::PointerFocus>
where
    D: DndGrabHandler,
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: DndFocus<D> + 'static,
    D: 'static,
    S: Source,
{
    fn ptr_focus(&self) -> Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)> {
        // While the grab is active, we don't want any focus except for xwayland
        self.pointer_start_data
            .as_ref()?
            .focus
            .clone()
            .filter(|(focus, _)| {
                focus.wl_surface().is_some_and(|s| {
                    s.client()
                        .is_some_and(|c| c.get_data::<XWaylandClientData>().is_some())
                })
            })
    }
}

#[cfg(feature = "xwayland")]
impl<D, S> DnDGrab<D, S, D::TouchFocus>
where
    D: DndGrabHandler,
    D: SeatHandler,
    <D as SeatHandler>::TouchFocus: DndFocus<D> + 'static,
    D: 'static,
    S: Source,
{
    fn touch_focus(&self) -> Option<(<D as SeatHandler>::TouchFocus, Point<f64, Logical>)> {
        // While the grab is active, we don't want any focus except for xwayland
        self.touch_start_data
            .as_ref()?
            .focus
            .clone()
            .filter(|(focus, _)| {
                focus.wl_surface().is_some_and(|s| {
                    s.client()
                        .is_some_and(|c| c.get_data::<XWaylandClientData>().is_some())
                })
            })
    }
}
