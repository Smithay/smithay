use std::any::Any;
use std::any::TypeId;

use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1 as PrimaryDevice;
use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
use wayland_server::backend::ObjectId;
use wayland_server::protocol::wl_data_device::WlDataDevice;
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::Resource;

use super::data_device::DataDeviceUserData;
use super::offer::SelectionOffer;
use super::primary_selection::PrimaryDeviceUserData;
use super::private::selection_dispatch;
use super::wlr_data_control::DataControlDeviceUserData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionDevice {
    DataDevice(WlDataDevice),
    Primary(PrimaryDevice),
    DataControl(ZwlrDataControlDeviceV1),
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
        selection_dispatch!(self; Self(device) => device.type_id())
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
            SelectionDevice::DataControl(device) => {
                let data: &DataControlDeviceUserData = device.data().unwrap();
                data.wl_seat.clone()
            }
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
