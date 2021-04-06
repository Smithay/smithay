//use crate::backend::graphics::SwapBuffersError;
use drm::control::{connector, crtc, plane, Mode, RawResourceHandle};
use std::path::PathBuf;

/// Errors thrown by the [`DrmDevice`](::backend::drm::DrmDevice)
/// and the [`DrmSurface`](::backend::drm::DrmSurface).
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Unable to acquire DRM master
    #[error("Failed to aquire DRM master")]
    DrmMasterFailed,
    /// The `DrmDevice` encountered an access error
    #[error("DRM access error: {errmsg} on device `{dev:?}` ({source:})")]
    Access {
        /// Error message associated to the access error
        errmsg: &'static str,
        /// Device on which the error was generated
        dev: Option<PathBuf>,
        /// Underlying device error
        source: drm::SystemError,
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
    /// This operation would result in a surface without connectors.
    #[error("Surface of crtc `{0:?}` would have no connectors, which is not accepted")]
    SurfaceWithoutConnectors(crtc::Handle),
    #[error("Plane `{1:?}` is not compatible for use with crtc `{0:?}`")]
    PlaneNotCompatible(crtc::Handle, plane::Handle),
    #[error("Non-Primary Planes (provided was `{0:?}`) are not available for use with legacy devices")]
    NonPrimaryPlane(plane::Handle),
    /// No encoder was found for a given connector on the set crtc
    #[error("No encoder found for the given connector '{connector:?}' on crtc `{crtc:?}`")]
    NoSuitableEncoder {
        /// Connector
        connector: connector::Handle,
        /// CRTC
        crtc: crtc::Handle,
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

/*
impl Into<SwapBuffersError> for Error {
    fn into(self) -> SwapBuffersError {
        match self {
            x @ Error::DeviceInactive => SwapBuffersError::TemporaryFailure(Box::new(x)),
            Error::Access {
                errmsg, dev, source, ..
            } if match source.get_ref() {
                drm::SystemError::PermissionDenied => true,
                drm::SystemError::Unknown {
                    errno: nix::errno::Errno::EBUSY,
                } => true,
                drm::SystemError::Unknown {
                    errno: nix::errno::Errno::EINTR,
                } => true,
                _ => false,
            } =>
            {
                SwapBuffersError::TemporaryFailure(Box::new(Error::Access { errmsg, dev, source }))
            }
            x => SwapBuffersError::ContextLost(Box::new(x)),
        }
    }
}
*/