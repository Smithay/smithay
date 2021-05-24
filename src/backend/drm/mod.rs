//! This module represents abstraction on top the linux direct rendering manager api (drm).
//!
//! ## DrmDevice
//!
//! A device  exposes certain properties, which are directly derived
//! from the *device* as perceived by the direct rendering manager api (drm). These resources consists
//! out of connectors, encoders, framebuffers, planes and crtcs.
//!
//! [`crtc`](drm::control::crtc)s represent scanout engines of the device pointer to one framebuffer.
//! Their responsibility is to read the data of the framebuffer and export it into an "Encoder".
//! The number of crtc's represent the number of independent output devices the hardware may handle.
//!
//! On modern graphic cards it is better to think about the `crtc` as some sort of rendering engine.
//! You can only have so many different pictures, you may display, as you have `crtc`s, but a single image
//! may be put onto multiple displays.
//!
//! An [`encoder`](drm::control::encoder) encodes the data of connected crtcs into a video signal for a fixed set of connectors.
//! E.g. you might have an analog encoder based on a DAG for VGA ports, but another one for digital ones.
//! Also not every encoder might be connected to every crtc.
//!
//! A [`connector`](drm::control::connector) represents a port on your computer, possibly with a connected monitor, TV, capture card, etc.
//!
//! A [`framebuffer`](drm::control::framebuffer) represents a buffer you may be rendering to, see `Surface` below.
//!
//! A [`plane`](drm::control::plane) adds another layer on top of the crtcs, which allow us to layer multiple images on top of each other more efficiently
//! then by combining the rendered images in the rendering phase, e.g. via OpenGL. Planes can be explicitly used by the user.
//! Every device has at least one primary plane used to display an image to the whole crtc. Additionally cursor and overlay planes may be present.
//! Cursor planes are usually very restricted in size and meant to be used for hardware cursors, while overlay planes may
//! be used for performance reasons to display any overlay on top of the image, e.g. top-most windows.
//!
//! The main functionality of a `Device` in smithay is to give access to all these properties for the user to
//! choose an appropriate rendering configuration. What that means is defined by the requirements and constraints documented
//! in the specific device implementations. The second functionality is the creation of a `Surface`.
//! Surface creation requires a `crtc` (which cannot be the same as another existing `Surface`'s crtc), a plane,
//! as well as a `Mode` and a set of `connectors`.
//!
//! smithay does not make sure that `connectors` are not already in use by another `Surface`. Overlapping `connector`-Sets may
//! be an error or result in undefined rendering behavior depending on the `Surface` implementation.
//!
//! ## DrmSurface
//!
//! A surface is a part of a `Device` that may output a picture to a number of connectors. It pumps pictures of buffers to outputs.
//!
//! On surface creation a matching encoder for your `encoder`-`connector` is automatically selected,
//! if it exists, which means you still need to check your configuration.
//!
//! A surface consists of one `crtc` that is rendered to by the user. This is fixed for the `Surface`s lifetime and cannot be changed.
//! A surface also always needs at least one connector to output the resulting image to as well as a `Mode` that is valid for the given connector.
//!
//! The state of a `Surface` is double-buffered, meaning all operations that chance the set of `connector`s or their `Mode` are stored and
//! only applied on the next commit. `Surface`s do their best to validate these changes, if possible.
//!
//! A commit/page_flip may be triggered to apply the pending state.
//!
//! ## Rendering
//!
//! The drm infrastructure makes no assumptions about the used renderer and does not interface with them directly.
//! It just provides a way to create framebuffers from various buffer types (mainly `DumbBuffer`s and hardware-backed gbm `BufferObject`s).
//!
//! Buffer management and details about the various types can be found in the [`allocator`-Module](crate::backend::allocator) and
//! renderering abstractions, which can target these buffers can be found in the [`renderer`-Module](crate::backend::renderer).

pub(crate) mod device;
pub(self) mod error;
#[cfg(feature = "backend_gbm")]
mod render;
#[cfg(feature = "backend_session")]
pub(self) mod session;
pub(self) mod surface;

pub use device::{device_bind, DevPath, DeviceHandler, DrmDevice, DrmSource};
pub use error::Error as DrmError;
#[cfg(feature = "backend_gbm")]
pub use render::{DrmRenderSurface, Error as DrmRenderError};
pub use surface::DrmSurface;

use drm::control::{plane, crtc, Device as ControlDevice, PlaneType};

/// A set of planes as supported by a crtc
pub struct Planes {
    /// The primary plane of the crtc (automatically selected for [DrmDevice::create_surface])
    pub primary: plane::Handle,
    /// The cursor plane of the crtc, if available
    pub cursor: Option<plane::Handle>,
    /// Overlay planes supported by the crtc, if available
    pub overlay: Vec<plane::Handle>,
}

fn planes(dev: &impl ControlDevice, crtc: &crtc::Handle, has_universal_planes: bool) -> Result<Planes, DrmError> {
    let mut primary = None;
    let mut cursor = None;
    let mut overlay = Vec::new();
    
    let planes = dev.plane_handles().map_err(|source| DrmError::Access {
        errmsg: "Error loading plane handles",
        dev: dev.dev_path(),
        source,
    })?;

    let resources = dev.resource_handles().map_err(|source| DrmError::Access {
        errmsg: "Error loading resource handles",
        dev: dev.dev_path(),
        source,
    })?;       

    for plane in planes.planes() {
        let info = dev.get_plane(*plane).map_err(|source| DrmError::Access {
            errmsg: "Failed to get plane information",
            dev: dev.dev_path(),
            source,
        })?;
        let filter = info.possible_crtcs();
        if resources.filter_crtcs(filter).contains(crtc) {
            match plane_type(dev, *plane)? {
                PlaneType::Primary => {
                    primary = Some(*plane);
                }
                PlaneType::Cursor => {
                    cursor = Some(*plane);
                }
                PlaneType::Overlay => {
                    overlay.push(*plane);
                }
            };
        }
    }

    Ok(Planes {
        primary: primary.expect("Crtc has no primary plane"),
        cursor: if has_universal_planes {
            cursor
        } else {
            None
        },
        overlay: if has_universal_planes {
            overlay
        } else {
            Vec::new()
        },
    })
}

fn plane_type(dev: &impl ControlDevice, plane: plane::Handle) -> Result<PlaneType, DrmError> {
    let props = dev.get_properties(plane).map_err(|source| DrmError::Access {
        errmsg: "Failed to get properties of plane",
        dev: dev.dev_path(),
        source,
    })?;
    let (ids, vals) = props.as_props_and_values();
    for (&id, &val) in ids.iter().zip(vals.iter()) {
        let info = dev.get_property(id).map_err(|source| DrmError::Access {
            errmsg: "Failed to get property info",
            dev: dev.dev_path(),
            source,
        })?;
        if info.name().to_str().map(|x| x == "type").unwrap_or(false) {
            return Ok(match val {
                x if x == (PlaneType::Primary as u64) => PlaneType::Primary,
                x if x == (PlaneType::Cursor as u64) => PlaneType::Cursor,
                _ => PlaneType::Overlay,
            });
        }
    }
    unreachable!()
}