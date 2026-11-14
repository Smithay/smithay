use std::collections::HashMap;
use std::sync::{Arc, RwLock, atomic::AtomicBool};

use drm::control::atomic::AtomicModeReq;
use drm::control::{
    AtomicCommitFlags, Device as ControlDevice, ResourceHandle, connector, crtc, plane, property,
};

use super::DrmDeviceFd;
use crate::backend::drm::error::AccessError;
use crate::{backend::drm::error::Error, utils::DevPath};

use tracing::{debug, info_span, trace};

#[derive(Clone, Debug, Default)]
pub struct PropMapping {
    pub connectors: HashMap<connector::Handle, HashMap<String, property::Handle>>,
    pub crtcs: HashMap<crtc::Handle, HashMap<String, property::Handle>>,
    pub planes: HashMap<plane::Handle, HashMap<String, property::Handle>>,
}

impl PropMapping {
    pub(crate) fn conn_prop_handle(
        &self,
        handle: connector::Handle,
        name: &'static str,
    ) -> Result<property::Handle, Error> {
        self.connectors
            .get(&handle)
            .ok_or(Error::UnknownConnector(handle))?
            .get(name)
            .ok_or_else(|| Error::UnknownProperty {
                handle: handle.into(),
                name,
            })
            .copied()
    }

    pub(crate) fn crtc_prop_handle(
        &self,
        handle: crtc::Handle,
        name: &'static str,
    ) -> Result<property::Handle, Error> {
        self.crtcs
            .get(&handle)
            .ok_or(Error::UnknownCrtc(handle))?
            .get(name)
            .ok_or_else(|| Error::UnknownProperty {
                handle: handle.into(),
                name,
            })
            .copied()
    }

    pub(crate) fn plane_prop_handle(
        &self,
        handle: plane::Handle,
        name: &'static str,
    ) -> Result<property::Handle, Error> {
        self.planes
            .get(&handle)
            .ok_or(Error::UnknownPlane(handle))?
            .get(name)
            .ok_or_else(|| Error::UnknownProperty {
                handle: handle.into(),
                name,
            })
            .copied()
    }
}

#[derive(Debug)]
pub struct AtomicDrmDevice {
    pub(crate) fd: DrmDeviceFd,
    pub(crate) active: Arc<AtomicBool>,
    pub(crate) prop_mapping: Arc<RwLock<PropMapping>>,
    pub(super) span: tracing::Span,
}

impl AtomicDrmDevice {
    pub fn new(fd: DrmDeviceFd, active: Arc<AtomicBool>, disable_connectors: bool) -> Result<Self, Error> {
        let span = info_span!("drm_atomic");
        let dev = AtomicDrmDevice {
            fd,
            active,
            prop_mapping: Default::default(),
            span,
        };
        let _guard = dev.span.enter();

        // Enumerate the device resources.
        let res_handles = dev.fd.resource_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading drm resources",
                dev: dev.fd.dev_path(),
                source,
            })
        })?;

        let planes = dev.fd.plane_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading planes",
                dev: dev.fd.dev_path(),
                source,
            })
        })?;

        let mut mapping = dev.prop_mapping.write().unwrap();

        // Because the mapping is not consistent across devices,
        // we need to lookup the handle for a property name.
        // And we do this a fair bit, so lets cache that mapping.
        map_props(&dev.fd, res_handles.connectors(), &mut mapping.connectors)?;
        map_props(&dev.fd, res_handles.crtcs(), &mut mapping.crtcs)?;
        map_props(&dev.fd, &planes, &mut mapping.planes)?;

        trace!("Mapping: {:#?}", mapping);

        drop(mapping);

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
            debug!("Resetting drm device to known state");
            dev.reset_state()?;
        }

        drop(_guard);
        Ok(dev)
    }

    pub(super) fn reset_state(&self) -> Result<(), Error> {
        // reset state sets the connectors into a known state (all disabled),
        // for the same reasons we do this on device creation.
        //
        // We might end up with conflicting commit requirements, if we want to restore our state,
        // on top of the state the previous compositor left the device in.
        // This is because we do commits per surface and not per device, so we do a global
        // commit here, to fix any conflicts.
        let res_handles = self.fd.resource_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading drm resources",
                dev: self.fd.dev_path(),
                source,
            })
        })?;
        let plane_handles = self.fd.plane_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading drm plane resources",
                dev: self.fd.dev_path(),
                source,
            })
        })?;

        let mut prop_mapping = self.prop_mapping.write().unwrap();

        // Make sure the mapping is up to date
        prop_mapping.connectors.clear();
        map_props(&self.fd, res_handles.connectors(), &mut prop_mapping.connectors)?;

        // Disable all connectors (otherwise we might run into conflicting commits when restarting the rendering loop)
        let mut req = AtomicModeReq::new();
        for conn in res_handles.connectors() {
            let prop = prop_mapping
                .conn_prop_handle(*conn, "CRTC_ID")
                .expect("Unknown property CRTC_ID");
            req.add_property(*conn, prop, property::Value::CRTC(None));
        }
        // Disable all planes
        for plane in plane_handles {
            let prop = prop_mapping
                .plane_prop_handle(plane, "CRTC_ID")
                .expect("Unknown property CRTC_ID");
            req.add_property(plane, prop, property::Value::CRTC(None));

            let prop = prop_mapping
                .plane_prop_handle(plane, "FB_ID")
                .expect("Unknown property FB_ID");
            req.add_property(plane, prop, property::Value::Framebuffer(None));
        }
        // A crtc without a connector has no mode, we also need to reset that.
        // Otherwise the commit will not be accepted.
        for crtc in res_handles.crtcs() {
            let mode_prop = prop_mapping
                .crtc_prop_handle(*crtc, "MODE_ID")
                .expect("Unknown property MODE_ID");
            let active_prop = prop_mapping
                .crtc_prop_handle(*crtc, "ACTIVE")
                .expect("Unknown property ACTIVE");
            req.add_property(*crtc, active_prop, property::Value::Boolean(false));
            req.add_property(*crtc, mode_prop, property::Value::Unknown(0));
        }
        self.fd
            .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, req)
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to disable connectors",
                    dev: self.fd.dev_path(),
                    source,
                })
            })?;

        Ok(())
    }
}

/// Create a mapping of property names and handles for given handles of a given drm resource type.
/// You may use this to easily lookup properties by name instead of going through this procedure manually.
pub(in crate::backend::drm) fn map_props<D, T>(
    fd: &D,
    handles: &[T],
    mapping: &mut HashMap<T, HashMap<String, property::Handle>>,
) -> Result<(), Error>
where
    D: DevPath + ControlDevice,
    T: ResourceHandle + Eq + std::hash::Hash,
{
    handles
        .iter()
        .map(|x| (x, fd.get_properties(*x)))
        .try_for_each(|(handle, props)| {
            let mut map = HashMap::new();
            match props {
                Ok(props) => {
                    let (prop_handles, _) = props.as_props_and_values();
                    for prop in prop_handles {
                        if let Ok(info) = fd.get_property(*prop) {
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
        .map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error reading properties",
                dev: fd.dev_path(),
                source,
            })
        })
}
