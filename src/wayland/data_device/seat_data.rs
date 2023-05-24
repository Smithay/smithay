use std::{
    os::unix::io::{AsRawFd, OwnedFd},
    sync::Arc,
};

use tracing::debug;
use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, ObjectData, ObjectId},
    protocol::{
        wl_data_device::WlDataDevice,
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::WlDataSource,
        wl_seat::WlSeat,
    },
    Client, DisplayHandle, Resource,
};

use crate::{
    input::{Seat, SeatHandler},
    utils::IsAlive,
};

use super::{with_source_metadata, DataDeviceHandler, DataDeviceUserData, SourceMetadata};

pub enum Selection<U: Clone + Send + Sync + 'static> {
    Empty,
    Client(WlDataSource),
    Compositor { metadata: SourceMetadata, user_data: U },
}

pub struct SeatData<U: Clone + Send + Sync + 'static> {
    known_devices: Vec<WlDataDevice>,
    selection: Selection<U>,
    current_focus: Option<Client>,
}

impl<U: Clone + Send + Sync + 'static> Default for SeatData<U> {
    fn default() -> Self {
        Self {
            known_devices: Vec::new(),
            selection: Selection::Empty,
            current_focus: None,
        }
    }
}

impl<U: Clone + Send + Sync + 'static> SeatData<U> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_devices(&self) -> &[WlDataDevice] {
        &self.known_devices
    }

    pub fn add_device(&mut self, device: WlDataDevice) {
        self.known_devices.push(device);
    }

    pub fn retain_devices<F>(&mut self, f: F)
    where
        F: FnMut(&WlDataDevice) -> bool,
    {
        self.known_devices.retain(f)
    }

    pub fn set_selection<D>(&mut self, dh: &DisplayHandle, new_selection: Selection<U>)
    where
        D: DataDeviceHandler<SelectionUserData = U>,
        D: 'static,
    {
        if let Selection::Client(data_source) = &self.selection {
            match &new_selection {
                Selection::Client(new_data_source) if new_data_source == data_source => {}
                _ => {
                    data_source.cancelled();
                }
            }
        }
        self.selection = new_selection;
        self.send_selection::<D>(dh);
    }

    pub fn get_selection(&self) -> &Selection<U> {
        &self.selection
    }

    pub fn set_focus<D>(&mut self, dh: &DisplayHandle, new_focus: Option<Client>)
    where
        D: DataDeviceHandler<SelectionUserData = U>,
        D: 'static,
    {
        self.current_focus = new_focus;
        self.send_selection::<D>(dh);
    }

    pub fn send_selection<D>(&mut self, dh: &DisplayHandle)
    where
        D: DataDeviceHandler<SelectionUserData = U>,
        D: 'static,
    {
        let client = match self.current_focus.as_ref() {
            Some(c) => c,
            None => return,
        };
        // first sanitize the selection, reseting it to null if the client holding
        // it dropped it
        let cleanup = if let Selection::Client(ref data_source) = self.selection {
            !data_source.alive()
        } else {
            false
        };
        if cleanup {
            self.selection = Selection::Empty;
        }

        // then send it if appropriate
        match self.selection {
            Selection::Empty => {
                // send an empty selection
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(dd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }
                    dd.selection(None);
                }
            }
            Selection::Client(ref data_source) => {
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(dd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }
                    let source = data_source.clone();

                    let handle = dh.backend_handle();
                    // create a data offer
                    let offer = handle
                        .create_object::<D>(
                            client.id(),
                            WlDataOffer::interface(),
                            dd.version(),
                            Arc::new(ClientSelection { source }),
                        )
                        .unwrap();
                    let offer = WlDataOffer::from_id(dh, offer).unwrap();

                    // advertize the offer to the client
                    dd.data_offer(&offer);
                    with_source_metadata(data_source, |meta| {
                        for mime_type in meta.mime_types.iter().cloned() {
                            offer.offer(mime_type);
                        }
                    })
                    .unwrap();
                    dd.selection(Some(&offer));
                }
            }
            Selection::Compositor {
                metadata: ref meta,
                ref user_data,
            } => {
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(dd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }

                    let offer_meta = meta.clone();

                    let handle = dh.backend_handle();
                    // create a data offer
                    let wl_seat = match dd.data::<DataDeviceUserData>() {
                        Some(data) => data.wl_seat.clone(),
                        None => continue,
                    };
                    let offer = handle
                        .create_object::<D>(
                            client.id(),
                            WlDataOffer::interface(),
                            dd.version(),
                            Arc::new(ServerSelection {
                                offer_meta,
                                wl_seat,
                                user_data: user_data.clone(),
                            }),
                        )
                        .unwrap();
                    let offer = WlDataOffer::from_id(dh, offer).unwrap();

                    // advertize the offer to the client
                    dd.data_offer(&offer);
                    for mime_type in meta.mime_types.iter().cloned() {
                        offer.offer(mime_type);
                    }
                    dd.selection(Some(&offer));
                }
            }
        }
    }
}

struct ClientSelection {
    source: WlDataSource,
}

impl<D> ObjectData<D> for ClientSelection
where
    D: DataDeviceHandler,
{
    fn request(
        self: Arc<Self>,
        dh: &Handle,
        _handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let dh = DisplayHandle::from(dh.clone());
        if let Ok((_resource, request)) = WlDataOffer::parse_request(&dh, msg) {
            handle_client_selection(request, &self.source);
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

fn handle_client_selection(request: wl_data_offer::Request, source: &WlDataSource) {
    // selection data offers only care about the `receive` event
    if let wl_data_offer::Request::Receive { fd, mime_type } = request {
        // check if the source and associated mime type is still valid
        let valid =
            with_source_metadata(source, |meta| meta.mime_types.contains(&mime_type)).unwrap_or(false);
        // TODO:?
        // && source.as_ref().is_alive();
        if !valid {
            // deny the receive
            debug!("Denying a wl_data_offer.receive with invalid source.");
        } else {
            source.send(mime_type, fd.as_raw_fd());
        }
    }
}

struct ServerSelection<U: Clone + Send + Sync + 'static> {
    offer_meta: SourceMetadata,
    wl_seat: WlSeat,
    user_data: U,
}

impl<D> ObjectData<D> for ServerSelection<D::SelectionUserData>
where
    D: DataDeviceHandler + SeatHandler + 'static,
{
    fn request(
        self: Arc<Self>,
        dh: &Handle,
        handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let dh = DisplayHandle::from(dh.clone());
        if let Ok((_resource, request)) = WlDataOffer::parse_request(&dh, msg) {
            if !self.wl_seat.is_alive() {
                return None;
            }
            if let Some(seat) = Seat::<D>::from_resource(&self.wl_seat) {
                handle_server_selection(handler, request, seat, &self.offer_meta, &self.user_data);
            }
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

pub fn handle_server_selection<D>(
    handler: &mut D,
    request: wl_data_offer::Request,
    seat: Seat<D>,
    offer_meta: &SourceMetadata,
    user_data: &D::SelectionUserData,
) where
    D: DataDeviceHandler + SeatHandler + 'static,
{
    // selection data offers only care about the `receive` event
    if let wl_data_offer::Request::Receive { fd, mime_type } = request {
        // check if the associated mime type is valid
        if !offer_meta.mime_types.contains(&mime_type) {
            // deny the receive
            debug!("Denying a wl_data_offer.receive with invalid source.");
        } else {
            handler.send_selection(mime_type, fd, seat, user_data);
        }
    }
}
