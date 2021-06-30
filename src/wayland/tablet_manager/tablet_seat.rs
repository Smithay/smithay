use wayland_protocols::unstable::tablet::v2::server::zwp_tablet_seat_v2::ZwpTabletSeatV2;
use wayland_server::{Filter, Main};

use crate::backend::input::TabletToolDescriptor;
use crate::wayland::seat::CursorImageStatus;

use super::tablet::{TabletDescriptor, TabletHandle};
use super::tablet_tool::TabletToolHandle;

use std::convert::AsRef;
use std::fmt;
use std::ops::Deref as _;
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Default)]
struct TabletSeat {
    instances: Vec<ZwpTabletSeatV2>,
    tablets: HashMap<TabletDescriptor, TabletHandle>,
    tools: HashMap<TabletToolDescriptor, TabletToolHandle>,

    cursor_callback: Option<Box<dyn FnMut(&TabletToolDescriptor, CursorImageStatus)>>,
}

impl fmt::Debug for TabletSeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabletSeat")
            .field("instances", &self.instances)
            .field("tablets", &self.tablets)
            .field("tools", &self.tools)
            .field(
                "cursor_callback",
                if self.cursor_callback.is_some() {
                    &"Some(...)"
                } else {
                    &"None"
                },
            )
            .finish()
    }
}

/// Handle to a tablet seat
///
/// TabletSeat extends `Seat` with graphic tablet specyfic functionality
///
/// TabletSeatHandle can be used to advertise avalible graphics tablets and tools to wayland clients
#[derive(Default, Debug, Clone)]
pub struct TabletSeatHandle {
    inner: Rc<RefCell<TabletSeat>>,
}

impl TabletSeatHandle {
    pub(super) fn add_instance(&self, seat: Main<ZwpTabletSeatV2>) {
        let mut inner = self.inner.borrow_mut();

        // Notify new instance about avaluble tablets
        for (desc, tablet) in inner.tablets.iter_mut() {
            tablet.new_instance(seat.deref(), desc);
        }

        // Notify new instance about avalible tools
        for (desc, tool) in inner.tools.iter_mut() {
            let inner = self.inner.clone();
            tool.new_instance(seat.deref(), desc, move |desc, status| {
                if let Some(ref mut cursor_callback) = inner.borrow_mut().cursor_callback {
                    cursor_callback(desc, status);
                }
            });
        }

        inner.instances.push(seat.deref().clone());

        let inner = self.inner.clone();
        seat.assign_destructor(Filter::new(move |seat: ZwpTabletSeatV2, _, _| {
            inner
                .borrow_mut()
                .instances
                .retain(|i| !i.as_ref().equals(&seat.as_ref()));
        }));
    }

    /// Add a callback to SetCursor event
    pub fn on_cursor_surface<F>(&mut self, cb: F)
    where
        F: FnMut(&TabletToolDescriptor, CursorImageStatus) + 'static,
    {
        self.inner.borrow_mut().cursor_callback = Some(Box::new(cb));
    }

    /// Add a new tablet to a seat.
    ///
    /// You can either add tablet on [LibinputEvent::NewDevice](crate::backend::libinput::LibinputEvent::NewDevice) event,
    /// or you can add tablet based on tool event, then clients will not know about devices that are not being used
    ///
    /// Returns new [TabletHandle] if tablet was not know by this seat, if tablet was allready know it returns exsisting handle.
    pub fn add_tablet(&self, tablet_desc: &TabletDescriptor) -> TabletHandle {
        let inner = &mut *self.inner.borrow_mut();

        let tablets = &mut inner.tablets;
        let instances = &inner.instances;

        let tablet = tablets.entry(tablet_desc.clone()).or_insert_with(|| {
            let mut tablet = TabletHandle::default();
            // Create new tablet instance for every seat instance
            for seat in instances.iter() {
                tablet.new_instance(seat, tablet_desc);
            }
            tablet
        });

        tablet.clone()
    }

    /// Get a handle to a tablet
    pub fn get_tablet(&self, tablet_desc: &TabletDescriptor) -> Option<TabletHandle> {
        self.inner.borrow().tablets.get(tablet_desc).cloned()
    }

    /// Count all tablet devices
    pub fn count_tablets(&self) -> usize {
        self.inner.borrow_mut().tablets.len()
    }

    /// Remove tablet device
    ///
    /// Called when tablet is no longer avalible
    /// For example on [LibinputEvent::RemovedDevice](crate::backend::libinput::LibinputEvent::RemovedDevice) event.
    pub fn remove_tablet(&self, tablet_desc: &TabletDescriptor) {
        self.inner.borrow_mut().tablets.remove(tablet_desc);
    }

    /// Remove all tablet devices
    pub fn clear_tablets(&self) {
        self.inner.borrow_mut().tablets.clear();
    }

    /// Add a new tool to a seat.
    ///
    /// Tool is usually added on [TabletToolProximityEvent](crate::backend::input::InputEvent::TabletToolProximity) event.
    ///
    /// Returns new [TabletToolHandle] if tool was not know by this seat, if tool was allready know it returns exsisting handle,
    /// it allows you to send tool input events to clients.
    pub fn add_tool(&self, tool_desc: &TabletToolDescriptor) -> TabletToolHandle {
        let inner = &mut *self.inner.borrow_mut();

        let tools = &mut inner.tools;
        let instances = &inner.instances;

        let tool = tools.entry(tool_desc.clone()).or_insert_with(|| {
            let mut tool = TabletToolHandle::default();
            // Create new tool instance for every seat instance
            for seat in instances.iter() {
                let inner = self.inner.clone();
                tool.new_instance(seat.deref(), tool_desc, move |desc, status| {
                    if let Some(ref mut cursor_callback) = inner.borrow_mut().cursor_callback {
                        cursor_callback(desc, status);
                    }
                });
            }
            tool
        });

        tool.clone()
    }

    /// Get a handle to a tablet tool
    pub fn get_tool(&self, tool_desc: &TabletToolDescriptor) -> Option<TabletToolHandle> {
        self.inner.borrow().tools.get(tool_desc).cloned()
    }

    /// Count all tablet tool devices
    pub fn count_tools(&self) -> usize {
        self.inner.borrow_mut().tools.len()
    }

    /// Remove tablet tool device
    ///
    /// Policy of tool removal is a compositor-specific.
    ///
    /// One posible policy would be to remove a tool when all tablets the tool was used on are removed.
    pub fn remove_tool(&self, tool_desc: &TabletToolDescriptor) {
        self.inner.borrow_mut().tools.remove(tool_desc);
    }

    /// Remove all tablet tool devices
    pub fn clear_tools(&self) {
        self.inner.borrow_mut().tools.clear();
    }
}
