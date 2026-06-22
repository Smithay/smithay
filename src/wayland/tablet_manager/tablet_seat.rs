use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_seat_v2::ZwpTabletSeatV2, zwp_tablet_tool_v2::ZwpTabletToolV2, zwp_tablet_v2::ZwpTabletV2,
};
use wayland_server::{Client, Dispatch, DisplayHandle, Resource};

use crate::{
    input::tablet::{TabletSeat, TabletSeatHandler},
    wayland::{
        Dispatch2,
        compositor::CompositorHandler,
        tablet_manager::{TabletToolUserData, tablet::TabletUserData},
    },
};

impl<D: TabletSeatHandler + 'static> TabletSeat<D> {
    pub(super) fn add_instance(
        &self,
        state: &mut D,
        dh: &DisplayHandle,
        seat: &ZwpTabletSeatV2,
        client: &Client,
    ) where
        D: Dispatch<ZwpTabletV2, TabletUserData>,
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData<D>>,
        D: CompositorHandler,
        D: 'static,
    {
        let mut inner = self.arc.lock().unwrap();

        for (desc, tablet) in inner.tablets.iter_mut() {
            tablet
                .arc
                .wp_tablet
                .new_instance::<D>(client, dh, seat, tablet.clone(), desc);
        }

        for (desc, tool) in inner.tools.iter_mut() {
            tool.arc
                .wp_tablet_tool
                .new_instance::<D>(state, client, dh, seat, tool.clone(), desc)
        }

        inner.instances.push(seat.downgrade())
    }
}

/// User data of ZwpTabletSeatV2 object
#[derive(Debug)]
pub struct TabletSeatUserData<D: TabletSeatHandler> {
    pub(super) handle: TabletSeat<D>,
}

impl<D> Dispatch2<ZwpTabletSeatV2, D> for TabletSeatUserData<D>
where
    D: TabletSeatHandler + 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        _resource: &ZwpTabletSeatV2,
        _request: <ZwpTabletSeatV2 as wayland_server::Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(&self, _state: &mut D, _client: wayland_server::backend::ClientId, seat: &ZwpTabletSeatV2) {
        self.handle
            .arc
            .lock()
            .unwrap()
            .instances
            .retain(|i| i.id() != seat.id());
    }
}
