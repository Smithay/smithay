use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::ZwpTabletToolV2,
    zwp_tablet_v2::ZwpTabletV2,
};
use wayland_server::{backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, Resource, Weak};

use crate::input::pointer::CursorImageStatus;
use crate::{backend::input::TabletToolDescriptor, wayland::compositor::CompositorHandler};

use super::{
    tablet::TabletUserData,
    tablet_tool::{TabletToolHandle, TabletToolUserData},
};
use super::{
    tablet::{TabletDescriptor, TabletHandle},
    TabletManagerState,
};

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct TabletSeat {
    instances: Vec<Weak<ZwpTabletSeatV2>>,
    tablets: HashMap<TabletDescriptor, TabletHandle>,
    tools: HashMap<TabletToolDescriptor, TabletToolHandle>,
}

impl fmt::Debug for TabletSeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabletSeat")
            .field("instances", &self.instances)
            .field("tablets", &self.tablets)
            .field("tools", &self.tools)
            .finish()
    }
}

/// Handler trait for Tablet Seats
pub trait TabletSeatHandler {
    /// Callback that will be notified whenever a client requests to set a custom tool image.
    fn tablet_tool_image(&mut self, tool: &TabletToolDescriptor, image: CursorImageStatus) {
        let _ = tool;
        let _ = image;
    }
}

/// Handle to a tablet seat
///
/// TabletSeat extends `Seat` with graphic tablet specific functionality
///
/// TabletSeatHandle can be used to advertise available graphics tablets and tools to wayland clients
#[derive(Default, Debug, Clone)]
pub struct TabletSeatHandle {
    pub(crate) inner: Arc<Mutex<TabletSeat>>,
}

impl TabletSeatHandle {
    pub(super) fn add_instance<D>(
        &self,
        state: &mut D,
        dh: &DisplayHandle,
        seat: &ZwpTabletSeatV2,
        client: &Client,
    ) where
        D: Dispatch<ZwpTabletV2, TabletUserData>,
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData>,
        D: TabletSeatHandler + 'static,
        D: CompositorHandler,
    {
        let mut inner = self.inner.lock().unwrap();

        // Notify new instance about available tablets
        for (desc, tablet) in inner.tablets.iter_mut() {
            tablet.new_instance::<D>(client, dh, seat, desc);
        }

        // Notify new instance about available tools
        for (desc, tool) in inner.tools.iter_mut() {
            tool.new_instance(state, client, dh, seat, desc);
        }

        inner.instances.push(seat.downgrade());
    }

    /// Add a new tablet to a seat.
    ///
    /// You can either add tablet on [input::Event::DeviceAdded](crate::backend::input::InputEvent::DeviceAdded) event,
    /// or you can add tablet based on tool event, then clients will not know about devices that are not being used
    ///
    /// Returns new [TabletHandle] if tablet was not know by this seat, if tablet was already know it returns existing handle.
    pub fn add_tablet<D>(&self, dh: &DisplayHandle, tablet_desc: &TabletDescriptor) -> TabletHandle
    where
        D: Dispatch<ZwpTabletV2, TabletUserData>,
        D: 'static,
    {
        let inner = &mut *self.inner.lock().unwrap();

        let tablets = &mut inner.tablets;
        let instances = &inner.instances;

        let tablet = tablets.entry(tablet_desc.clone()).or_insert_with(|| {
            let mut tablet = TabletHandle::default();
            // Create new tablet instance for every seat instance
            for seat in instances.iter() {
                let Ok(seat) = seat.upgrade() else {
                    continue;
                };

                if let Ok(client) = dh.get_client(seat.id()) {
                    tablet.new_instance::<D>(&client, dh, &seat, tablet_desc);
                }
            }
            tablet
        });

        tablet.clone()
    }

    /// Get a handle to a tablet
    pub fn get_tablet(&self, tablet_desc: &TabletDescriptor) -> Option<TabletHandle> {
        self.inner.lock().unwrap().tablets.get(tablet_desc).cloned()
    }

    /// Count all tablet devices
    pub fn count_tablets(&self) -> usize {
        self.inner.lock().unwrap().tablets.len()
    }

    /// Remove tablet device
    ///
    /// Called when tablet is no longer available
    /// For example on [input::Event::DeviceRemoved](crate::backend::input::InputEvent::DeviceRemoved) event.
    pub fn remove_tablet(&self, tablet_desc: &TabletDescriptor) {
        self.inner.lock().unwrap().tablets.remove(tablet_desc);
    }

    /// Remove all tablet devices
    pub fn clear_tablets(&self) {
        self.inner.lock().unwrap().tablets.clear();
    }

    /// Add a new tool to a seat.
    ///
    /// Tool is usually added on [TabletToolProximityEvent](crate::backend::input::InputEvent::TabletToolProximity) event.
    ///
    /// Returns new [TabletToolHandle] if tool was not know by this seat, if tool was already know it returns existing handle,
    /// it allows you to send tool input events to clients.
    pub fn add_tool<D>(
        &self,
        state: &mut D,
        dh: &DisplayHandle,
        tool_desc: &TabletToolDescriptor,
    ) -> TabletToolHandle
    where
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData>,
        D: TabletSeatHandler + 'static,
        D: CompositorHandler,
    {
        let inner = &mut *self.inner.lock().unwrap();

        let tools = &mut inner.tools;
        let instances = &inner.instances;

        let tool = tools.entry(tool_desc.clone()).or_insert_with(|| {
            let mut tool = TabletToolHandle::default();
            // Create new tool instance for every seat instance
            for seat in instances.iter() {
                let Ok(seat) = seat.upgrade() else {
                    continue;
                };

                if let Ok(client) = dh.get_client(seat.id()) {
                    tool.new_instance(state, &client, dh, &seat, tool_desc);
                }
            }
            tool
        });

        tool.clone()
    }

    /// Get a handle to a tablet tool
    pub fn get_tool(&self, tool_desc: &TabletToolDescriptor) -> Option<TabletToolHandle> {
        self.inner.lock().unwrap().tools.get(tool_desc).cloned()
    }

    /// Count all tablet tool devices
    pub fn count_tools(&self) -> usize {
        self.inner.lock().unwrap().tools.len()
    }

    /// Remove tablet tool device
    ///
    /// Policy of tool removal is a compositor-specific.
    ///
    /// One possible policy would be to remove a tool when all tablets the tool was used on are removed.
    pub fn remove_tool(&self, tool_desc: &TabletToolDescriptor) {
        self.inner.lock().unwrap().tools.remove(tool_desc);
    }

    /// Remove all tablet tool devices
    pub fn clear_tools(&self) {
        self.inner.lock().unwrap().tools.clear();
    }
}

/// User data of ZwpTabletSeatV2 object
#[derive(Debug)]
pub struct TabletSeatUserData {
    pub(super) handle: TabletSeatHandle,
}

impl<D> Dispatch<ZwpTabletSeatV2, TabletSeatUserData, D> for TabletManagerState
where
    D: Dispatch<ZwpTabletSeatV2, TabletSeatUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _seat: &ZwpTabletSeatV2,
        _request: zwp_tablet_seat_v2::Request,
        _data: &TabletSeatUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(_state: &mut D, _client: ClientId, seat: &ZwpTabletSeatV2, data: &TabletSeatUserData) {
        data.handle
            .inner
            .lock()
            .unwrap()
            .instances
            .retain(|i| i.id() != seat.id());
    }
}
