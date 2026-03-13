use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use wayland_backend::server::ClientId;
use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
    zwp_tablet_seat_v2::ZwpTabletSeatV2,
};
use wayland_server::{
    protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, Resource, Weak,
};

use crate::{backend::input::Device, wayland::tablet_manager::TabletManagerState};

/// Description of graphics tablet pad group
#[derive(Debug, Clone)]
pub struct TabletPadGroupDescriptor {
    /// Buttons available in the group
    pub buttons: Vec<u32>,
}

/// Description of graphics tablet pad device
#[derive(Debug, Clone)]
pub struct TabletPadDescriptor {
    /// Unique id of the device at a point in time.
    pub id: String,
    /// Tablet device name
    pub name: String,
    /// Tablet device USB (product,vendor) id
    pub usb_id: Option<(u32, u32)>,
    /// Path to the device
    pub syspath: Option<PathBuf>,
    /// The number of buttons on a device
    pub number_of_buttons: u32,
    /// Most devices only provide a single mode group, however devices
    /// such as the Wacom Cintiq 22HD provide two mode groups.
    pub groups: Vec<TabletPadGroupDescriptor>,
}

impl From<&input::Device> for TabletPadDescriptor {
    #[inline]
    fn from(device: &input::Device) -> Self {
        let number_of_buttons = device.tablet_pad_number_of_buttons().max(0) as u32;
        let _number_of_rings = device.tablet_pad_number_of_rings().max(0) as u32;
        let _number_of_strips = device.tablet_pad_number_of_strips().max(0) as u32;

        let groups = (0..device.tablet_pad_number_of_mode_groups().max(0) as u32)
            .map(|idx| {
                let group = device.tablet_pad_mode_group(idx).unwrap();

                let mut buttons = Vec::new();

                for idx in 0..number_of_buttons {
                    if group.has_button(idx) {
                        buttons.push(idx);
                    }
                }

                // let mut rings = Vec::new();
                // for idx in 0..number_of_rings {
                //     if group.has_ring(idx) {
                //         rings.push(idx);
                //     }
                // }

                // let mut strips = Vec::new();
                // for idx in 0..number_of_strips {
                //     if group.has_strip(idx) {
                //         strips.push(idx);
                //     }
                // }

                // No modes support for now
                let _number_of_modes = group.number_of_modes();

                TabletPadGroupDescriptor { buttons }
            })
            .collect();

        Self {
            id: device.id(),
            name: device.name().into(),
            syspath: device.syspath(),
            usb_id: device.usb_id(),
            number_of_buttons,
            groups,
        }
    }
}

#[derive(Debug, Default)]
struct TabletPad {
    instances: Vec<Weak<ZwpTabletPadV2>>,
}

/// Handle to a tablet pad device
#[derive(Debug, Clone)]
pub struct TabletPadHandle {
    inner: Arc<Mutex<TabletPad>>,
    desc: TabletPadDescriptor,
}

impl TabletPadHandle {
    pub(super) fn new(desc: TabletPadDescriptor) -> Self {
        Self {
            inner: Default::default(),
            desc,
        }
    }

    pub(super) fn new_instance<D>(&mut self, client: &Client, dh: &DisplayHandle, seat: &ZwpTabletSeatV2)
    where
        D: Dispatch<ZwpTabletPadV2, TabletPadUserData>,
        D: 'static,
    {
        let wl_tablet_pad = client
            .create_resource::<ZwpTabletPadV2, _, D>(
                dh,
                seat.version(),
                TabletPadUserData { handle: self.clone() },
            )
            .unwrap();

        seat.pad_added(&wl_tablet_pad);

        wl_tablet_pad.buttons(self.desc.number_of_buttons);

        if let Some(syspath) = self.desc.syspath.as_ref().and_then(|p| p.to_str()) {
            wl_tablet_pad.path(syspath.to_owned());
        }

        for _grp in self.desc.groups.iter() {
            // TODO
        }

        wl_tablet_pad.done();

        self.inner
            .lock()
            .unwrap()
            .instances
            .push(wl_tablet_pad.downgrade());
    }
}

/// User data of ZwpTabletPadV2 object
#[derive(Debug)]
pub struct TabletPadUserData {
    handle: TabletPadHandle,
}

impl<D> Dispatch<ZwpTabletPadV2, TabletPadUserData, D> for TabletManagerState
where
    D: Dispatch<ZwpTabletPadV2, TabletPadUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _tablet: &ZwpTabletPadV2,
        _request: zwp_tablet_pad_v2::Request,
        _data: &TabletPadUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client: ClientId, pad: &ZwpTabletPadV2, data: &TabletPadUserData) {
        data.handle
            .inner
            .lock()
            .unwrap()
            .instances
            .retain(|i| i.id() != Resource::id(pad));
    }
}
