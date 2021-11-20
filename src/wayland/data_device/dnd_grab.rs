use std::{cell::RefCell, ops::Deref as _, rc::Rc};

use wayland_server::{
    protocol::{wl_data_device_manager::DndAction, wl_data_offer, wl_data_source, wl_pointer, wl_surface},
    DispatchData, Main,
};

use crate::{
    utils::{Logical, Point},
    wayland::{
        seat::{AxisFrame, GrabStartData, PointerGrab, PointerInnerHandle, Seat},
        Serial,
    },
};

use super::{with_source_metadata, DataDeviceData, SeatData};

pub(crate) struct DnDGrab {
    start_data: GrabStartData,
    data_source: Option<wl_data_source::WlDataSource>,
    current_focus: Option<wl_surface::WlSurface>,
    pending_offers: Vec<wl_data_offer::WlDataOffer>,
    offer_data: Option<Rc<RefCell<OfferData>>>,
    icon: Option<wl_surface::WlSurface>,
    origin: wl_surface::WlSurface,
    callback: Rc<RefCell<dyn FnMut(super::DataDeviceEvent)>>,
    seat: Seat,
}

impl DnDGrab {
    pub(crate) fn new(
        start_data: GrabStartData,
        source: Option<wl_data_source::WlDataSource>,
        origin: wl_surface::WlSurface,
        seat: Seat,
        icon: Option<wl_surface::WlSurface>,
        callback: Rc<RefCell<dyn FnMut(super::DataDeviceEvent)>>,
    ) -> DnDGrab {
        DnDGrab {
            start_data,
            data_source: source,
            current_focus: None,
            pending_offers: Vec::with_capacity(1),
            offer_data: None,
            origin,
            icon,
            callback,
            seat,
        }
    }
}

impl PointerGrab for DnDGrab {
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        focus: Option<(wl_surface::WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
        _ddata: DispatchData<'_>,
    ) {
        let seat_data = self
            .seat
            .user_data()
            .get::<RefCell<SeatData>>()
            .unwrap()
            .borrow_mut();
        if focus.as_ref().map(|&(ref s, _)| s) != self.current_focus.as_ref() {
            // focus changed, we need to make a leave if appropriate
            if let Some(surface) = self.current_focus.take() {
                // only leave if there is a data source or we are on the original client
                if self.data_source.is_some() || self.origin.as_ref().same_client_as(surface.as_ref()) {
                    for device in &seat_data.known_devices {
                        if device.as_ref().same_client_as(surface.as_ref()) {
                            device.leave();
                        }
                    }
                    // disable the offers
                    self.pending_offers.clear();
                    if let Some(offer_data) = self.offer_data.take() {
                        offer_data.borrow_mut().active = false;
                    }
                }
            }
        }
        if let Some((surface, surface_location)) = focus {
            // early return if the surface is no longer valid
            let client = match surface.as_ref().client() {
                Some(c) => c,
                None => return,
            };
            let (x, y) = (location - surface_location.to_f64()).into();
            if self.current_focus.is_none() {
                // We entered a new surface, send the data offer if appropriate
                if let Some(ref source) = self.data_source {
                    let offer_data = Rc::new(RefCell::new(OfferData {
                        active: true,
                        dropped: false,
                        accepted: true,
                        chosen_action: DndAction::empty(),
                    }));
                    for device in seat_data
                        .known_devices
                        .iter()
                        .filter(|d| d.as_ref().same_client_as(surface.as_ref()))
                    {
                        let action_choice = device
                            .as_ref()
                            .user_data()
                            .get::<DataDeviceData>()
                            .unwrap()
                            .action_choice
                            .clone();
                        // create a data offer
                        let offer = client
                            .create_resource::<wl_data_offer::WlDataOffer>(device.as_ref().version())
                            .map(|offer| {
                                implement_dnd_data_offer(
                                    offer,
                                    source.clone(),
                                    offer_data.clone(),
                                    action_choice,
                                )
                            })
                            .unwrap();
                        // advertize the offer to the client
                        device.data_offer(&offer);
                        with_source_metadata(source, |meta| {
                            for mime_type in meta.mime_types.iter().cloned() {
                                offer.offer(mime_type);
                            }
                            offer.source_actions(meta.dnd_action);
                        })
                        .unwrap();
                        device.enter(serial.into(), &surface, x, y, Some(&offer));
                        self.pending_offers.push(offer);
                    }
                    self.offer_data = Some(offer_data);
                } else {
                    // only send if we are on a surface of the same client
                    if self.origin.as_ref().same_client_as(surface.as_ref()) {
                        for device in &seat_data.known_devices {
                            if device.as_ref().same_client_as(surface.as_ref()) {
                                device.enter(serial.into(), &surface, x, y, None);
                            }
                        }
                    }
                }
                self.current_focus = Some(surface);
            } else {
                // make a move
                if self.data_source.is_some() || self.origin.as_ref().same_client_as(surface.as_ref()) {
                    for device in &seat_data.known_devices {
                        if device.as_ref().same_client_as(surface.as_ref()) {
                            device.motion(time, x, y);
                        }
                    }
                }
            }
        }
    }

    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        _button: u32,
        _state: wl_pointer::ButtonState,
        serial: Serial,
        time: u32,
        _ddata: DispatchData<'_>,
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
                let data = data.borrow();
                data.accepted && (!data.chosen_action.is_empty())
            } else {
                false
            };
            if let Some(ref surface) = self.current_focus {
                if self.data_source.is_some() || self.origin.as_ref().same_client_as(surface.as_ref()) {
                    for device in &seat_data.known_devices {
                        if device.as_ref().same_client_as(surface.as_ref()) {
                            if validated {
                                device.drop();
                            } else {
                                device.leave();
                            }
                        }
                    }
                }
            }
            if let Some(ref offer_data) = self.offer_data {
                let mut data = offer_data.borrow_mut();
                if validated {
                    data.dropped = true;
                } else {
                    data.active = false;
                }
            }
            if let Some(ref source) = self.data_source {
                source.dnd_drop_performed();
                if !validated {
                    source.cancelled();
                }
            }
            (&mut *self.callback.borrow_mut())(super::DataDeviceEvent::DnDDropped);
            self.icon = None;
            // in all cases abandon the drop
            // no more buttons are pressed, release the grab
            handle.unset_grab(serial, time);
        }
    }

    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame, _ddata: DispatchData<'_>) {
        // we just forward the axis events as is
        handle.axis(details);
    }

    fn start_data(&self) -> &GrabStartData {
        &self.start_data
    }
}

struct OfferData {
    active: bool,
    dropped: bool,
    accepted: bool,
    chosen_action: DndAction,
}

fn implement_dnd_data_offer(
    offer: Main<wl_data_offer::WlDataOffer>,
    source: wl_data_source::WlDataSource,
    offer_data: Rc<RefCell<OfferData>>,
    action_choice: Rc<RefCell<dyn FnMut(DndAction, DndAction) -> DndAction + 'static>>,
) -> wl_data_offer::WlDataOffer {
    use self::wl_data_offer::Request;
    offer.quick_assign(move |offer, req, _| {
        let mut data = offer_data.borrow_mut();
        match req {
            Request::Accept { mime_type, .. } => {
                if let Some(mtype) = mime_type {
                    if let Err(crate::utils::UnmanagedResource) = with_source_metadata(&source, |meta| {
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
                    && source.as_ref().is_alive()
                    && data.active;
                if valid {
                    source.send(mime_type, fd);
                }
                let _ = ::nix::unistd::close(fd);
            }
            Request::Destroy => {}
            Request::Finish => {
                if !data.active {
                    offer.as_ref().post_error(
                        wl_data_offer::Error::InvalidFinish as u32,
                        "Cannot finish a data offer that is no longer active.".into(),
                    );
                    return;
                }
                if !data.accepted {
                    offer.as_ref().post_error(
                        wl_data_offer::Error::InvalidFinish as u32,
                        "Cannot finish a data offer that has not been accepted.".into(),
                    );
                    return;
                }
                if !data.dropped {
                    offer.as_ref().post_error(
                        wl_data_offer::Error::InvalidFinish as u32,
                        "Cannot finish a data offer that has not been dropped.".into(),
                    );
                    return;
                }
                if data.chosen_action.is_empty() {
                    offer.as_ref().post_error(
                        wl_data_offer::Error::InvalidFinish as u32,
                        "Cannot finish a data offer with no valid action.".into(),
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
                let preferred_action = preferred_action;

                // preferred_action must only contain one bitflag at the same time
                if ![DndAction::None, DndAction::Move, DndAction::Copy, DndAction::Ask]
                    .contains(&preferred_action)
                {
                    offer.as_ref().post_error(
                        wl_data_offer::Error::InvalidAction as u32,
                        "Invalid preferred action.".into(),
                    );
                    return;
                }
                let source_actions = with_source_metadata(&source, |meta| meta.dnd_action)
                    .unwrap_or_else(|_| DndAction::empty());
                let possible_actions = source_actions & dnd_actions;
                data.chosen_action = (&mut *action_choice.borrow_mut())(possible_actions, preferred_action);
                // check that the user provided callback respects that one precise action should be chosen
                debug_assert!(
                    [DndAction::None, DndAction::Move, DndAction::Copy, DndAction::Ask]
                        .contains(&data.chosen_action)
                );
                offer.action(data.chosen_action);
                source.action(data.chosen_action);
            }
            _ => unreachable!(),
        }
    });

    offer.deref().clone()
}
