//!
//! Errors thrown by the `DrmDevice` and `DrmBackend`
//!

use backend::graphics::egl;
use drm::control::{connector, crtc, Mode};
use rental::TryNewError;

error_chain! {
    errors {
        #[doc = "Unable to acquire drm master"]
        DrmMasterFailed {
            description("Failed to acquire drm master")
        }

        #[doc = "The `DrmDevice` encountered an access error"]
        DrmDev(dev: String) {
            description("The drm device encountered an access error"),
            display("The drm device ({:?}) encountered an access error", dev),
        }

        #[doc = "Creation of gbm resource failed"]
        GbmInitFailed {
            description("Creation of gbm resource failed"),
            display("Creation of gbm resource failed"),
        }

        #[doc = "Swapping front buffers failed"]
        FailedToSwap {
            description("Swapping front buffers failed"),
            display("Swapping front buffers failed"),
        }

        #[doc = "Device is currently paused"]
        DeviceInactive {
            description("Device is currently paused, operation rejected"),
            display("Device is currently paused, operation rejected"),
        }

        #[doc = "Mode is not compatible with all given connectors"]
        ModeNotSuitable(mode: Mode) {
            description("Mode is not compatible with all given connectors"),
            display("Mode ({:?}) is not compatible with all given connectors", mode),
        }

        #[doc = "The given crtc is already in use by another backend"]
        CrtcAlreadyInUse(crtc: crtc::Handle) {
            description("The given crtc is already in use by another backend"),
            display("The given crtc ({:?}) is already in use by another backend", crtc),
        }

        #[doc = "No encoder was found for a given connector on the set crtc"]
        NoSuitableEncoder(connector: connector::Info, crtc: crtc::Handle) {
            description("No encoder found for given connector on set crtc"),
            display("No encoder found for the given connector '{:?}' on the set crtc ({:?})", connector.connector_type(), crtc),
        }
    }

    links {
        EGL(egl::Error, egl::ErrorKind) #[doc = "EGL error"];
    }
}

impl<H> From<TryNewError<Error, H>> for Error {
    fn from(err: TryNewError<Error, H>) -> Error {
        err.0
    }
}
