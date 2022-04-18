use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, ObjectData, ObjectId},
    protocol::{
        wl_data_device_manager::DndAction,
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_surface,
    },
    DisplayHandle, Resource,
};

use crate::wayland::seat::{
    AxisFrame, ButtonEvent, MotionEvent, PointerGrab, PointerGrabStartData, PointerInnerHandle, Seat,
};

use super::{seat_data::SeatData, with_source_metadata, ClientDndGrabHandler, DataDeviceHandler};

pub(crate) struct DnDGrab<D> {
    start_data: PointerGrabStartData,
    data_source: Option<wl_data_source::WlDataSource>,
    current_focus: Option<wl_surface::WlSurface>,
    pending_offers: Vec<wl_data_offer::WlDataOffer>,
    offer_data: Option<Arc<Mutex<OfferData>>>,
    icon: Option<wl_surface::WlSurface>,
    origin: wl_surface::WlSurface,
    seat: Seat<D>,
}

impl<D> DnDGrab<D> {
    pub(crate) fn new(
        start_data: PointerGrabStartData,
        source: Option<wl_data_source::WlDataSource>,
        origin: wl_surface::WlSurface,
        seat: Seat<D>,
        icon: Option<wl_surface::WlSurface>,
    ) -> Self {
        Self {
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
    D: 'static,
{
    fn motion(
        &mut self,
        _data: &mut D,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(dh, event.location, None, event.serial, event.time);

        let seat_data = self
            .seat
            .user_data()
            .get::<RefCell<SeatData>>()
            .unwrap()
            .borrow_mut();
        if event.focus.as_ref().map(|&(ref s, _)| s) != self.current_focus.as_ref() {
            // focus changed, we need to make a leave if appropriate
            if let Some(surface) = self.current_focus.take() {
                // only leave if there is a data source or we are on the original client
                if self.data_source.is_some() || self.origin.id().same_client_as(&surface.id()) {
                    for device in seat_data.known_devices() {
                        if device.id().same_client_as(&surface.id()) {
                            device.leave(dh);
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
        if let Some((ref surface, surface_location)) = event.focus {
            // early return if the surface is no longer valid
            let client = match dh.get_client(surface.id()) {
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
                        .known_devices()
                        .iter()
                        .filter(|d| d.id().same_client_as(&surface.id()))
                    {
                        let handle = dh.backend_handle::<D>().unwrap();

                        // create a data offer
                        let offer = handle
                            .create_object(
                                client.id(),
                                WlDataOffer::interface(),
                                device.version(),
                                Arc::new(DndDataOffer {
                                    offer_data: offer_data.clone(),
                                    source: source.clone(),
                                }),
                            )
                            .unwrap();
                        let offer = WlDataOffer::from_id(dh, offer).unwrap();

                        // advertize the offer to the client
                        device.data_offer(dh, &offer);
                        with_source_metadata(source, |meta| {
                            for mime_type in meta.mime_types.iter().cloned() {
                                offer.offer(dh, mime_type);
                            }
                            offer.source_actions(dh, meta.dnd_action);
                        })
                        .unwrap();
                        device.enter(dh, event.serial.into(), surface, x, y, Some(&offer));
                        self.pending_offers.push(offer);
                    }
                    self.offer_data = Some(offer_data);
                } else {
                    // only send if we are on a surface of the same client
                    if self.origin.id().same_client_as(&surface.id()) {
                        for device in seat_data.known_devices() {
                            if device.id().same_client_as(&surface.id()) {
                                device.enter(dh, event.serial.into(), surface, x, y, None);
                            }
                        }
                    }
                }
                self.current_focus = Some(surface.clone());
            } else {
                // make a move
                if self.data_source.is_some() || self.origin.id().same_client_as(&surface.id()) {
                    for device in seat_data.known_devices() {
                        if device.id().same_client_as(&surface.id()) {
                            device.motion(dh, event.time, x, y);
                        }
                    }
                }
            }
        }
    }

    fn button(
        &mut self,
        handler: &mut D,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &ButtonEvent,
    ) {
        if handle.current_pressed().is_empty() {
            // the user dropped, proceed to the drop
            let seat_data = self
                .seat
                .user_data()
                .get::<RefCell<SeatData>>()
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
                    for device in seat_data.known_devices() {
                        if device.id().same_client_as(&surface.id()) {
                            if validated {
                                device.drop(dh);
                            } else {
                                device.leave(dh);
                            }
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
                source.dnd_drop_performed(dh);
                if !validated {
                    source.cancelled(dh);
                }
            }

            ClientDndGrabHandler::dropped(handler, self.seat.clone());
            self.icon = None;
            // in all cases abandon the drop
            // no more buttons are pressed, release the grab
            handle.unset_grab(dh, event.serial, event.time);
        }
    }

    fn axis(
        &mut self,
        _data: &mut D,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, D>,
        details: AxisFrame,
    ) {
        // we just forward the axis events as is
        handle.axis(dh, details);
    }

    fn start_data(&self) -> &PointerGrabStartData {
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
        dh: &mut Handle<D>,
        handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let mut dh = DisplayHandle::from(dh);

        if let Ok((resource, request)) = WlDataOffer::parse_request(&mut dh, msg) {
            handle_dnd(handler, &resource, request, &self, &mut dh);
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

fn handle_dnd<D>(
    handler: &mut D,
    offer: &WlDataOffer,
    request: wl_data_offer::Request,
    data: &DndDataOffer,
    dh: &mut wayland_server::DisplayHandle<'_>,
) where
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
                    // TODO:
                    // && source.as_ref().is_alive() 
                    && data.active;
            if valid {
                source.send(dh, mime_type, fd);
            }
            let _ = ::nix::unistd::close(fd);
        }
        Request::Destroy => {}
        Request::Finish => {
            if !data.active {
                offer.post_error(
                    dh,
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer that is no longer active.",
                );
                return;
            }
            if !data.accepted {
                offer.post_error(
                    dh,
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer that has not been accepted.",
                );
                return;
            }
            if !data.dropped {
                offer.post_error(
                    dh,
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer that has not been dropped.",
                );
                return;
            }
            if data.chosen_action.is_empty() {
                offer.post_error(
                    dh,
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer with no valid action.",
                );
                return;
            }
            source.dnd_finished(dh);
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
                offer.post_error(
                    dh,
                    wl_data_offer::Error::InvalidAction,
                    "Invalid preferred action.",
                );
                return;
            }

            let source_actions =
                with_source_metadata(source, |meta| meta.dnd_action).unwrap_or_else(|_| DndAction::empty());
            let possible_actions = source_actions & dnd_actions;
            data.chosen_action = handler.action_choice(possible_actions, preferred_action);
            // check that the user provided callback respects that one precise action should be chosen
            debug_assert!(
                [DndAction::None, DndAction::Move, DndAction::Copy, DndAction::Ask]
                    .contains(&data.chosen_action),
                "Only one precise action should be chosen"
            );
            offer.action(dh, data.chosen_action);
            source.action(dh, data.chosen_action);
        }
        _ => unreachable!(),
    }
}
