use std::{fmt, sync::Arc};

#[cfg(feature = "wayland_frontend")]
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};

use crate::{
    input::{
        dnd::OfferData,
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
            PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{GrabStartData as TouchGrabStartData, TouchGrab},
        Seat, SeatHandler,
    },
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::selection::data_device::{ClientDndGrabHandler, DataDeviceHandler},
};

use super::{DndFocus, Source};

/// Grab during a client-initiated DnD operation.
pub struct DnDGrab<D: SeatHandler, S: Source, F: DndFocus<D> + 'static> {
    #[cfg(feature = "wayland_frontend")]
    dh: DisplayHandle,
    pointer_start_data: Option<PointerGrabStartData<D>>,
    touch_start_data: Option<TouchGrabStartData<D>>,
    data_source: Arc<S>,
    current_focus: Option<F>,
    offer_data: Option<F::OfferData<S>>,
    #[cfg(feature = "wayland_frontend")]
    icon: Option<WlSurface>,
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
        f.debug_struct("DnDGrab")
            .field("dh", &self.dh)
            .field("pointer_start_data", &self.pointer_start_data)
            .field("touch_start_data", &self.touch_start_data)
            .field("data_source", &self.data_source)
            .field("current_focus", &self.current_focus)
            .field("offer_data", &self.offer_data)
            .field("icon", &self.icon)
            .field("seat", &self.seat)
            .finish()
    }
}

impl<D: SeatHandler, S: Source> DnDGrab<D, S, D::PointerFocus>
where
    D::PointerFocus: DndFocus<D>,
{
    pub(crate) fn new_pointer(
        #[cfg(feature = "wayland_frontend")] dh: &DisplayHandle,
        start_data: PointerGrabStartData<D>,
        source: S,
        seat: Seat<D>,
        #[cfg(feature = "wayland_frontend")] icon: Option<WlSurface>,
    ) -> Self {
        Self {
            #[cfg(feature = "wayland_frontend")]
            dh: dh.clone(),
            pointer_start_data: Some(start_data),
            touch_start_data: None,
            data_source: Arc::new(source),
            current_focus: None,
            offer_data: None,
            #[cfg(feature = "wayland_frontend")]
            icon,
            seat,
        }
    }
}

impl<D: SeatHandler, S: Source> DnDGrab<D, S, D::TouchFocus>
where
    D::TouchFocus: DndFocus<D>,
{
    pub(crate) fn new_touch(
        #[cfg(feature = "wayland_frontend")] dh: &DisplayHandle,
        start_data: TouchGrabStartData<D>,
        source: S,
        seat: Seat<D>,
        #[cfg(feature = "wayland_frontend")] icon: Option<WlSurface>,
    ) -> Self {
        Self {
            #[cfg(feature = "wayland_frontend")]
            dh: dh.clone(),
            pointer_start_data: None,
            touch_start_data: Some(start_data),
            data_source: Arc::new(source),
            current_focus: None,
            offer_data: None,
            #[cfg(feature = "wayland_frontend")]
            icon,
            seat,
        }
    }
}

impl<D, S, F> DnDGrab<D, S, F>
where
    D: DataDeviceHandler,
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
        if focus
            .as_ref()
            .zip(self.current_focus.as_ref())
            .is_some_and(|((f, _), current)| f != current)
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

    fn drop(&mut self, data: &mut D) {
        // the user dropped, proceed to the drop
        let validated = self.offer_data.as_ref().is_none_or(|data| data.validated());
        if let Some(ref focus) = self.current_focus {
            if validated {
                focus.drop(data, self.offer_data.as_mut(), &self.seat);
            }
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

        ClientDndGrabHandler::dropped(data, self.current_focus.as_ref(), validated, self.seat.clone());
        self.icon = None;
        // in all cases abandon the drop
        // no more buttons are pressed, release the grab
        if let Some(ref focus) = self.current_focus {
            focus.leave(data, self.offer_data.as_mut(), &self.seat);
        }
    }
}

impl<D, S> PointerGrab<D> for DnDGrab<D, S, D::PointerFocus>
where
    D: DataDeviceHandler,
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
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        self.update_focus(data, focus, event.location, event.serial, event.time);
    }

    fn relative_motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, event: &ButtonEvent) {
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
        self.drop(data);
    }
}

impl<D, S> TouchGrab<D> for DnDGrab<D, S, D::TouchFocus>
where
    D: DataDeviceHandler,
    D: SeatHandler,
    <D as SeatHandler>::TouchFocus: DndFocus<D> + 'static,
    D: 'static,
    S: Source,
{
    fn down(
        &mut self,
        _data: &mut D,
        _handle: &mut crate::input::touch::TouchInnerHandle<'_, D>,
        _focus: Option<(<D as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        _event: &crate::input::touch::DownEvent,
        _seq: crate::utils::Serial,
    ) {
        // Ignore
    }

    fn up(
        &mut self,
        data: &mut D,
        handle: &mut crate::input::touch::TouchInnerHandle<'_, D>,
        event: &crate::input::touch::UpEvent,
        _seq: crate::utils::Serial,
    ) {
        if event.slot != self.start_data().slot {
            return;
        }

        handle.unset_grab(self, data);
    }

    fn motion(
        &mut self,
        data: &mut D,
        _handle: &mut crate::input::touch::TouchInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &crate::input::touch::MotionEvent,
        _seq: crate::utils::Serial,
    ) {
        if event.slot != self.start_data().slot {
            return;
        }

        self.update_focus(
            data,
            focus,
            event.location,
            SERIAL_COUNTER.next_serial(),
            event.time,
        );
    }

    fn frame(
        &mut self,
        _data: &mut D,
        _handle: &mut crate::input::touch::TouchInnerHandle<'_, D>,
        _seq: crate::utils::Serial,
    ) {
    }

    fn cancel(
        &mut self,
        data: &mut D,
        handle: &mut crate::input::touch::TouchInnerHandle<'_, D>,
        _seq: crate::utils::Serial,
    ) {
        // TODO: should we cancel something here?
        handle.unset_grab(self, data);
    }

    fn shape(
        &mut self,
        _data: &mut D,
        _handle: &mut crate::input::touch::TouchInnerHandle<'_, D>,
        _event: &crate::input::touch::ShapeEvent,
        _seq: Serial,
    ) {
    }

    fn orientation(
        &mut self,
        _data: &mut D,
        _handle: &mut crate::input::touch::TouchInnerHandle<'_, D>,
        _event: &crate::input::touch::OrientationEvent,
        _seq: Serial,
    ) {
    }

    fn start_data(&self) -> &TouchGrabStartData<D> {
        self.touch_start_data.as_ref().unwrap()
    }

    fn unset(&mut self, data: &mut D) {
        self.drop(data);
    }
}
