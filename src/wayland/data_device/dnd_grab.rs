use std::sync::{Arc, Mutex};

use wayland_server::{
    protocol::{
        wl_data_device, wl_data_device_manager::DndAction, wl_data_offer, wl_data_source, wl_pointer,
        wl_surface,
    },
    NewResource, Resource,
};

use wayland::seat::{AxisFrame, PointerGrab, PointerInnerHandle, Seat};

use super::{with_source_metadata, SeatData};

pub(crate) struct DnDGrab<F: 'static> {
    data_source: Option<Resource<wl_data_source::WlDataSource>>,
    current_focus: Option<Resource<wl_surface::WlSurface>>,
    pending_offers: Vec<Resource<wl_data_offer::WlDataOffer>>,
    offer_data: Option<Arc<Mutex<OfferData>>>,
    origin: Resource<wl_surface::WlSurface>,
    seat: Seat,
    action_choice: Arc<Mutex<F>>,
}

impl<F: 'static> DnDGrab<F> {
    pub(crate) fn new(
        source: Option<Resource<wl_data_source::WlDataSource>>,
        origin: Resource<wl_surface::WlSurface>,
        seat: Seat,
        action_choice: Arc<Mutex<F>>,
    ) -> DnDGrab<F> {
        DnDGrab {
            data_source: source,
            current_focus: None,
            pending_offers: Vec::with_capacity(1),
            offer_data: None,
            origin,
            seat,
            action_choice,
        }
    }
}

impl<F> PointerGrab for DnDGrab<F>
where
    F: FnMut(DndAction, DndAction) -> DndAction + Send + 'static,
{
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle,
        location: (f64, f64),
        focus: Option<(Resource<wl_surface::WlSurface>, (f64, f64))>,
        serial: u32,
        time: u32,
    ) {
        let (x, y) = location;
        let seat_data = self
            .seat
            .user_data()
            .get::<Mutex<SeatData>>()
            .unwrap()
            .lock()
            .unwrap();
        if focus.as_ref().map(|&(ref s, _)| s) != self.current_focus.as_ref() {
            // focus changed, we need to make a leave if appropriate
            if let Some(surface) = self.current_focus.take() {
                // only leave if there is a data source or we are on the original client
                if self.data_source.is_some() || self.origin.same_client_as(&surface) {
                    for device in &seat_data.known_devices {
                        if device.same_client_as(&surface) {
                            device.send(wl_data_device::Event::Leave);
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
        if let Some((surface, (sx, sy))) = focus {
            // early return if the surface is no longer valid
            let client = match surface.client() {
                Some(c) => c,
                None => return,
            };
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
                        .known_devices
                        .iter()
                        .filter(|d| d.same_client_as(&surface))
                    {
                        // create a data offer
                        let offer = client
                            .create_resource::<wl_data_offer::WlDataOffer>(device.version())
                            .map(|offer| {
                                implement_dnd_data_offer(
                                    offer,
                                    source.clone(),
                                    offer_data.clone(),
                                    self.action_choice.clone(),
                                )
                            }).unwrap();
                        // advertize the offer to the client
                        device.send(wl_data_device::Event::DataOffer { id: offer.clone() });
                        with_source_metadata(source, |meta| {
                            for mime_type in meta.mime_types.iter().cloned() {
                                offer.send(wl_data_offer::Event::Offer { mime_type })
                            }
                            offer.send(wl_data_offer::Event::SourceActions {
                                source_actions: meta.dnd_action.to_raw(),
                            });
                        }).unwrap();
                        device.send(wl_data_device::Event::Enter {
                            serial,
                            x: x - sx,
                            y: y - sy,
                            surface: surface.clone(),
                            id: Some(offer.clone()),
                        });
                        self.pending_offers.push(offer);
                    }
                    self.offer_data = Some(offer_data);
                } else {
                    // only send if we are on a surface of the same client
                    if self.origin.same_client_as(&surface) {
                        for device in &seat_data.known_devices {
                            if device.same_client_as(&surface) {
                                device.send(wl_data_device::Event::Enter {
                                    serial,
                                    x: x - sx,
                                    y: y - sy,
                                    surface: surface.clone(),
                                    id: None,
                                });
                            }
                        }
                    }
                }
                self.current_focus = Some(surface);
            } else {
                // make a move
                if self.data_source.is_some() || self.origin.same_client_as(&surface) {
                    for device in &seat_data.known_devices {
                        if device.same_client_as(&surface) {
                            device.send(wl_data_device::Event::Motion {
                                time,
                                x: x - sx,
                                y: y - sy,
                            });
                        }
                    }
                }
            }
        }
    }

    fn button(
        &mut self,
        handle: &mut PointerInnerHandle,
        _button: u32,
        _state: wl_pointer::ButtonState,
        serial: u32,
        time: u32,
    ) {
        if handle.current_pressed().len() == 0 {
            // the user dropped, proceed to the drop
            let seat_data = self
                .seat
                .user_data()
                .get::<Mutex<SeatData>>()
                .unwrap()
                .lock()
                .unwrap();
            let validated = if let Some(ref data) = self.offer_data {
                let data = data.lock().unwrap();
                data.accepted && (!data.chosen_action.is_empty())
            } else {
                false
            };
            if let Some(ref surface) = self.current_focus {
                if self.data_source.is_some() || self.origin.same_client_as(&surface) {
                    for device in &seat_data.known_devices {
                        if device.same_client_as(surface) {
                            if validated {
                                device.send(wl_data_device::Event::Drop);
                            } else {
                                device.send(wl_data_device::Event::Leave);
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
                source.send(wl_data_source::Event::DndDropPerformed);
                if !validated {
                    source.send(wl_data_source::Event::Cancelled);
                }
            }
            // in all cases abandon the drop
            // no more buttons are pressed, release the grab
            handle.unset_grab(serial, time);
        }
    }

    fn axis(&mut self, handle: &mut PointerInnerHandle, details: AxisFrame) {
        // we just forward the axis events as is
        handle.axis(details);
    }
}

struct OfferData {
    active: bool,
    dropped: bool,
    accepted: bool,
    chosen_action: DndAction,
}

fn implement_dnd_data_offer<F>(
    offer: NewResource<wl_data_offer::WlDataOffer>,
    source: Resource<wl_data_source::WlDataSource>,
    offer_data: Arc<Mutex<OfferData>>,
    action_choice: Arc<Mutex<F>>,
) -> Resource<wl_data_offer::WlDataOffer>
where
    F: FnMut(DndAction, DndAction) -> DndAction + Send + 'static,
{
    use self::wl_data_offer::Request;
    offer.implement(
        move |req, offer| {
            let mut data = offer_data.lock().unwrap();
            match req {
                Request::Accept { serial: _, mime_type } => {
                    if let Some(mtype) = mime_type {
                        if let Err(()) = with_source_metadata(&source, |meta| {
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
                    let valid = with_source_metadata(&source, |meta| meta.mime_types.contains(&mime_type))
                        .unwrap_or(false)
                        && source.is_alive()
                        && data.active;
                    if valid {
                        source.send(wl_data_source::Event::Send { mime_type, fd });
                    }
                    let _ = ::nix::unistd::close(fd);
                }
                Request::Destroy => {}
                Request::Finish => {
                    if !data.active {
                        offer.post_error(
                            wl_data_offer::Error::InvalidFinish as u32,
                            "Cannot finish a data offer that is no longer active.".into(),
                        );
                    }
                    if !data.accepted {
                        offer.post_error(
                            wl_data_offer::Error::InvalidFinish as u32,
                            "Cannot finish a data offer that has not been accepted.".into(),
                        );
                    }
                    if !data.dropped {
                        offer.post_error(
                            wl_data_offer::Error::InvalidFinish as u32,
                            "Cannot finish a data offer that has not been dropped.".into(),
                        );
                    }
                    if data.chosen_action.is_empty() {
                        offer.post_error(
                            wl_data_offer::Error::InvalidFinish as u32,
                            "Cannot finish a data offer with no valid action.".into(),
                        );
                    }
                    source.send(wl_data_source::Event::DndFinished);
                    data.active = false;
                }
                Request::SetActions {
                    dnd_actions,
                    preferred_action,
                } => {
                    let preferred_action = DndAction::from_bits_truncate(preferred_action);
                    if ![DndAction::Move, DndAction::Copy, DndAction::Ask].contains(&preferred_action) {
                        offer.post_error(
                            wl_data_offer::Error::InvalidAction as u32,
                            "Invalid preferred action.".into(),
                        );
                    }
                    let source_actions =
                        with_source_metadata(&source, |meta| meta.dnd_action).unwrap_or(DndAction::empty());
                    let possible_actions = source_actions & DndAction::from_bits_truncate(dnd_actions);
                    data.chosen_action =
                        (&mut *action_choice.lock().unwrap())(possible_actions, preferred_action);
                    // check that the user provided callback respects that one precise action should be chosen
                    debug_assert!(
                        [DndAction::Move, DndAction::Copy, DndAction::Ask].contains(&data.chosen_action)
                    );
                    offer.send(wl_data_offer::Event::Action {
                        dnd_action: data.chosen_action.to_raw(),
                    });
                    source.send(wl_data_source::Event::Action {
                        dnd_action: data.chosen_action.to_raw(),
                    });
                }
            }
        },
        None::<fn(_)>,
        (),
    )
}
