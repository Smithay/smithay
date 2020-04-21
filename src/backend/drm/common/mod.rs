//!
//! Module for common/shared types of the various [`Device`](::backend::drm::Device)
//! and [`Surface`](::backend::drm::Surface) implementations of the `backend::drm` module.
//!

use drm::control::{connector, crtc, Mode, RawResourceHandle};

use std::path::PathBuf;

pub mod fallback;

/// Errors thrown by the [`LegacyDrmDevice`](::backend::drm::legacy::LegacyDrmDevice),
/// [`AtomicDrmDevice`](::backend::drm::atomic::AtomicDrmDevice)
/// and their surfaces: [`LegacyDrmSurface`](::backend::drm::legacy::LegacyDrmSurface)
/// and [`AtomicDrmSurface`](::backend::drm::atomic::AtomicDrmSurface).
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Unable to acquire DRM master
    #[error("Failed to aquire DRM master")]
    DrmMasterFailed,
    /// The `DrmDevice` encountered an access error
    #[error("DRM access error: {errmsg} on device `{dev:?}`")]
    Access {
        /// Error message associated to the access error
        errmsg: &'static str,
        /// Device on which the error was generated
        dev: Option<PathBuf>,
        /// Underlying device error
        source: failure::Compat<drm::SystemError>,
    },
    /// Unable to determine device id of drm device
    #[error("Unable to determine device id of drm device")]
    UnableToGetDeviceId(#[source] nix::Error),
    /// Device is currently paused
    #[error("Device is currently paused, operation rejected")]
    DeviceInactive,
    /// Mode is not compatible with all given connectors
    #[error("Mode `{0:?}` is not compatible with all given connectors")]
    ModeNotSuitable(Mode),
    /// The given crtc is already in use by another backend
    #[error("Crtc `{0:?}` is already in use by another backend")]
    CrtcAlreadyInUse(crtc::Handle),
    /// No encoder was found for a given connector on the set crtc
    #[error("No encoder found for the given connector '{connector:?}' on crtc `{crtc:?}`")]
    NoSuitableEncoder {
        /// Connector
        connector: connector::Handle,
        /// CRTC
        crtc: crtc::Handle,
    },
    /// No matching primary and cursor plane could be found for the given crtc
    #[error("No matching primary and cursor plane could be found for crtc {crtc:?} on {dev:?}")]
    NoSuitablePlanes {
        /// CRTC
        crtc: crtc::Handle,
        /// Device on which the error was generated
        dev: Option<PathBuf>,
    },
    /// The DrmDevice is missing a required property
    #[error("The DrmDevice is missing a required property '{name}' for handle ({handle:?})")]
    UnknownProperty {
        /// Property handle
        handle: RawResourceHandle,
        /// Property name
        name: &'static str,
    },
    /// Atomic Test failed for new properties
    #[error("Atomic Test failed for new properties on crtc ({0:?})")]
    TestFailed(crtc::Handle),
}
