use crate::backend::SwapBuffersError;
use drm::control::{connector, crtc, plane, Mode, RawResourceHandle};
use std::{
    io::{self, ErrorKind},
    path::PathBuf,
};

/// DRM access error
#[derive(Debug, thiserror::Error)]
#[error("DRM access error: {errmsg} on device `{dev:?}` ({source:})")]
pub struct AccessError {
    /// Error message associated to the access error
    pub(crate) errmsg: &'static str,
    /// Device on which the error was generated
    pub(crate) dev: Option<PathBuf>,
    /// Underlying device error
    #[source]
    pub source: io::Error,
}

/// Errors thrown by the [`DrmDevice`](crate::backend::drm::DrmDevice)
/// and the [`DrmSurface`](crate::backend::drm::DrmSurface).
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Unable to acquire DRM master
    #[error("Failed to aquire DRM master")]
    DrmMasterFailed,
    /// The `DrmDevice` encountered an access error
    #[error(transparent)]
    Access(#[from] AccessError),
    /// Unable to determine device id of drm device
    #[error("Unable to determine device id of drm device")]
    UnableToGetDeviceId(#[source] rustix::io::Errno),
    /// Device is currently paused
    #[error("Device is currently paused, operation rejected")]
    DeviceInactive,
    /// Mode is not compatible with all given connectors
    #[error("Mode `{0:?}` is not compatible with all given connectors")]
    ModeNotSuitable(Mode),
    /// The given crtc is already in use by another surface
    #[error("Crtc `{0:?}` is already in use by another surface")]
    CrtcAlreadyInUse(crtc::Handle),
    /// This operation would result in a surface without connectors.
    #[error("Surface of crtc `{0:?}` would have no connectors, which is not accepted")]
    SurfaceWithoutConnectors(crtc::Handle),
    /// The given plane cannot be used with the given crtc
    #[error("Plane `{1:?}` is not compatible for use with crtc `{0:?}`")]
    PlaneNotCompatible(crtc::Handle, plane::Handle),
    /// The given configuration does not specify a plane which is not supported by the underlying implementation
    #[error("No Plane has been specified which is not supported by the underlying implementation")]
    NoPlane,
    /// The given plane is not a primary plane and therefor not supported by the underlying implementation
    #[error("Non-Primary Planes (provided was `{0:?}`) are not available for use with legacy devices")]
    NonPrimaryPlane(plane::Handle),
    /// The given plane does not allow to clear the framebuffer
    #[error("Clearing the framebuffer on plane `{0:?}` is not supported")]
    NoFramebuffer(plane::Handle),
    /// The configuration is not supported on the given plane
    #[error("The configuration is not supported on plane `{0:?}`")]
    UnsupportedPlaneConfiguration(plane::Handle),
    /// No encoder was found for a given connector on the set crtc
    #[error("No encoder found for the given connector '{connector:?}' on crtc `{crtc:?}`")]
    NoSuitableEncoder {
        /// Connector
        connector: connector::Handle,
        /// CRTC
        crtc: crtc::Handle,
    },
    /// Unknown connector handle
    #[error("The connector ({0:?}) is unknown")]
    UnknownConnector(connector::Handle),
    /// Unknown crtc handle
    #[error("The crtc ({0:?}) is unknown")]
    UnknownCrtc(crtc::Handle),
    /// Unknown crtc handle
    #[error("The plane ({0:?}) is unknown")]
    UnknownPlane(plane::Handle),
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

impl From<Error> for SwapBuffersError {
    #[inline]
    fn from(err: Error) -> SwapBuffersError {
        // FIXME: replace the special handling for EBUSY with ErrorKind::ResourceBusy once
        // we reach MSRV >= 1.83
        match err {
            x @ Error::DeviceInactive => SwapBuffersError::TemporaryFailure(Box::new(x)),
            Error::Access(AccessError {
                errmsg, dev, source, ..
            }) if matches!(
                source.kind(),
                ErrorKind::PermissionDenied | ErrorKind::WouldBlock | ErrorKind::Interrupted
            ) || rustix::io::Errno::from_io_error(&source) == Some(rustix::io::Errno::BUSY) =>
            {
                SwapBuffersError::TemporaryFailure(Box::new(Error::Access(AccessError {
                    errmsg,
                    dev,
                    source,
                })))
            }
            x => SwapBuffersError::ContextLost(Box::new(x)),
        }
    }
}
