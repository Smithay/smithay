use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use drm::control::atomic::AtomicModeReq;
use drm::control::{
    connector, crtc, framebuffer, plane, property, AtomicCommitFlags, Device as ControlDevice,
    PropertyValueSet, ResourceHandle,
};

use super::{DevPath, FdWrapper};
use crate::backend::drm::error::Error;

use slog::{error, o, trace};

type OldState = (
    Vec<(connector::Handle, PropertyValueSet)>,
    Vec<(crtc::Handle, PropertyValueSet)>,
    Vec<(framebuffer::Handle, PropertyValueSet)>,
    Vec<(plane::Handle, PropertyValueSet)>,
);

pub type Mapping = (
    HashMap<connector::Handle, HashMap<String, property::Handle>>,
    HashMap<crtc::Handle, HashMap<String, property::Handle>>,
    HashMap<framebuffer::Handle, HashMap<String, property::Handle>>,
    HashMap<plane::Handle, HashMap<String, property::Handle>>,
);
#[derive(Debug)]
pub struct AtomicDrmDevice<A: AsRawFd + 'static> {
    pub(crate) fd: Arc<FdWrapper<A>>,
    pub(crate) active: Arc<AtomicBool>,
    old_state: OldState,
    pub(crate) prop_mapping: Mapping,
    logger: ::slog::Logger,
}

impl<A: AsRawFd + 'static> AtomicDrmDevice<A> {
    pub fn new(
        fd: Arc<FdWrapper<A>>,
        active: Arc<AtomicBool>,
        disable_connectors: bool,
        logger: ::slog::Logger,
    ) -> Result<Self, Error> {
        let mut dev = AtomicDrmDevice {
            fd,
            active,
            old_state: (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
            prop_mapping: (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new()),
            logger: logger.new(o!("smithay_module" => "backend_drm_atomic", "drm_module" => "device")),
        };

        // Enumerate (and save) the current device state.
        let res_handles = dev.fd.resource_handles().map_err(|source| Error::Access {
            errmsg: "Error loading drm resources",
            dev: dev.fd.dev_path(),
            source,
        })?;

        let plane_handles = dev.fd.plane_handles().map_err(|source| Error::Access {
            errmsg: "Error loading planes",
            dev: dev.fd.dev_path(),
            source,
        })?;
        let planes = plane_handles.planes();

        let mut old_state = dev.old_state.clone();
        let mut mapping = dev.prop_mapping.clone();

        // This helper function takes a snapshot of the current device properties.
        // (everything in the atomic api is set via properties.)
        dev.add_props(res_handles.connectors(), &mut old_state.0)?;
        dev.add_props(res_handles.crtcs(), &mut old_state.1)?;
        dev.add_props(res_handles.framebuffers(), &mut old_state.2)?;
        dev.add_props(planes, &mut old_state.3)?;

        // And because the mapping is not consistent across devices,
        // we also need to lookup the handle for a property name.
        // And we do this a fair bit, so lets cache that mapping.
        dev.map_props(res_handles.connectors(), &mut mapping.0)?;
        dev.map_props(res_handles.crtcs(), &mut mapping.1)?;
        dev.map_props(res_handles.framebuffers(), &mut mapping.2)?;
        dev.map_props(planes, &mut mapping.3)?;

        dev.old_state = old_state;
        dev.prop_mapping = mapping;
        trace!(dev.logger, "Mapping: {:#?}", dev.prop_mapping);

        // If the user does not explicitly requests us to skip this,
        // we clear out the complete connector<->crtc mapping on device creation.
        //
        // The reason is, that certain operations may be racy otherwise. Surfaces can
        // exist on different threads: as a result, we cannot really enumerate the current state
        // (it might be changed on another thread during the enumeration). And commits can fail,
        // if e.g. a connector is already bound to another surface, which is difficult to analyse at runtime.
        //
        // An easy workaround is to set a known state on device creation, so we can only
        // run into these errors on our own and not because previous compositors left the device
        // in a funny state.
        if disable_connectors {
            dev.reset_state()?;
        }

        Ok(dev)
    }

    // Add all properties of given handles to a given drm resource type to state.
    // You may use this to snapshot the current state of the drm device (fully or partially).
    fn add_props<T>(&self, handles: &[T], state: &mut Vec<(T, PropertyValueSet)>) -> Result<(), Error>
    where
        T: ResourceHandle,
    {
        let iter = handles.iter().map(|x| (x, self.fd.get_properties(*x)));
        if let Some(len) = iter.size_hint().1 {
            state.reserve_exact(len)
        }

        iter.map(|(x, y)| (*x, y))
            .try_for_each(|(x, y)| match y {
                Ok(y) => {
                    state.push((x, y));
                    Ok(())
                }
                Err(err) => Err(err),
            })
            .map_err(|source| Error::Access {
                errmsg: "Error reading properties",
                dev: self.fd.dev_path(),
                source,
            })
    }

    /// Create a mapping of property names and handles for given handles of a given drm resource type.
    /// You may use this to easily lookup properties by name instead of going through this procedure manually.
    fn map_props<T>(
        &self,
        handles: &[T],
        mapping: &mut HashMap<T, HashMap<String, property::Handle>>,
    ) -> Result<(), Error>
    where
        T: ResourceHandle + Eq + std::hash::Hash,
    {
        handles
            .iter()
            .map(|x| (x, self.fd.get_properties(*x)))
            .try_for_each(|(handle, props)| {
                let mut map = HashMap::new();
                match props {
                    Ok(props) => {
                        let (prop_handles, _) = props.as_props_and_values();
                        for prop in prop_handles {
                            if let Ok(info) = self.fd.get_property(*prop) {
                                let name = info.name().to_string_lossy().into_owned();
                                map.insert(name, *prop);
                            }
                        }
                        mapping.insert(*handle, map);
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            })
            .map_err(|source| Error::Access {
                errmsg: "Error reading properties on {:?}",
                dev: self.fd.dev_path(),
                source,
            })
    }

    pub(super) fn reset_state(&self) -> Result<(), Error> {
        // reset state sets the connectors into a known state (all disabled),
        // for the same reasons we do this on device creation.
        //
        // We might end up with conflicting commit requirements, if we want to restore our state,
        // on top of the state the previous compositor left the device in.
        // This is because we do commits per surface and not per device, so we do a global
        // commit here, to fix any conflicts.
        let res_handles = self.fd.resource_handles().map_err(|source| Error::Access {
            errmsg: "Error loading drm resources",
            dev: self.fd.dev_path(),
            source,
        })?;
        let plane_handles = self.fd.plane_handles().map_err(|source| Error::Access {
            errmsg: "Error loading drm plane resources",
            dev: self.fd.dev_path(),
            source,
        })?;

        // Disable all connectors (otherwise we might run into conflicting commits when restarting the rendering loop)
        let mut req = AtomicModeReq::new();
        for conn in res_handles.connectors() {
            let prop = self
                .prop_mapping
                .0
                .get(conn)
                .expect("Unknown handle")
                .get("CRTC_ID")
                .expect("Unknown property CRTC_ID");
            req.add_property(*conn, *prop, property::Value::CRTC(None));
        }
        // Disable all planes
        for plane in plane_handles.planes() {
            let prop = self
                .prop_mapping
                .3
                .get(plane)
                .expect("Unknown handle")
                .get("CRTC_ID")
                .expect("Unknown property CRTC_ID");
            req.add_property(*plane, *prop, property::Value::CRTC(None));

            let prop = self
                .prop_mapping
                .3
                .get(plane)
                .expect("Unknown handle")
                .get("FB_ID")
                .expect("Unknown property FB_ID");
            req.add_property(*plane, *prop, property::Value::Framebuffer(None));
        }
        // A crtc without a connector has no mode, we also need to reset that.
        // Otherwise the commit will not be accepted.
        for crtc in res_handles.crtcs() {
            let mode_prop = self
                .prop_mapping
                .1
                .get(crtc)
                .expect("Unknown handle")
                .get("MODE_ID")
                .expect("Unknown property MODE_ID");
            let active_prop = self
                .prop_mapping
                .1
                .get(crtc)
                .expect("Unknown handle")
                .get("ACTIVE")
                .expect("Unknown property ACTIVE");
            req.add_property(*crtc, *active_prop, property::Value::Boolean(false));
            req.add_property(*crtc, *mode_prop, property::Value::Unknown(0));
        }
        self.fd
            .atomic_commit(&[AtomicCommitFlags::AllowModeset], req)
            .map_err(|source| Error::Access {
                errmsg: "Failed to disable connectors",
                dev: self.fd.dev_path(),
                source,
            })?;

        Ok(())
    }
}

impl<A: AsRawFd + 'static> Drop for AtomicDrmDevice<A> {
    fn drop(&mut self) {
        if self.active.load(Ordering::SeqCst) {
            // Here we restore the card/tty's to it's previous state.
            // In case e.g. getty was running on the tty sets the correct framebuffer again,
            // so that getty will be visible.
            // We do exit correctly if this fails, but the user will be presented with
            // a black screen if no display handler takes control again.

            // create an atomic mode request consisting of all properties we captured on creation.
            // TODO, check current connector status and remove deactivated connectors from this req.
            let mut req = AtomicModeReq::new();
            fn add_multiple_props<T: ResourceHandle>(
                req: &mut AtomicModeReq,
                old_state: &[(T, PropertyValueSet)],
            ) {
                for (handle, set) in old_state {
                    let (prop_handles, values) = set.as_props_and_values();
                    for (&prop_handle, &val) in prop_handles.iter().zip(values.iter()) {
                        req.add_raw_property((*handle).into(), prop_handle, val);
                    }
                }
            }

            add_multiple_props(&mut req, &self.old_state.0);
            add_multiple_props(&mut req, &self.old_state.1);
            add_multiple_props(&mut req, &self.old_state.2);
            add_multiple_props(&mut req, &self.old_state.3);

            if let Err(err) = self.fd.atomic_commit(&[AtomicCommitFlags::AllowModeset], req) {
                error!(self.logger, "Failed to restore previous state. Error: {}", err);
            }
        }
    }
}
