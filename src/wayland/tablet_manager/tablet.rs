use std::ops::Deref as _;
use std::path::PathBuf;
use std::{cell::RefCell, rc::Rc};

use wayland_protocols::unstable::tablet::v2::server::zwp_tablet_seat_v2::ZwpTabletSeatV2;
use wayland_protocols::unstable::tablet::v2::server::zwp_tablet_v2::ZwpTabletV2;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::Filter;

use crate::backend::input::Device;

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
    inner: Rc<RefCell<Tablet>>,
}

impl TabletHandle {
    pub(super) fn new_instance(&mut self, seat: &ZwpTabletSeatV2, tablet: &TabletDescriptor) {
        if let Some(client) = seat.as_ref().client() {
            let wl_tablet = client
                .create_resource::<ZwpTabletV2>(seat.as_ref().version())
                .unwrap();

            wl_tablet.quick_assign(|_, _req, _| {});

            let inner = self.inner.clone();
            wl_tablet.assign_destructor(Filter::new(move |instance: ZwpTabletV2, _, _| {
                inner
                    .borrow_mut()
                    .instances
                    .retain(|i| !i.as_ref().equals(instance.as_ref()));
            }));

            seat.tablet_added(&wl_tablet);

            wl_tablet.name(tablet.name.clone());

            if let Some((id_product, id_vendor)) = tablet.usb_id {
                wl_tablet.id(id_product, id_vendor);
            }

            if let Some(syspath) = tablet.syspath.as_ref().and_then(|p| p.to_str()) {
                wl_tablet.path(syspath.to_owned());
            }

            wl_tablet.done();

            self.inner.borrow_mut().instances.push(wl_tablet.deref().clone());
        }
    }

    pub(super) fn with_focused_tablet<F>(&self, focus: &WlSurface, cb: F)
    where
        F: Fn(&ZwpTabletV2),
    {
        if let Some(instance) = self
            .inner
            .borrow()
            .instances
            .iter()
            .find(|i| i.as_ref().same_client_as(focus.as_ref()))
        {
            cb(instance);
        }
    }
}
