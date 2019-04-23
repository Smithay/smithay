use std::cell::RefCell;
use std::rc::Rc;

use wayland_server::{
    protocol::{wl_data_device_manager::DndAction, wl_data_offer, wl_data_source, wl_pointer, wl_surface},
    NewResource,
};

use crate::wayland::{
    compositor::{roles::Role, CompositorToken},
    seat::{AxisFrame, PointerGrab, PointerInnerHandle, Seat},
};

use super::{with_source_metadata, DataDeviceData, DnDIconRole, SeatData};

pub(crate) struct DnDGrab<R> {
    data_source: Option<wl_data_source::WlDataSource>,
    current_focus: Option<wl_surface::WlSurface>,
    pending_offers: Vec<wl_data_offer::WlDataOffer>,
    offer_data: Option<Rc<RefCell<OfferData>>>,
    icon: Option<wl_surface::WlSurface>,
    origin: wl_surface::WlSurface,
    callback: Rc<RefCell<dyn FnMut(super::DataDeviceEvent)>>,
    token: CompositorToken<R>,
    seat: Seat,
}

impl<R: Role<DnDIconRole> + 'static> DnDGrab<R> {
    pub(crate) fn new(
        source: Option<wl_data_source::WlDataSource>,
        origin: wl_surface::WlSurface,
        seat: Seat,
        icon: Option<wl_surface::WlSurface>,
        token: CompositorToken<R>,
        callback: Rc<RefCell<dyn FnMut(super::DataDeviceEvent)>>,
    ) -> DnDGrab<R> {
        DnDGrab {
            data_source: source,
            current_focus: None,
            pending_offers: Vec::with_capacity(1),
            offer_data: None,
            origin,
            icon,
            callback,
            token,
            seat,
        }
    }
}

impl<R: Role<DnDIconRole> + 'static> PointerGrab for DnDGrab<R> {
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle<'_>,
        location: (f64, f64),
        focus: Option<(wl_surface::WlSurface, (f64, f64))>,
        serial: u32,
        time: u32,
    ) {
        let (x, y) = location;
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
                if self.data_source.is_some() || self.origin.as_ref().same_client_as(&surface.as_ref()) {
                    for device in &seat_data.known_devices {
                        if device.as_ref().same_client_as(&surface.as_ref()) {
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
        if let Some((surface, (sx, sy))) = focus {
            // early return if the surface is no longer valid
            let client = match surface.as_ref().client() {
                Some(c) => c,
                None => return,
            };
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
                        .filter(|d| d.as_ref().same_client_as(&surface.as_ref()))
                    {
                        let action_choice = device
                            .as_ref()
                            .user_data::<DataDeviceData>()
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
                            offer.source_actions(meta.dnd_action.to_raw());
                        })
                        .unwrap();
                        device.enter(serial, &surface, x - sx, y - sy, Some(&offer));
                        self.pending_offers.push(offer);
                    }
                    self.offer_data = Some(offer_data);
                } else {
                    // only send if we are on a surface of the same client
                    if self.origin.as_ref().same_client_as(&surface.as_ref()) {
                        for device in &seat_data.known_devices {
                            if device.as_ref().same_client_as(&surface.as_ref()) {
                                device.enter(serial, &surface, x - sx, y - sy, None);
                            }
                        }
                    }
                }
                self.current_focus = Some(surface);
            } else {
                // make a move
                if self.data_source.is_some() || self.origin.as_ref().same_client_as(&surface.as_ref()) {
                    for device in &seat_data.known_devices {
                        if device.as_ref().same_client_as(&surface.as_ref()) {
                            device.motion(time, x - sx, y - sy);
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
        serial: u32,
        time: u32,
    ) {
        if handle.current_pressed().len() == 0 {
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
                if self.data_source.is_some() || self.origin.as_ref().same_client_as(&surface.as_ref()) {
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
            if let Some(icon) = self.icon.take() {
                if icon.as_ref().is_alive() {
                    self.token.remove_role::<super::DnDIconRole>(&icon).unwrap();
                }
            }
            // in all cases abandon the drop
            // no more buttons are pressed, release the grab
            handle.unset_grab(serial, time);
        }
    }

    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame) {
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

fn implement_dnd_data_offer(
    offer: NewResource<wl_data_offer::WlDataOffer>,
    source: wl_data_source::WlDataSource,
    offer_data: Rc<RefCell<OfferData>>,
    action_choice: Rc<RefCell<dyn FnMut(DndAction, DndAction) -> DndAction + 'static>>,
) -> wl_data_offer::WlDataOffer {
    use self::wl_data_offer::Request;
    offer.implement_closure(
        move |req, offer| {
            let mut data = offer_data.borrow_mut();
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
                    }
                    if !data.accepted {
                        offer.as_ref().post_error(
                            wl_data_offer::Error::InvalidFinish as u32,
                            "Cannot finish a data offer that has not been accepted.".into(),
                        );
                    }
                    if !data.dropped {
                        offer.as_ref().post_error(
                            wl_data_offer::Error::InvalidFinish as u32,
                            "Cannot finish a data offer that has not been dropped.".into(),
                        );
                    }
                    if data.chosen_action.is_empty() {
                        offer.as_ref().post_error(
                            wl_data_offer::Error::InvalidFinish as u32,
                            "Cannot finish a data offer with no valid action.".into(),
                        );
                    }
                    source.dnd_finished();
                    data.active = false;
                }
                Request::SetActions {
                    dnd_actions,
                    preferred_action,
                } => {
                    let preferred_action = DndAction::from_bits_truncate(preferred_action);
                    if ![DndAction::Move, DndAction::Copy, DndAction::Ask].contains(&preferred_action) {
                        offer.as_ref().post_error(
                            wl_data_offer::Error::InvalidAction as u32,
                            "Invalid preferred action.".into(),
                        );
                    }
                    let source_actions =
                        with_source_metadata(&source, |meta| meta.dnd_action).unwrap_or(DndAction::empty());
                    let possible_actions = source_actions & DndAction::from_bits_truncate(dnd_actions);
                    data.chosen_action =
                        (&mut *action_choice.borrow_mut())(possible_actions, preferred_action);
                    // check that the user provided callback respects that one precise action should be chosen
                    debug_assert!(
                        [DndAction::Move, DndAction::Copy, DndAction::Ask].contains(&data.chosen_action)
                    );
                    offer.action(data.chosen_action.to_raw());
                    source.action(data.chosen_action.to_raw());
                }
                _ => unreachable!(),
            }
        },
        None::<fn(_)>,
        (),
    )
}
