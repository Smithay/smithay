//!
//! Errors thrown by the [`LegacyDrmDevice`](::backend::drm::legacy::LegacyDrmDevice)
//! and [`LegacyDrmSurface`](::backend::drm::legacy::LegacyDrmSurface).
//!

use drm::control::{connector, crtc, Mode};

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

        #[doc = "Unable to determine device id of drm device"]
        UnableToGetDeviceId {
            description("Unable to determine device id of drm device"),
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
            display("No encoder found for the given connector '{:?}' on the set crtc ({:?})", connector.interface(), crtc),
        }
    }

    foreign_links {
        FailedToSwap(crate::backend::graphics::SwapBuffersError) #[doc = "Swapping front buffers failed"];
    }
}
