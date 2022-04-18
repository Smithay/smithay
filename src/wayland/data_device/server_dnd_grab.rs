use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, ObjectData, ObjectId},
    protocol::{
        wl_data_device_manager::DndAction,
        wl_data_offer::{self, WlDataOffer},
        wl_surface,
    },
    DisplayHandle, Resource,
};

use crate::wayland::seat::{
    AxisFrame, ButtonEvent, MotionEvent, PointerGrab, PointerGrabStartData, PointerInnerHandle, Seat,
};

use super::{DataDeviceHandler, SeatData, ServerDndGrabHandler, SourceMetadata};

pub(crate) struct ServerDnDGrab<D> {
    start_data: PointerGrabStartData,
    metadata: super::SourceMetadata,
    current_focus: Option<wl_surface::WlSurface>,
    pending_offers: Vec<wl_data_offer::WlDataOffer>,
    offer_data: Option<Arc<Mutex<ServerDndOfferData>>>,
    seat: Seat<D>,
}

impl<D> ServerDnDGrab<D> {
    pub(crate) fn new(
        start_data: PointerGrabStartData,
        metadata: super::SourceMetadata,
        seat: Seat<D>,
    ) -> Self {
        Self {
            start_data,
            metadata,
            current_focus: None,
            pending_offers: Vec::with_capacity(1),
            offer_data: None,
            seat,
        }
    }
}

impl<D> PointerGrab<D> for ServerDnDGrab<D>
where
    D: DataDeviceHandler,
    D: 'static,
{
    fn motion(
        &mut self,
        _data: &mut D,
        dh: &mut DisplayHandle<'_>,
        _handle: &mut PointerInnerHandle<'_, D>,
        event: &MotionEvent,
    ) {
        let focus = event.focus.clone();
        let location = event.location;
        let serial = event.serial;
        let time = event.time;

        let seat_data = self
            .seat
            .user_data()
            .get::<RefCell<SeatData>>()
            .unwrap()
            .borrow_mut();
        if focus.as_ref().map(|&(ref s, _)| s) != self.current_focus.as_ref() {
            // focus changed, we need to make a leave if appropriate
            if let Some(surface) = self.current_focus.take() {
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
        if let Some((surface, surface_location)) = focus {
            // early return if the surface is no longer valid
            let client = match dh.get_client(surface.id()) {
                Ok(c) => c,
                _ => return,
            };
            let (x, y) = (location - surface_location.to_f64()).into();
            if self.current_focus.is_none() {
                // We entered a new surface, send the data offer
                let offer_data = Arc::new(Mutex::new(ServerDndOfferData {
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
                            Arc::new(ServerDndData {
                                metadata: self.metadata.clone(),
                                ofer_data: offer_data.clone(),
                            }),
                        )
                        .unwrap();
                    let offer = WlDataOffer::from_id(dh, offer).unwrap();

                    // advertize the offer to the client
                    device.data_offer(dh, &offer);
                    for mime_type in self.metadata.mime_types.iter().cloned() {
                        offer.offer(dh, mime_type);
                    }
                    offer.source_actions(dh, self.metadata.dnd_action);
                    device.enter(dh, serial.into(), &surface, x, y, Some(&offer));
                    self.pending_offers.push(offer);
                }
                self.offer_data = Some(offer_data);
                self.current_focus = Some(surface);
            } else {
                // make a move
                for device in seat_data.known_devices() {
                    if device.id().same_client_as(&surface.id()) {
                        device.motion(dh, time, x, y);
                    }
                }
            }
        }
    }

    fn button(
        &mut self,
        data: &mut D,
        dh: &mut DisplayHandle<'_>,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &ButtonEvent,
    ) {
        let serial = event.serial;
        let time = event.time;

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
            if let Some(ref offer_data) = self.offer_data {
                let mut data = offer_data.lock().unwrap();
                if validated {
                    data.dropped = true;
                } else {
                    data.active = false;
                }
            }

            ServerDndGrabHandler::dropped(data);
            if !validated {
                data.cancelled();
            }
            // in all cases abandon the drop
            // no more buttons are pressed, release the grab
            handle.unset_grab(dh, serial, time);
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
struct ServerDndOfferData {
    active: bool,
    dropped: bool,
    accepted: bool,
    chosen_action: DndAction,
}

struct ServerDndData {
    metadata: SourceMetadata,
    ofer_data: Arc<Mutex<ServerDndOfferData>>,
}

impl<D> ObjectData<D> for ServerDndData
where
    D: DataDeviceHandler,
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
            handle_server_dnd(handler, &resource, request, &self, &mut dh);
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

fn handle_server_dnd<D>(
    handler: &mut D,
    offer: &WlDataOffer,
    request: wl_data_offer::Request,
    data: &ServerDndData,
    dh: &mut wayland_server::DisplayHandle<'_>,
) where
    D: DataDeviceHandler,
{
    use self::wl_data_offer::Request;

    let metadata = &data.metadata;
    let offer_data = &data.ofer_data;

    let mut data = offer_data.lock().unwrap();
    match request {
        Request::Accept { mime_type, .. } => {
            if let Some(mtype) = mime_type {
                data.accepted = metadata.mime_types.contains(&mtype);
            } else {
                data.accepted = false;
            }
        }
        Request::Receive { mime_type, fd } => {
            // check if the source and associated mime type is still valid
            if metadata.mime_types.contains(&mime_type) && data.active {
                handler.send(mime_type, fd);
            }
        }
        Request::Destroy => {}
        Request::Finish => {
            if !data.active {
                offer.post_error(
                    dh,
                    wl_data_offer::Error::InvalidFinish as u32,
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

            handler.finished();
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
            let possible_actions = metadata.dnd_action & dnd_actions;
            data.chosen_action = handler.action_choice(possible_actions, preferred_action);
            // check that the user provided callback respects that one precise action should be chosen
            debug_assert!(
                [DndAction::None, DndAction::Move, DndAction::Copy, DndAction::Ask]
                    .contains(&data.chosen_action),
                "Only one precise action should be chosen"
            );
            offer.action(dh, data.chosen_action);

            handler.action(data.chosen_action);
        }
        _ => unreachable!(),
    }
}
