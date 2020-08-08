use std::{cell::RefCell, ops::Deref as _, os::unix::io::RawFd, rc::Rc};

use wayland_server::{
    protocol::{wl_data_device_manager::DndAction, wl_data_offer, wl_pointer, wl_surface},
    Main,
};

use crate::wayland::seat::{AxisFrame, GrabStartData, PointerGrab, PointerInnerHandle, Seat};
use crate::wayland::Serial;

use super::{DataDeviceData, SeatData};

/// Event generated by the interactions of clients with a server initiated drag'n'drop
pub enum ServerDndEvent {
    /// The client chose an action
    Action(DndAction),
    /// The DnD resource was dropped by the user
    ///
    /// After that, the client can still interract with your ressource
    Dropped,
    /// The Dnd was cancelled
    ///
    /// The client can no longer interact
    Cancelled,
    /// The client requested for data to be sent
    Send {
        /// The requested mime type
        mime_type: String,
        /// The FD to write into
        fd: RawFd,
    },
    /// The client has finished interacting with the resource
    ///
    /// This can only happen after the resource was dropped.
    Finished,
}

pub(crate) struct ServerDnDGrab<C: 'static> {
    start_data: GrabStartData,
    metadata: super::SourceMetadata,
    current_focus: Option<wl_surface::WlSurface>,
    pending_offers: Vec<wl_data_offer::WlDataOffer>,
    offer_data: Option<Rc<RefCell<OfferData>>>,
    seat: Seat,
    callback: Rc<RefCell<C>>,
}

impl<C: 'static> ServerDnDGrab<C> {
    pub(crate) fn new(
        start_data: GrabStartData,
        metadata: super::SourceMetadata,
        seat: Seat,
        callback: Rc<RefCell<C>>,
    ) -> ServerDnDGrab<C> {
        ServerDnDGrab {
            start_data,
            metadata,
            current_focus: None,
            pending_offers: Vec::with_capacity(1),
            offer_data: None,
            seat,
            callback,
        }
    }
}

impl<C> PointerGrab for ServerDnDGrab<C>
where
    C: FnMut(ServerDndEvent) + 'static,
{
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle<'_>,
        location: (f64, f64),
        focus: Option<(wl_surface::WlSurface, (f64, f64))>,
        serial: Serial,
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
        if let Some((surface, (sx, sy))) = focus {
            // early return if the surface is no longer valid
            let client = match surface.as_ref().client() {
                Some(c) => c,
                None => return,
            };
            if self.current_focus.is_none() {
                // We entered a new surface, send the data offer
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
                                self.metadata.clone(),
                                offer_data.clone(),
                                self.callback.clone(),
                                action_choice,
                            )
                        })
                        .unwrap();
                    // advertize the offer to the client
                    device.data_offer(&offer);
                    for mime_type in self.metadata.mime_types.iter().cloned() {
                        offer.offer(mime_type);
                    }
                    offer.source_actions(self.metadata.dnd_action.to_raw());
                    device.enter(serial.into(), &surface, x - sx, y - sy, Some(&offer));
                    self.pending_offers.push(offer);
                }
                self.offer_data = Some(offer_data);
                self.current_focus = Some(surface);
            } else {
                // make a move
                for device in &seat_data.known_devices {
                    if device.as_ref().same_client_as(&surface.as_ref()) {
                        device.motion(time, x - sx, y - sy);
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
            if let Some(ref offer_data) = self.offer_data {
                let mut data = offer_data.borrow_mut();
                if validated {
                    data.dropped = true;
                } else {
                    data.active = false;
                }
            }
            let mut callback = self.callback.borrow_mut();
            (&mut *callback)(ServerDndEvent::Dropped);
            if !validated {
                (&mut *callback)(ServerDndEvent::Cancelled);
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

fn implement_dnd_data_offer<C>(
    offer: Main<wl_data_offer::WlDataOffer>,
    metadata: super::SourceMetadata,
    offer_data: Rc<RefCell<OfferData>>,
    callback: Rc<RefCell<C>>,
    action_choice: Rc<RefCell<dyn FnMut(DndAction, DndAction) -> DndAction + 'static>>,
) -> wl_data_offer::WlDataOffer
where
    C: FnMut(ServerDndEvent) + 'static,
{
    use self::wl_data_offer::Request;
    offer.quick_assign(move |offer, req, _| {
        let mut data = offer_data.borrow_mut();
        match req {
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
                    (&mut *callback.borrow_mut())(ServerDndEvent::Send { mime_type, fd });
                }
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
                (&mut *callback.borrow_mut())(ServerDndEvent::Finished);
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
                let possible_actions = metadata.dnd_action & DndAction::from_bits_truncate(dnd_actions);
                data.chosen_action = (&mut *action_choice.borrow_mut())(possible_actions, preferred_action);
                // check that the user provided callback respects that one precise action should be chosen
                debug_assert!(
                    [DndAction::Move, DndAction::Copy, DndAction::Ask].contains(&data.chosen_action)
                );
                offer.action(data.chosen_action.to_raw());
                (&mut *callback.borrow_mut())(ServerDndEvent::Action(data.chosen_action));
            }
            _ => unreachable!(),
        }
    });

    offer.deref().clone()
}
