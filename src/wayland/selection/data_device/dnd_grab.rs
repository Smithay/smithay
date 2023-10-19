use std::{
    cell::RefCell,
    os::unix::io::{AsFd, OwnedFd},
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, ObjectData, ObjectId},
    protocol::{
        wl_data_device_manager::DndAction,
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_surface::WlSurface,
    },
    DisplayHandle, Resource,
};

use crate::{
    input::{
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
            PointerInnerHandle, RelativeMotionEvent,
        },
        Seat, SeatHandler,
    },
    utils::{IsAlive, Logical, Point},
    wayland::{seat::WaylandFocus, selection::seat_data::SeatData},
};

use super::{with_source_metadata, ClientDndGrabHandler, DataDeviceHandler};

pub(crate) struct DnDGrab<D: SeatHandler> {
    dh: DisplayHandle,
    start_data: PointerGrabStartData<D>,
    data_source: Option<wl_data_source::WlDataSource>,
    current_focus: Option<WlSurface>,
    pending_offers: Vec<wl_data_offer::WlDataOffer>,
    offer_data: Option<Arc<Mutex<OfferData>>>,
    icon: Option<WlSurface>,
    origin: WlSurface,
    seat: Seat<D>,
}

impl<D: SeatHandler> DnDGrab<D> {
    pub(crate) fn new(
        dh: &DisplayHandle,
        start_data: PointerGrabStartData<D>,
        source: Option<wl_data_source::WlDataSource>,
        origin: WlSurface,
        seat: Seat<D>,
        icon: Option<WlSurface>,
    ) -> Self {
        Self {
            dh: dh.clone(),
            start_data,
            data_source: source,
            current_focus: None,
            pending_offers: Vec::with_capacity(1),
            offer_data: None,
            origin,
            icon,
            seat,
        }
    }
}

impl<D> PointerGrab<D> for DnDGrab<D>
where
    D: DataDeviceHandler,
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: WaylandFocus,
    D: 'static,
{
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        let seat_data = self
            .seat
            .user_data()
            .get::<RefCell<SeatData<D::SelectionUserData>>>()
            .unwrap()
            .borrow_mut();
        if focus.as_ref().and_then(|(s, _)| s.wl_surface()) != self.current_focus.clone() {
            // focus changed, we need to make a leave if appropriate
            if let Some(surface) = self.current_focus.take() {
                // only leave if there is a data source or we are on the original client
                if self.data_source.is_some() || self.origin.id().same_client_as(&surface.id()) {
                    for device in seat_data.known_data_devices() {
                        if device.id().same_client_as(&surface.id()) {
                            device.leave();
                        }
                    }
                    // disable the offers
                    self.pending_offers.clear();
                    if let Some(offer_data) = self.offer_data.take() {
                        offer_data.lock().unwrap().active = false;
                    }
                }
            }
        }
        if let Some((surface, surface_location)) = focus
            .as_ref()
            .and_then(|(h, loc)| h.wl_surface().map(|s| (s, loc)))
        {
            // early return if the surface is no longer valid
            let client = match self.dh.get_client(surface.id()) {
                Ok(c) => c,
                Err(_) => return,
            };
            let (x, y) = (event.location - surface_location.to_f64()).into();
            if self.current_focus.is_none() {
                // We entered a new surface, send the data offer if appropriate
                if let Some(ref source) = self.data_source {
                    let offer_data = Arc::new(Mutex::new(OfferData {
                        active: true,
                        dropped: false,
                        accepted: true,
                        chosen_action: DndAction::empty(),
                    }));
                    for device in seat_data
                        .known_data_devices()
                        .filter(|d| d.id().same_client_as(&surface.id()))
                    {
                        let handle = self.dh.backend_handle();

                        // create a data offer
                        let offer = handle
                            .create_object::<D>(
                                client.id(),
                                WlDataOffer::interface(),
                                device.version(),
                                Arc::new(DndDataOffer {
                                    offer_data: offer_data.clone(),
                                    source: source.clone(),
                                }),
                            )
                            .unwrap();
                        let offer = WlDataOffer::from_id(&self.dh, offer).unwrap();

                        // advertize the offer to the client
                        device.data_offer(&offer);
                        with_source_metadata(source, |meta| {
                            for mime_type in meta.mime_types.iter().cloned() {
                                offer.offer(mime_type);
                            }
                            offer.source_actions(meta.dnd_action);
                        })
                        .unwrap();
                        device.enter(event.serial.into(), &surface, x, y, Some(&offer));
                        self.pending_offers.push(offer);
                    }
                    self.offer_data = Some(offer_data);
                } else {
                    // only send if we are on a surface of the same client
                    if self.origin.id().same_client_as(&surface.id()) {
                        for device in seat_data.known_data_devices() {
                            if device.id().same_client_as(&surface.id()) {
                                device.enter(event.serial.into(), &surface, x, y, None);
                            }
                        }
                    }
                }
                self.current_focus = Some(surface);
            } else {
                // make a move
                if self.data_source.is_some() || self.origin.id().same_client_as(&surface.id()) {
                    for device in seat_data.known_data_devices() {
                        if device.id().same_client_as(&surface.id()) {
                            device.motion(event.time, x, y);
                        }
                    }
                }
            }
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<i32, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, event: &ButtonEvent) {
        if handle.current_pressed().is_empty() {
            // the user dropped, proceed to the drop
            let seat_data = self
                .seat
                .user_data()
                .get::<RefCell<SeatData<D::SelectionUserData>>>()
                .unwrap()
                .borrow_mut();
            let validated = if let Some(ref data) = self.offer_data {
                let data = data.lock().unwrap();
                data.accepted && (!data.chosen_action.is_empty())
            } else {
                false
            };
            if let Some(ref surface) = self.current_focus {
                if self.data_source.is_some() || self.origin.id().same_client_as(&surface.id()) {
                    for device in seat_data.known_data_devices() {
                        if device.id().same_client_as(&surface.id()) && validated {
                            device.drop();
                        }
                    }
                }
            }
            if let Some(ref offer_data) = self.offer_data {
                let mut data = offer_data.lock().unwrap();
                if validated {
                    data.dropped = true;
                } else {
                    data.active = false;
                }
            }
            if let Some(ref source) = self.data_source {
                if source.version() >= 3 {
                    source.dnd_drop_performed();
                }
                if !validated {
                    source.cancelled();
                }
            }

            ClientDndGrabHandler::dropped(data, self.seat.clone());
            self.icon = None;
            // in all cases abandon the drop
            // no more buttons are pressed, release the grab
            if let Some(ref surface) = self.current_focus {
                for device in seat_data.known_data_devices() {
                    if device.id().same_client_as(&surface.id()) {
                        device.leave();
                    }
                }
            }
            handle.unset_grab(data, event.serial, event.time, true);
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
        &self.start_data
    }
}

#[derive(Debug)]
struct OfferData {
    active: bool,
    dropped: bool,
    accepted: bool,
    chosen_action: DndAction,
}

#[derive(Debug)]
struct DndDataOffer {
    offer_data: Arc<Mutex<OfferData>>,
    source: WlDataSource,
}

impl<D> ObjectData<D> for DndDataOffer
where
    D: DataDeviceHandler,
    D: 'static,
{
    fn request(
        self: Arc<Self>,
        dh: &Handle,
        handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let dh = DisplayHandle::from(dh.clone());
        if let Ok((resource, request)) = WlDataOffer::parse_request(&dh, msg) {
            handle_dnd(handler, &resource, request, &self);
        }

        None
    }

    fn destroyed(
        self: Arc<Self>,
        _handle: &Handle,
        _data: &mut D,
        _client_id: ClientId,
        _object_id: ObjectId,
    ) {
    }
}

fn handle_dnd<D>(handler: &mut D, offer: &WlDataOffer, request: wl_data_offer::Request, data: &DndDataOffer)
where
    D: DataDeviceHandler,
    D: 'static,
{
    use self::wl_data_offer::Request;
    let source = &data.source;
    let mut data = data.offer_data.lock().unwrap();
    match request {
        Request::Accept { mime_type, .. } => {
            if let Some(mtype) = mime_type {
                if let Err(crate::utils::UnmanagedResource) = with_source_metadata(source, |meta| {
                    data.accepted = meta.mime_types.contains(&mtype);
                }) {
                    data.accepted = false;
                }
            } else {
                data.accepted = false;
            }
        }
        Request::Receive { mime_type, fd } => {
            // check if the source and associated mime type is still valid
            let valid = with_source_metadata(source, |meta| meta.mime_types.contains(&mime_type))
                .unwrap_or(false)
                && source.alive()
                && data.active;
            if valid {
                source.send(mime_type, fd.as_fd());
            }
        }
        Request::Destroy => {}
        Request::Finish => {
            if !data.active {
                offer.post_error(
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer that is no longer active.",
                );
                return;
            }
            if !data.accepted {
                offer.post_error(
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer that has not been accepted.",
                );
                return;
            }
            if !data.dropped {
                offer.post_error(
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer that has not been dropped.",
                );
                return;
            }
            if data.chosen_action.is_empty() {
                offer.post_error(
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer with no valid action.",
                );
                return;
            }
            source.dnd_finished();
            data.active = false;
        }
        Request::SetActions {
            dnd_actions,
            preferred_action,
        } => {
            let dnd_actions = dnd_actions.into_result().unwrap_or(DndAction::None);
            let preferred_action = preferred_action.into_result().unwrap_or(DndAction::None);

            // preferred_action must only contain one bitflag at the same time
            if ![DndAction::None, DndAction::Move, DndAction::Copy, DndAction::Ask]
                .contains(&preferred_action)
            {
                offer.post_error(wl_data_offer::Error::InvalidAction, "Invalid preferred action.");
                return;
            }

            let source_actions =
                with_source_metadata(source, |meta| meta.dnd_action).unwrap_or_else(|_| DndAction::empty());
            let possible_actions = source_actions & dnd_actions;
            let chosen_action = handler.action_choice(possible_actions, preferred_action);
            // check that the user provided callback respects that one precise action should be chosen
            debug_assert!(
                [DndAction::None, DndAction::Move, DndAction::Copy, DndAction::Ask].contains(&chosen_action),
                "Only one precise action should be chosen"
            );
            if chosen_action != data.chosen_action {
                data.chosen_action = chosen_action;
                offer.action(chosen_action);
                source.action(chosen_action);
            }
        }
        _ => unreachable!(),
    }
}
