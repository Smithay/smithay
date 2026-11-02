use std::sync::{Arc, atomic::AtomicBool};

use drm::control::{Device as ControlDevice, connector};

use super::DrmDeviceFd;
use crate::backend::drm::error::{AccessError, Error};
use crate::utils::DevPath;

use tracing::{debug, info_span, trace};

#[derive(Debug)]
pub struct LegacyDrmDevice {
    pub(crate) fd: DrmDeviceFd,
    pub(crate) active: Arc<AtomicBool>,
    pub(super) span: tracing::Span,
}

impl LegacyDrmDevice {
    pub fn new(fd: DrmDeviceFd, active: Arc<AtomicBool>, disable_connectors: bool) -> Result<Self, Error> {
        let span = info_span!("drm_legacy");
        let dev = LegacyDrmDevice { fd, active, span };
        let _guard = dev.span.enter();

        // If the user does not explicitly requests us to skip this,
        // we clear out the complete connector<->crtc mapping on device creation.
        //
        // The reason is, that certain operations may be racy otherwise, as surfaces can
        // exist on different threads. As a result, we cannot enumerate the current state
        // on surface creation (it might be changed on another thread during the enumeration).
        // An easy workaround is to set a known state on device creation.
        if disable_connectors {
            debug!("Resetting drm device to known state");
            dev.reset_state()?;
        }

        drop(_guard);
        Ok(dev)
    }

    pub(super) fn reset_state(&self) -> Result<(), Error> {
        let res_handles = self.fd.resource_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Failed to query resource handles",
                dev: self.fd.dev_path(),
                source,
            })
        })?;
        set_connector_state(&self.fd, res_handles.connectors().iter().copied(), false)?;

        for crtc in res_handles.crtcs() {
            #[allow(deprecated)]
            let _ = self
                .fd
                .set_cursor(*crtc, Option::<&drm::control::dumbbuffer::DumbBuffer>::None);
            // null commit (necessary to trigger removal on the kernel side with the legacy api.)
            self.fd
                .set_crtc(*crtc, None, (0, 0), &[], None)
                .map_err(|source| {
                    Error::Access(AccessError {
                        errmsg: "Error setting crtc",
                        dev: self.fd.dev_path(),
                        source,
                    })
                })?;
        }

        Ok(())
    }
}

pub fn set_connector_state<D>(
    dev: &D,
    connectors: impl Iterator<Item = connector::Handle>,
    enabled: bool,
) -> Result<(), Error>
where
    D: DevPath + ControlDevice,
{
    // for every connector...
    for conn in connectors {
        let info = dev.get_connector(conn, false).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Failed to get connector infos",
                dev: dev.dev_path(),
                source,
            })
        })?;
        // that is currently connected ...
        if info.state() == connector::State::Connected {
            // get a list of it's properties.
            let props = dev.get_properties(conn).map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to get properties for connector",
                    dev: dev.dev_path(),
                    source,
                })
            })?;
            let (handles, _) = props.as_props_and_values();
            // for every handle ...
            for handle in handles {
                // get information of that property
                let info = dev.get_property(*handle).map_err(|source| {
                    Error::Access(AccessError {
                        errmsg: "Failed to get property of connector",
                        dev: dev.dev_path(),
                        source,
                    })
                })?;
                // to find out, if we got the handle of the "DPMS" property ...
                if info.name().to_str().map(|x| x == "DPMS").unwrap_or(false) {
                    // so we can use that to turn on / off the connector
                    trace!(connector = ?conn, "Setting DPMS {}", enabled);
                    dev.set_property(
                        conn,
                        *handle,
                        if enabled {
                            drm_ffi::DRM_MODE_DPMS_ON.into()
                        } else {
                            drm_ffi::DRM_MODE_DPMS_OFF.into()
                        },
                    )
                    .map_err(|source| {
                        Error::Access(AccessError {
                            errmsg: "Failed to set property of connector",
                            dev: dev.dev_path(),
                            source,
                        })
                    })?;
                }
            }
        }
    }
    Ok(())
}
