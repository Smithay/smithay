use std::sync::Arc;

use slog::debug;
use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, ObjectData, ObjectId},
    protocol::{
        wl_data_device::WlDataDevice,
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::WlDataSource,
    },
    Client, DisplayHandle, Resource,
};

use super::{with_source_metadata, DataDeviceHandler, SourceMetadata};

pub enum Selection {
    Empty,
    Client(WlDataSource),
    Compositor(SourceMetadata),
}

pub struct SeatData {
    known_devices: Vec<WlDataDevice>,
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

    pub fn set_selection<D>(&mut self, dh: &mut DisplayHandle<'_>, new_selection: Selection)
    where
        D: DataDeviceHandler,
        D: 'static,
    {
        self.selection = new_selection;
        self.send_selection::<D>(dh);
    }

    pub fn set_focus<D>(&mut self, dh: &mut DisplayHandle<'_>, new_focus: Option<Client>)
    where
        D: DataDeviceHandler,
        D: 'static,
    {
        self.current_focus = new_focus;
        self.send_selection::<D>(dh);
    }

    pub fn send_selection<D>(&mut self, dh: &mut DisplayHandle<'_>)
    where
        D: DataDeviceHandler,
        D: 'static,
    {
        let client = match self.current_focus.as_ref() {
            Some(c) => c,
            None => return,
        };
        // TODO:
        // first sanitize the selection, reseting it to null if the client holding
        // it dropped it
        // let cleanup = if let Selection::Client(ref _data_source) = self.selection {
        //     false
        //     // !data_source.as_ref().is_alive()
        // } else {
        //     false
        // };
        // if cleanup {
        //     self.selection = Selection::Empty;
        // }

        // then send it if appropriate
        match self.selection {
            Selection::Empty => {
                // send an empty selection
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(dd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }
                    dd.selection(dh, None);
                }
            }
            Selection::Client(ref data_source) => {
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(dd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }
                    let source = data_source.clone();

                    let handle = dh.backend_handle::<D>().unwrap();
                    // create a data offer
                    let offer = handle
                        .create_object(
                            client.id(),
                            WlDataOffer::interface(),
                            dd.version(),
                            Arc::new(ClientSelection { source }),
                        )
                        .unwrap();
                    let offer = WlDataOffer::from_id(dh, offer).unwrap();

                    // advertize the offer to the client
                    dd.data_offer(dh, &offer);
                    with_source_metadata(data_source, |meta| {
                        for mime_type in meta.mime_types.iter().cloned() {
                            offer.offer(dh, mime_type);
                        }
                    })
                    .unwrap();
                    dd.selection(dh, Some(&offer));
                }
            }
            Selection::Compositor(ref meta) => {
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dh.get_client(dd.id()).map(|c| &c != client).unwrap_or(true) {
                        continue;
                    }

                    let offer_meta = meta.clone();

                    let handle = dh.backend_handle::<D>().unwrap();
                    // create a data offer
                    let offer = handle
                        .create_object(
                            client.id(),
                            WlDataOffer::interface(),
                            dd.version(),
                            Arc::new(ServerSelection { offer_meta }),
                        )
                        .unwrap();
                    let offer = WlDataOffer::from_id(dh, offer).unwrap();

                    // advertize the offer to the client
                    dd.data_offer(dh, &offer);
                    for mime_type in meta.mime_types.iter().cloned() {
                        offer.offer(dh, mime_type);
                    }
                    dd.selection(dh, Some(&offer));
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
        dh: &mut Handle<D>,
        handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let mut dh = DisplayHandle::from(dh);

        if let Ok((_resource, request)) = WlDataOffer::parse_request(&mut dh, msg) {
            handle_client_selection(handler, request, &self.source, &mut dh);
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

fn handle_client_selection<D>(
    state: &mut D,
    request: wl_data_offer::Request,
    source: &WlDataSource,
    dh: &mut wayland_server::DisplayHandle<'_>,
) where
    D: DataDeviceHandler,
{
    let data_device_state = state.data_device_state();

    // selection data offers only care about the `receive` event
    if let wl_data_offer::Request::Receive { fd, mime_type } = request {
        // check if the source and associated mime type is still valid
        let valid =
            with_source_metadata(source, |meta| meta.mime_types.contains(&mime_type)).unwrap_or(false);
        // TODO:?
        // && source.as_ref().is_alive();
        if !valid {
            // deny the receive
            debug!(
                data_device_state.log,
                "Denying a wl_data_offer.receive with invalid source."
            );
        } else {
            source.send(dh, mime_type, fd);
        }
        let _ = ::nix::unistd::close(fd);
    }
}

struct ServerSelection {
    offer_meta: SourceMetadata,
}

impl<D> ObjectData<D> for ServerSelection
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

        if let Ok((_resource, request)) = WlDataOffer::parse_request(&mut dh, msg) {
            handle_server_selection(handler, request, &self.offer_meta);
        }

        None
    }

    fn destroyed(&self, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

pub fn handle_server_selection<D>(
    handler: &mut D,
    request: wl_data_offer::Request,
    offer_meta: &SourceMetadata,
) where
    D: DataDeviceHandler,
{
    let data_device_state = handler.data_device_state();

    // selection data offers only care about the `receive` event
    if let wl_data_offer::Request::Receive { fd, mime_type } = request {
        // check if the associated mime type is valid
        if !offer_meta.mime_types.contains(&mime_type) {
            // deny the receive
            debug!(
                data_device_state.log,
                "Denying a wl_data_offer.receive with invalid source."
            );
            let _ = ::nix::unistd::close(fd);
        } else {
            handler.send_selection(mime_type, fd);
        }
    }
}
