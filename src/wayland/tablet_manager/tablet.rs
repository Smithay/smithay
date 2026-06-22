use std::sync::{Arc, Mutex};

use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_seat_v2::ZwpTabletSeatV2, zwp_tablet_v2::ZwpTabletV2,
};
use wayland_server::{
    Client, Dispatch, DisplayHandle, Resource, Weak, backend::ObjectId, protocol::wl_surface::WlSurface,
};

use crate::{
    input::tablet::{Tablet, TabletDescriptor, TabletRc, TabletSeat, TabletSeatHandler, WeakTablet},
    wayland::Dispatch2,
};

impl Tablet {
    fn new_bound(descriptor: TabletDescriptor) -> Self {
        Self {
            arc: Arc::new(TabletRc {
                descriptor,
                #[cfg(feature = "wayland_frontend")]
                wp_tablet: WpTabletHandle {
                    bound: true,
                    known_instances: Default::default(),
                },
            }),
        }
    }
}

impl<D: TabletSeatHandler + 'static> TabletSeat<D> {
    /// Add a new tablet to a seat, and exposes it to wayland clients
    ///
    /// You can either add tablet on [DeviceAdded] event, or you can add tablet based
    /// on tool event, then clients will not know about devices that are not being used.
    ///
    /// If the tablet was already known it removes it and recreate a new handle. Because
    /// [`TabletToolHandle`] will keep a handle to the [`Tablet`] while in proximity, it may appears
    /// to clients that the tablet hasn't been removed until the tool leave its proximity.
    ///
    /// [DeviceAdded]: crate::backend::input::InputEvent::DeviceAdded
    /// [`TabletToolHandle`]: crate::input::tablet::tool::TabletToolHandle
    pub fn add_wp_tablet(&self, dh: &DisplayHandle, tablet_desc: &TabletDescriptor) -> Tablet
    where
        D: Dispatch<ZwpTabletV2, TabletUserData>,
        D: 'static,
    {
        let inner = &mut *self.arc.lock().unwrap();

        let tablet = inner.add_tablet(tablet_desc, Tablet::new_bound);
        let instances = &mut inner.instances;

        for seat in instances.iter() {
            let Ok(seat) = seat.upgrade() else {
                continue;
            };

            if let Ok(client) = dh.get_client(seat.id()) {
                tablet
                    .arc
                    .wp_tablet
                    .new_instance::<D>(&client, dh, &seat, tablet.clone(), tablet_desc);
            }
        }

        tablet
    }
}

/// User data of ZwpTabletV2 object
#[derive(Debug)]
pub struct TabletUserData {
    tablet: WeakTablet,
    seat_id: ObjectId,
}

#[derive(Default, Debug)]
pub(crate) struct WpTabletHandle {
    bound: bool,
    known_instances: Mutex<Vec<Weak<wayland_protocols::wp::tablet::zv2::server::zwp_tablet_v2::ZwpTabletV2>>>,
}

impl WpTabletHandle {
    pub(super) fn new_instance<D>(
        &self,
        client: &Client,
        dh: &DisplayHandle,
        seat: &ZwpTabletSeatV2,
        tablet: Tablet,
        desc: &TabletDescriptor,
    ) where
        D: Dispatch<ZwpTabletV2, TabletUserData>,
        D: 'static,
    {
        if !self.bound {
            return;
        }

        let wp_tablet = client
            .create_resource::<ZwpTabletV2, _, D>(
                dh,
                seat.version(),
                TabletUserData {
                    tablet: tablet.downgrade(),
                    seat_id: seat.id(),
                },
            )
            .unwrap();

        seat.tablet_added(&wp_tablet);

        wp_tablet.name(desc.name.clone());

        if let Some((id_product, id_vendor)) = desc.usb_id {
            wp_tablet.id(id_product, id_vendor);
        }

        if let Some(syspath) = desc.syspath.as_ref().and_then(|p| p.to_str()) {
            wp_tablet.path(syspath.to_owned());
        }

        wp_tablet.done();

        self.known_instances.lock().unwrap().push(wp_tablet.downgrade());
    }

    pub(crate) fn focused_tablet_for_seat(
        &self,
        surface: &WlSurface,
        seat_id: &ObjectId,
    ) -> Option<ZwpTabletV2> {
        self.known_instances.lock().unwrap().iter().find_map(|tablet| {
            tablet.upgrade().ok().filter(|wp_tablet| {
                Resource::id(wp_tablet).same_client_as(&surface.id())
                    && &wp_tablet.data::<TabletUserData>().unwrap().seat_id == seat_id
            })
        })
    }
}

impl Drop for WpTabletHandle {
    fn drop(&mut self) {
        let mut guard = self.known_instances.lock().unwrap();

        for wp_tablet in guard.drain(..) {
            let Ok(wp_tablet) = wp_tablet.upgrade() else {
                continue;
            };

            wp_tablet.removed();
        }
    }
}

impl<D> Dispatch2<ZwpTabletV2, D> for TabletUserData
where
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwpTabletV2,
        _request: <ZwpTabletV2 as wayland_server::Resource>::Request,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(&self, _state: &mut D, _client: wayland_server::backend::ClientId, wp_tablet: &ZwpTabletV2) {
        let Some(tablet) = self.tablet.upgrade() else {
            return;
        };

        tablet
            .arc
            .wp_tablet
            .known_instances
            .lock()
            .unwrap()
            .retain(|i| i.id() != Resource::id(wp_tablet));
    }
}
