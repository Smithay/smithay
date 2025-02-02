use std::os::unix::io::OwnedFd;
use std::sync::Arc;

use tracing::debug;
use wayland_protocols::ext::data_control::v1::server::ext_data_control_offer_v1::{
    self, ExtDataControlOfferV1,
};
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_offer_v1::{
    self, ZwpPrimarySelectionOfferV1 as PrimaryOffer,
};
use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_offer_v1::{
    self, ZwlrDataControlOfferV1,
};
use wayland_server::backend::protocol::Message;
use wayland_server::backend::ObjectId;
use wayland_server::backend::{ClientId, Handle, ObjectData};
use wayland_server::protocol::wl_data_offer;
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::DisplayHandle;
use wayland_server::{protocol::wl_data_offer::WlDataOffer, Resource};
use wl_data_offer::Request as DataOfferRequest;
use zwlr_data_control_offer_v1::Request as DataControlRequest;
use zwp_primary_selection_offer_v1::Request as PrimaryRequest;

use crate::input::Seat;

use super::device::{DataDeviceKind, SelectionDevice};
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
pub(crate) enum SelectionOffer {
    DataDevice(WlDataOffer),
    Primary(PrimaryOffer),
    WlrDataControl(ZwlrDataControlOfferV1),
    ExtDataControl(ExtDataControlOfferV1),
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

        let device_kind = device.device_kind();
        let data = Arc::new(OfferReplyData {
            device_kind,
            seat: device.seat(),
            source: data,
        });
        let backend = dh.backend_handle();

        let interface = match device_kind {
            DataDeviceKind::Core => WlDataOffer::interface(),
            DataDeviceKind::Primary => PrimaryOffer::interface(),
            DataDeviceKind::WlrDataControl => ZwlrDataControlOfferV1::interface(),
            DataDeviceKind::ExtDataControl => ExtDataControlOfferV1::interface(),
        };

        let offer = backend
            .create_object::<D>(client_id, interface, device.version(), data)
            .unwrap();

        match device_kind {
            DataDeviceKind::Core => Self::DataDevice(WlDataOffer::from_id(dh, offer).unwrap()),
            DataDeviceKind::Primary => Self::Primary(PrimaryOffer::from_id(dh, offer).unwrap()),
            DataDeviceKind::WlrDataControl => {
                Self::WlrDataControl(ZwlrDataControlOfferV1::from_id(dh, offer).unwrap())
            }
            DataDeviceKind::ExtDataControl => {
                Self::ExtDataControl(ExtDataControlOfferV1::from_id(dh, offer).unwrap())
            }
        }
    }

    pub fn offer(&self, mime_type: String) {
        selection_dispatch!(self; Self(offer) => offer.offer(mime_type))
    }
}

struct OfferReplyData<U: Clone + Send + Sync + 'static> {
    device_kind: DataDeviceKind,
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

        // NOTE: we can't parse message more than once, since it expects the `OwnedFd` which
        // we can't clone. To achieve that, we use RTTI passed along the selection data, to
        // make the parsing work only once.
        let (mime_type, fd, object_name) = match self.device_kind {
            DataDeviceKind::Core => {
                if let Ok((_resource, DataOfferRequest::Receive { mime_type, fd })) =
                    WlDataOffer::parse_request(&dh, msg)
                {
                    (mime_type, fd, "wl_data_offer")
                } else {
                    return None;
                }
            }
            DataDeviceKind::Primary => {
                if let Ok((_resource, PrimaryRequest::Receive { mime_type, fd })) =
                    PrimaryOffer::parse_request(&dh, msg)
                {
                    (mime_type, fd, "primary_selection_offer")
                } else {
                    return None;
                }
            }
            DataDeviceKind::WlrDataControl => {
                if let Ok((_resource, DataControlRequest::Receive { mime_type, fd })) =
                    ZwlrDataControlOfferV1::parse_request(&dh, msg)
                {
                    (mime_type, fd, "wlr_data_control_offer")
                } else {
                    return None;
                }
            }
            DataDeviceKind::ExtDataControl => {
                if let Ok((_resource, ext_data_control_offer_v1::Request::Receive { mime_type, fd })) =
                    ExtDataControlOfferV1::parse_request(&dh, msg)
                {
                    (mime_type, fd, "ext_data_control_offer")
                } else {
                    return None;
                }
            }
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
