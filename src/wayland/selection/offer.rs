use std::any::TypeId;
use std::os::unix::io::OwnedFd;
use std::sync::Arc;

use tracing::debug;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1 as PrimaryDevice;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_offer_v1::{
    self, ZwpPrimarySelectionOfferV1 as PrimaryOffer,
};
use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_offer_v1::{
    self, ZwlrDataControlOfferV1 as DataControlOffer,
};
use wayland_server::backend::protocol::Message;
use wayland_server::backend::ObjectId;
use wayland_server::backend::{ClientId, Handle, ObjectData};
use wayland_server::protocol::wl_data_device::WlDataDevice;
use wayland_server::protocol::wl_data_offer;
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::DisplayHandle;
use wayland_server::{protocol::wl_data_offer::WlDataOffer, Resource};
use wl_data_offer::Request as DataOfferRequest;
use zwlr_data_control_offer_v1::Request as DataControlRequest;
use zwp_primary_selection_offer_v1::Request as PrimaryRequest;

use crate::input::Seat;

use super::device::SelectionDevice;
use super::private::selection_dispatch;
use super::source::{CompositorSelectionProvider, SelectionSourceProvider};
use super::SelectionHandler;

#[derive(Debug, Clone)]
pub enum OfferReplySource<U: Clone + Send + Sync + 'static> {
    /// The selection is backend by client source.
    Client(SelectionSourceProvider),
    /// The selection is backed by the compositor.
    Compositor(CompositorSelectionProvider<U>),
}

impl<U: Clone + Send + Sync + 'static> OfferReplySource<U> {
    /// Get the copy of the underlying mime types.
    pub fn mime_types(&self) -> Vec<String> {
        match self {
            OfferReplySource::Client(source) => source.mime_types(),
            OfferReplySource::Compositor(source) => source.mime_types.clone(),
        }
    }

    /// Check whether the source contains the given `mime_type`.
    pub fn contains_mime_type(&self, mime_type: &String) -> bool {
        match self {
            OfferReplySource::Client(source) => source.contains_mime_type(mime_type),
            OfferReplySource::Compositor(source) => source.mime_types.contains(mime_type),
        }
    }
}

/// Offer representing various selection offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionOffer {
    DataDevice(WlDataOffer),
    Primary(PrimaryOffer),
    DataControl(DataControlOffer),
}

impl SelectionOffer {
    pub fn new<D>(
        dh: &DisplayHandle,
        device: &SelectionDevice,
        client_id: ClientId,
        data: OfferReplySource<D::SelectionUserData>,
    ) -> Self
    where
        D: SelectionHandler + 'static,
    {
        // NOTE: the types are tied to the `SelectionDevice`, so every
        // RTTI like checking is safe and reliable.

        let type_id = device.inner_type_id();
        let data = Arc::new(OfferReplyData {
            type_id,
            seat: device.seat(),
            source: data,
        });
        let backend = dh.backend_handle();

        let interface = if type_id == TypeId::of::<WlDataDevice>() {
            WlDataOffer::interface()
        } else if type_id == TypeId::of::<PrimaryDevice>() {
            PrimaryOffer::interface()
        } else {
            DataControlOffer::interface()
        };

        let offer = backend
            .create_object::<D>(client_id, interface, device.version(), data)
            .unwrap();

        if type_id == TypeId::of::<WlDataDevice>() {
            Self::DataDevice(WlDataOffer::from_id(dh, offer).unwrap())
        } else if type_id == TypeId::of::<PrimaryDevice>() {
            Self::Primary(PrimaryOffer::from_id(dh, offer).unwrap())
        } else {
            Self::DataControl(DataControlOffer::from_id(dh, offer).unwrap())
        }
    }

    pub fn offer(&self, mime_type: String) {
        selection_dispatch!(self; Self(offer) => offer.offer(mime_type))
    }
}

struct OfferReplyData<U: Clone + Send + Sync + 'static> {
    type_id: TypeId,
    source: OfferReplySource<U>,
    seat: WlSeat,
}

impl<D> ObjectData<D> for OfferReplyData<D::SelectionUserData>
where
    D: SelectionHandler + 'static,
{
    fn request(
        self: Arc<Self>,
        dh: &Handle,
        handle: &mut D,
        _: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let dh = DisplayHandle::from(dh.clone());
        let type_id = self.type_id;

        // NOTE: we can't parse message more than once, since it expects the `OwnedFd` which
        // we can't clone. To achieve that, we use RTTI passed along the selection data, to
        // make the parsing work only once.
        let (mime_type, fd, object_name) = if type_id == TypeId::of::<WlDataDevice>() {
            if let Ok((_resource, DataOfferRequest::Receive { mime_type, fd })) =
                WlDataOffer::parse_request(&dh, msg)
            {
                (mime_type, fd, "wl_data_offer")
            } else {
                return None;
            }
        } else if type_id == TypeId::of::<PrimaryDevice>() {
            if let Ok((_resource, PrimaryRequest::Receive { mime_type, fd })) =
                PrimaryOffer::parse_request(&dh, msg)
            {
                (mime_type, fd, "primary_selection_offer")
            } else {
                return None;
            }
        } else if let Ok((_resource, DataControlRequest::Receive { mime_type, fd })) =
            DataControlOffer::parse_request(&dh, msg)
        {
            (mime_type, fd, "data_control_offer")
        } else {
            return None;
        };

        if !self.source.contains_mime_type(&mime_type) {
            debug!("Denying a {object_name}.receive with invalid mime type");
        } else {
            match &self.source {
                OfferReplySource::Client(source) => {
                    source.send(mime_type, fd);
                }
                OfferReplySource::Compositor(source) => {
                    if let Some(seat) = Seat::<D>::from_resource(&self.seat) {
                        handle.send_selection(source.ty, mime_type, fd, seat, &source.user_data);
                    }
                }
            }
        }

        None
    }

    fn destroyed(self: Arc<Self>, _: &Handle, _: &mut D, _: ClientId, _: ObjectId) {}
}
