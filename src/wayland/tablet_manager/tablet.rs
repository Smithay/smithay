use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_seat_v2::ZwpTabletSeatV2,
    zwp_tablet_v2::{self, ZwpTabletV2},
};
use wayland_server::{
    backend::ClientId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, Resource,
};

use crate::backend::input::Device;

use super::TabletManagerState;

/// Description of graphics tablet device
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct TabletDescriptor {
    /// Tablet device name
    pub name: String,
    /// Tablet device USB (product,vendor) id
    pub usb_id: Option<(u32, u32)>,
    /// Path to the device
    pub syspath: Option<PathBuf>,
}

impl<D: Device> From<&D> for TabletDescriptor {
    fn from(device: &D) -> Self {
        TabletDescriptor {
            name: device.name(),
            syspath: device.syspath(),
            usb_id: device.usb_id(),
        }
    }
}

#[derive(Debug, Default)]
struct Tablet {
    instances: Vec<ZwpTabletV2>,
}

/// Handle to a tablet device
///
/// Tablet represents one graphics tablet device
#[derive(Debug, Default, Clone)]
pub struct TabletHandle {
    inner: Arc<Mutex<Tablet>>,
}

impl TabletHandle {
    pub(super) fn new_instance<D>(
        &mut self,
        client: &Client,
        dh: &DisplayHandle,
        seat: &ZwpTabletSeatV2,
        tablet: &TabletDescriptor,
    ) where
        D: Dispatch<ZwpTabletV2, TabletUserData>,
        D: 'static,
    {
        let wl_tablet = client
            .create_resource::<ZwpTabletV2, _, D>(dh, seat.version(), TabletUserData { handle: self.clone() })
            .unwrap();

        seat.tablet_added(&wl_tablet);

        wl_tablet.name(tablet.name.clone());

        if let Some((id_product, id_vendor)) = tablet.usb_id {
            wl_tablet.id(id_product, id_vendor);
        }

        if let Some(syspath) = tablet.syspath.as_ref().and_then(|p| p.to_str()) {
            wl_tablet.path(syspath.to_owned());
        }

        wl_tablet.done();

        self.inner.lock().unwrap().instances.push(wl_tablet);
    }

    pub(super) fn with_focused_tablet<F>(&self, focus: &WlSurface, cb: F)
    where
        F: Fn(&ZwpTabletV2),
    {
        if let Some(instance) = self
            .inner
            .lock()
            .unwrap()
            .instances
            .iter()
            .find(|i| Resource::id(*i).same_client_as(&focus.id()))
        {
            cb(instance);
        }
    }
}

/// User data of ZwpTabletV2 object
#[derive(Debug)]
pub struct TabletUserData {
    handle: TabletHandle,
}

impl<D> Dispatch<ZwpTabletV2, TabletUserData, D> for TabletManagerState
where
    D: Dispatch<ZwpTabletV2, TabletUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _tablet: &ZwpTabletV2,
        _request: zwp_tablet_v2::Request,
        _data: &TabletUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client: ClientId, tablet: &ZwpTabletV2, data: &TabletUserData) {
        data.handle
            .inner
            .lock()
            .unwrap()
            .instances
            .retain(|i| Resource::id(i) != Resource::id(tablet));
    }
}
