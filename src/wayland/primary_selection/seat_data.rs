use std::{
    os::unix::io::{AsRawFd, OwnedFd},
    sync::Arc,
};

use tracing::debug;
use wayland_protocols::wp::primary_selection::zv1::server::{
    zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1 as PrimaryDevice,
    zwp_primary_selection_offer_v1::{self as primary_offer, ZwpPrimarySelectionOfferV1 as PrimaryOffer},
    zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1 as PrimarySource,
};
use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, ObjectData, ObjectId},
    Client, DisplayHandle, Resource,
};

use crate::utils::IsAlive;

use super::{with_source_metadata, PrimarySelectionHandler, SourceMetadata};

pub enum Selection {
    Empty,
    Client(PrimarySource),
    Compositor(SourceMetadata),
}

pub struct SeatData {
    known_devices: Vec<PrimaryDevice>,
    selection: Selection,
    current_focus: Option<Client>,
}

impl Default for SeatData {
    fn default() -> Self {
        Self {
            known_devices: Vec::new(),
            selection: Selection::Empty,
            current_focus: None,
        }
    }
}

impl SeatData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_device(&mut self, device: PrimaryDevice) {
        self.known_devices.push(device);
    }

    pub fn retain_devices<F>(&mut self, f: F)
    where
        F: FnMut(&PrimaryDevice) -> bool,
    {
        self.known_devices.retain(f)
    }

    pub fn set_focus<D>(&mut self, dh: &DisplayHandle, new_focus: Option<Client>)
    where
        D: PrimarySelectionHandler,
        D: 'static,
    {
        self.current_focus = new_focus;
        self.send_selection::<D>(dh);
    }

    pub fn set_selection<D>(&mut self, dh: &DisplayHandle, new_selection: Selection)
    where
        D: PrimarySelectionHandler,
        D: 'static,
    {
        if let Selection::Client(source) = &self.selection {
            match &new_selection {
                Selection::Client(new_source) if new_source == source => {}
                _ => {
                    source.cancelled();
                }
            }
        }
        self.selection = new_selection;
        self.send_selection::<D>(dh);
    }

    pub fn send_selection<D>(&mut self, dh: &DisplayHandle)
    where
        D: PrimarySelectionHandler,
        D: 'static,
    {
        let client = match self.current_focus.as_ref() {
            Some(c) => c,
            None => return,
        };
        // first sanitize the selection, reseting it to null if the client holding
        // it dropped it
        let cleanup = if let Selection::Client(ref source) = self.selection {
            !source.alive()
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
                for pd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(pd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }
                    pd.selection(None);
                }
            }
            Selection::Client(ref source) => {
                for pd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(pd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }
                    let source_clone = source.clone();

                    let handle = dh.backend_handle();
                    // create a data offer
                    let offer = handle
                        .create_object::<D>(
                            client.id(),
                            PrimaryOffer::interface(),
                            pd.version(),
                            Arc::new(ClientSelection { source: source_clone }),
                        )
                        .unwrap();
                    let offer = PrimaryOffer::from_id(dh, offer).unwrap();

                    // advertize the offer to the client
                    pd.data_offer(&offer);
                    with_source_metadata(source, |meta| {
                        for mime_type in meta.mime_types.iter().cloned() {
                            offer.offer(mime_type);
                        }
                    })
                    .unwrap();
                    pd.selection(Some(&offer));
                }
            }
            Selection::Compositor(ref meta) => {
                for pd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(pd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }

                    let offer_meta = meta.clone();

                    let handle = dh.backend_handle();
                    // create a data offer
                    let offer = handle
                        .create_object::<D>(
                            client.id(),
                            PrimaryOffer::interface(),
                            pd.version(),
                            Arc::new(ServerSelection { offer_meta }),
                        )
                        .unwrap();
                    let offer = PrimaryOffer::from_id(dh, offer).unwrap();

                    // advertize the offer to the client
                    pd.data_offer(&offer);
                    for mime_type in meta.mime_types.iter().cloned() {
                        offer.offer(mime_type);
                    }
                    pd.selection(Some(&offer));
                }
            }
        }
    }
}

struct ClientSelection {
    source: PrimarySource,
}

impl<D> ObjectData<D> for ClientSelection
where
    D: PrimarySelectionHandler,
{
    fn request(
        self: Arc<Self>,
        dh: &Handle,
        _handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let dh = DisplayHandle::from(dh.clone());
        if let Ok((_resource, request)) = PrimaryOffer::parse_request(&dh, msg) {
            handle_client_selection(request, &self.source);
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

fn handle_client_selection(request: primary_offer::Request, source: &PrimarySource) {
    // selection data offers only care about the `receive` event
    if let primary_offer::Request::Receive { fd, mime_type } = request {
        // check if the source and associated mime type is still valid
        let valid =
            with_source_metadata(source, |meta| meta.mime_types.contains(&mime_type)).unwrap_or(false);
        // TODO:?
        // && source.as_ref().is_alive();
        if !valid {
            // deny the receive
            debug!("Denying a zwp_primary_selection_offer_v1.receive with invalid source.");
        } else {
            source.send(mime_type, fd.as_raw_fd());
        }
    }
}

struct ServerSelection {
    offer_meta: SourceMetadata,
}

impl<D> ObjectData<D> for ServerSelection
where
    D: PrimarySelectionHandler,
{
    fn request(
        self: Arc<Self>,
        dh: &Handle,
        handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let dh = DisplayHandle::from(dh.clone());
        if let Ok((_resource, request)) = PrimaryOffer::parse_request(&dh, msg) {
            handle_server_selection(handler, request, &self.offer_meta);
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

pub fn handle_server_selection<D>(
    handler: &mut D,
    request: primary_offer::Request,
    offer_meta: &SourceMetadata,
) where
    D: PrimarySelectionHandler,
{
    // selection data offers only care about the `receive` event
    if let primary_offer::Request::Receive { fd, mime_type } = request {
        // check if the associated mime type is valid
        if !offer_meta.mime_types.contains(&mime_type) {
            // deny the receive
            debug!("Denying a zwp_primary_selection_offer_v1.receive with invalid source.");
        } else {
            handler.send_selection(mime_type, fd);
        }
    }
}
