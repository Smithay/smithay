use std::any::Any;
use std::any::TypeId;

use wayland_protocols::ext::data_control::v1::server::ext_data_control_device_v1::ExtDataControlDeviceV1;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1 as PrimaryDevice;
use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
use wayland_server::backend::ObjectId;
use wayland_server::protocol::wl_data_device::WlDataDevice;
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::Resource;

use super::data_device::DataDeviceUserData;
use super::ext_data_control::ExtDataControlDeviceUserData;
use super::offer::DataControlOffer;
use super::offer::SelectionOffer;
use super::primary_selection::PrimaryDeviceUserData;
use super::private::selection_dispatch;
use super::wlr_data_control::DataControlDeviceUserData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DataControlDevice {
    Wlr(ZwlrDataControlDeviceV1),
    Ext(ExtDataControlDeviceV1),
}

impl DataControlDevice {
    fn data_offer(&self, offer: &DataControlOffer) {
        match (self, offer) {
            (Self::Wlr(obj), DataControlOffer::Wlr(offer)) => obj.data_offer(offer),
            (Self::Ext(obj), DataControlOffer::Ext(offer)) => obj.data_offer(offer),
            _ => unreachable!(),
        }
    }

    fn selection(&self, offer: Option<&DataControlOffer>) {
        match (self, offer) {
            (Self::Wlr(obj), Some(DataControlOffer::Wlr(offer))) => obj.selection(Some(offer)),
            (Self::Ext(obj), Some(DataControlOffer::Ext(offer))) => obj.selection(Some(offer)),
            (Self::Wlr(obj), None) => obj.selection(None),
            (Self::Ext(obj), None) => obj.selection(None),
            _ => unreachable!(),
        }
    }

    fn primary_selection(&self, offer: Option<&DataControlOffer>) {
        match (self, offer) {
            (Self::Wlr(obj), Some(DataControlOffer::Wlr(offer))) => obj.primary_selection(Some(offer)),
            (Self::Ext(obj), Some(DataControlOffer::Ext(offer))) => obj.primary_selection(Some(offer)),
            (Self::Wlr(obj), None) => obj.primary_selection(None),
            (Self::Ext(obj), None) => obj.primary_selection(None),
            _ => unreachable!(),
        }
    }

    fn wl_seat(&self) -> WlSeat {
        match self {
            Self::Wlr(device) => {
                let data: &DataControlDeviceUserData = device.data().unwrap();
                data.wl_seat.clone()
            }
            Self::Ext(device) => {
                let data: &ExtDataControlDeviceUserData = device.data().unwrap();
                data.wl_seat.clone()
            }
        }
    }

    fn id(&self) -> ObjectId {
        match self {
            Self::Wlr(obj) => obj.id(),
            Self::Ext(obj) => obj.id(),
        }
    }

    fn version(&self) -> u32 {
        match self {
            Self::Wlr(obj) => obj.version(),
            Self::Ext(obj) => obj.version(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectionDevice {
    DataDevice(WlDataDevice),
    Primary(PrimaryDevice),
    DataControl(DataControlDevice),
}

impl SelectionDevice {
    pub fn offer(&self, offer: &SelectionOffer) {
        selection_dispatch!(self, offer; Self(device), SelectionOffer(offer) => device.data_offer(offer))
    }

    pub fn version(&self) -> u32 {
        selection_dispatch!(self; Self(device) => device.version())
    }

    pub fn id(&self) -> ObjectId {
        selection_dispatch!(self; Self(device) => device.id())
    }

    /// Get the [`TypeId`] of the underlying data device provider.
    pub fn inner_type_id(&self) -> TypeId {
        match self {
            Self::DataDevice(device) => device.type_id(),
            Self::Primary(device) => device.type_id(),
            Self::DataControl(DataControlDevice::Wlr(device)) => device.type_id(),
            Self::DataControl(DataControlDevice::Ext(device)) => device.type_id(),
        }
    }

    /// [`WlSeat`] associated with this device.
    pub fn seat(&self) -> WlSeat {
        match self {
            SelectionDevice::DataDevice(device) => {
                let data: &DataDeviceUserData = device.data().unwrap();
                data.wl_seat.clone()
            }
            SelectionDevice::Primary(device) => {
                let data: &PrimaryDeviceUserData = device.data().unwrap();
                data.wl_seat.clone()
            }
            SelectionDevice::DataControl(device) => device.wl_seat(),
        }
    }

    /// Send regular selection.
    pub fn selection(&self, offer: &SelectionOffer) {
        match (self, offer) {
            (Self::DataDevice(device), SelectionOffer::DataDevice(offer)) => {
                device.selection(Some(offer));
            }
            (Self::DataControl(device), SelectionOffer::DataControl(offer)) => {
                device.selection(Some(offer));
            }
            _ => unreachable!("non-supported configuration for setting clipboard selection."),
        }
    }

    pub fn unset_selection(&self) {
        match self {
            Self::DataDevice(device) => device.selection(None),
            Self::DataControl(device) => device.selection(None),
            Self::Primary(_) => unreachable!("primary clipboard has no clipboard selection"),
        }
    }

    pub fn primary_selection(&self, offer: &SelectionOffer) {
        match (self, offer) {
            (Self::Primary(device), SelectionOffer::Primary(offer)) => {
                device.selection(Some(offer));
            }
            (Self::DataControl(device), SelectionOffer::DataControl(offer)) => {
                device.primary_selection(Some(offer));
            }
            _ => unreachable!("non-supported configuration for setting clipboard selection."),
        }
    }

    pub fn unset_primary_selection(&self) {
        match self {
            Self::Primary(device) => device.selection(None),
            Self::DataControl(device) => device.primary_selection(None),
            Self::DataDevice(_) => unreachable!("data control has primary selection"),
        }
    }
}
