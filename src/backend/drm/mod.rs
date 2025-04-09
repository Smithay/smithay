//! This module represents abstraction on top the linux direct rendering manager api (drm).
//!
//! ## [`DrmDevice`]
//!
//! A device exposes certain properties, which are directly derived
//! from the *device* as perceived by the direct rendering manager api (drm). These resources consists
//! out of:
//! - [`connectors`](drm::control::connector) represents a port on your computer, possibly with a connected monitor, TV, capture card, etc.
//! - [`encoder`](drm::control::encoder) encodes the data of connected crtcs into a video signal for a fixed set of connectors.
//!   E.g. you might have an analog encoder based on a DAG for VGA ports, but another one for digital ones.
//!   Also not every encoder might be connected to every crtc.
//! - [`framebuffer`] represents a buffer you may display, see `DrmSurface` below.
//! - [`plane`] adds another layer on top of the crtcs, which allow us to layer multiple images on top of each other more efficiently
//!   then by combining the rendered images in the rendering phase, e.g. via OpenGL. Planes have to be explicitly used by the user to be useful.
//!   Every device has at least one primary plane used to display an image to the whole crtc. Additionally cursor and overlay planes may be present.
//!   Cursor planes are usually very restricted in size and meant to be used for hardware cursors, while overlay planes may
//!   be used for performance reasons to display any overlay on top of the image, e.g. the top-most windows.
//!   Overlay planes may have a bunch of weird limitation, that you cannot query, e.g. only working on round pixel coordinates.
//!   You code should never rely on a fixed set of overlay planes, but always have a fallback solution in place.
//! - [`crtc`]s represent scanout engines of the device pointer to one framebuffer.
//!   Their responsibility is to read the data of the framebuffer and export it into an "Encoder".
//!   The number of crtc's represent the number of independent output devices the hardware may handle.
//!
//!  On modern graphic cards it is better to think about the `crtc` as some sort of rendering engine.
//!  You can only have so many different pictures, you may display, as you have `crtc`s, but a single image
//!  may be put onto multiple displays.
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
//! ## [`DrmSurface`]
//!
//! A surface is a part of a `Device` that may output a picture to a number of connectors. It pumps pictures of buffers to outputs.
//!
//! On surface creation a matching encoder for your `connector` is automatically selected, if one exists(!),
//! which means you still need to check your configuration beforehand, even if you do not need to provide an encoder.
//!
//! A surface consists of one `crtc` that is rendered to by the user. This is fixed for the `Surface`s lifetime and cannot be changed.
//! A surface also always needs at least one connector to output the resulting image to as well as a `Mode` that is valid for the given connectors.
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
//! rendering abstractions, which can target these buffers can be found in the [`renderer`-Module](crate::backend::renderer).
//!
//! ### Hardware composition
//!
//! The [`DrmCompositor`](crate::backend::drm::compositor::DrmCompositor) provides a simplified way to utilize drm planes for
//! using hardware composition.
//! See the [`compositor`] module docs for more information on that topic.
//!
//! ## [`DrmNode`]
//!
//! A drm node refers to a drm device and the capabilities that may be performed using the node.
//! Generally [`DrmNode`] is primarily used by clients (such as the output backends) which need
//! to allocate buffers for use in X11 or Wayland. If you need to do mode setting, you should use
//! [`DrmDevice`] instead.

#[cfg(all(feature = "wayland_frontend", feature = "backend_gbm"))]
pub mod compositor;
pub(crate) mod device;
#[cfg(feature = "backend_drm")]
pub mod dumb;
mod error;
pub mod exporter;
#[cfg(feature = "backend_gbm")]
pub mod gbm;
#[cfg(all(feature = "wayland_frontend", feature = "backend_gbm"))]
pub mod output;

mod surface;

use std::sync::Once;

use crate::utils::{DevPath, Physical, Size};
pub use device::{
    DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, EventMetadata as DrmEventMetadata, PlaneClaim,
    Time as DrmEventTime,
};
pub use drm::node::{CreateDrmNodeError, DrmNode, NodeType};
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
pub use error::AccessError as DrmAccessError;
pub use error::Error as DrmError;
use indexmap::IndexSet;
#[cfg(feature = "backend_gbm")]
pub use surface::gbm::{Error as GbmBufferedSurfaceError, GbmBufferedSurface};
pub use surface::{DrmSurface, PlaneConfig, PlaneDamageClips, PlaneState, VrrSupport};

use drm::{
    control::{crtc, framebuffer, plane, Device as ControlDevice, PlaneType},
    DriverCapability,
};
use tracing::trace;

use self::error::AccessError;

use super::allocator::format::FormatSet;

fn warn_legacy_fb_export() {
    static WARN_LEGACY_FB_EXPORT: Once = Once::new();
    WARN_LEGACY_FB_EXPORT.call_once(|| {
        tracing::warn!("using legacy fbadd");
    });
}

/// Common framebuffer operations
pub trait Framebuffer: AsRef<framebuffer::Handle> {
    /// Retrieve the format of the framebuffer
    fn format(&self) -> drm_fourcc::DrmFormat;
}

/// A set of planes as supported by a crtc
#[derive(Debug, Clone)]
pub struct Planes {
    /// The primary plane(s) of the crtc
    pub primary: Vec<PlaneInfo>,
    /// The cursor plane(s) of the crtc, if available
    pub cursor: Vec<PlaneInfo>,
    /// Overlay planes supported by the crtc, if available
    pub overlay: Vec<PlaneInfo>,
}

/// Info about a single plane
#[derive(Debug, Clone)]
pub struct PlaneInfo {
    /// Handle of the plane
    pub handle: plane::Handle,
    /// Type of the plane
    pub type_: PlaneType,
    /// z-position of the plane if available
    pub zpos: Option<i32>,
    /// Formats supported by this plane
    pub formats: FormatSet,
    /// Recommended plane size in order of preference
    pub size_hints: Option<Vec<Size<u16, Physical>>>,
}

fn planes(
    dev: &(impl DevPath + ControlDevice),
    crtc: &crtc::Handle,
    has_universal_planes: bool,
) -> Result<Planes, DrmError> {
    let mut primary = Vec::with_capacity(1);
    let mut cursor = Vec::new();
    let mut overlay = Vec::new();

    let planes = dev.plane_handles().map_err(|source| {
        DrmError::Access(AccessError {
            errmsg: "Error loading plane handles",
            dev: dev.dev_path(),
            source,
        })
    })?;

    let resources = dev.resource_handles().map_err(|source| {
        DrmError::Access(AccessError {
            errmsg: "Error loading resource handles",
            dev: dev.dev_path(),
            source,
        })
    })?;

    for plane in planes {
        let info = dev.get_plane(plane).map_err(|source| {
            DrmError::Access(AccessError {
                errmsg: "Failed to get plane information",
                dev: dev.dev_path(),
                source,
            })
        })?;
        let filter = info.possible_crtcs();
        if resources.filter_crtcs(filter).contains(crtc) {
            let zpos = plane_zpos(dev, plane).ok().flatten();
            let type_ = plane_type(dev, plane)?;
            let formats = plane_formats(dev, plane)?;
            let size_hints = plane_size_hints(dev, plane)?;
            let plane_info = PlaneInfo {
                handle: plane,
                type_,
                zpos,
                formats,
                size_hints,
            };
            match type_ {
                PlaneType::Primary => {
                    primary.push(plane_info);
                }
                PlaneType::Cursor => {
                    cursor.push(plane_info);
                }
                PlaneType::Overlay => {
                    overlay.push(plane_info);
                }
            };
        }
    }

    Ok(Planes {
        primary,
        cursor: if has_universal_planes { cursor } else { Vec::new() },
        overlay: if has_universal_planes { overlay } else { Vec::new() },
    })
}

fn plane_type(dev: &(impl ControlDevice + DevPath), plane: plane::Handle) -> Result<PlaneType, DrmError> {
    let props = dev.get_properties(plane).map_err(|source| {
        DrmError::Access(AccessError {
            errmsg: "Failed to get properties of plane",
            dev: dev.dev_path(),
            source,
        })
    })?;
    let (ids, vals) = props.as_props_and_values();
    for (&id, &val) in ids.iter().zip(vals.iter()) {
        let info = dev.get_property(id).map_err(|source| {
            DrmError::Access(AccessError {
                errmsg: "Failed to get property info",
                dev: dev.dev_path(),
                source,
            })
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

fn plane_zpos(dev: &(impl ControlDevice + DevPath), plane: plane::Handle) -> Result<Option<i32>, DrmError> {
    let props = dev.get_properties(plane).map_err(|source| {
        DrmError::Access(AccessError {
            errmsg: "Failed to get properties of plane",
            dev: dev.dev_path(),
            source,
        })
    })?;
    let (ids, vals) = props.as_props_and_values();
    for (&id, &val) in ids.iter().zip(vals.iter()) {
        let info = dev.get_property(id).map_err(|source| {
            DrmError::Access(AccessError {
                errmsg: "Failed to get property info",
                dev: dev.dev_path(),
                source,
            })
        })?;
        if info.name().to_str().map(|x| x == "zpos").unwrap_or(false) {
            let plane_zpos = match info.value_type().convert_value(val) {
                drm::control::property::Value::UnsignedRange(u) => Some(u as i32),
                drm::control::property::Value::SignedRange(i) => Some(i as i32),
                // A range from [0,1] will be interpreted as Boolean in drm-rs
                // TODO: Once that has been changed we can remove this special handling here
                drm::control::property::Value::Boolean(b) => Some(b.into()),
                _ => None,
            };
            return Ok(plane_zpos);
        }
    }
    Ok(None)
}

fn plane_formats(dev: &(impl ControlDevice + DevPath), plane: plane::Handle) -> Result<FormatSet, DrmError> {
    // get plane formats
    let plane_info = dev.get_plane(plane).map_err(|source| {
        DrmError::Access(AccessError {
            errmsg: "Error loading plane info",
            dev: dev.dev_path(),
            source,
        })
    })?;
    let mut formats = IndexSet::new();
    for code in plane_info
        .formats()
        .iter()
        .flat_map(|x| DrmFourcc::try_from(*x).ok())
    {
        formats.insert(DrmFormat {
            code,
            modifier: DrmModifier::Invalid,
        });
    }

    if let Ok(1) = dev.get_driver_capability(DriverCapability::AddFB2Modifiers) {
        let set = dev.get_properties(plane).map_err(|source| {
            DrmError::Access(AccessError {
                errmsg: "Failed to query properties",
                dev: dev.dev_path(),
                source,
            })
        })?;
        let (handles, _) = set.as_props_and_values();
        // for every handle ...
        let prop = handles
            .iter()
            .find(|handle| {
                // get information of that property
                if let Ok(info) = dev.get_property(**handle) {
                    // to find out, if we got the handle of the "IN_FORMATS" property ...
                    if info.name().to_str().map(|x| x == "IN_FORMATS").unwrap_or(false) {
                        // so we can use that to get formats
                        return true;
                    }
                }
                false
            })
            .copied();
        if let Some(prop) = prop {
            let prop_info = dev.get_property(prop).map_err(|source| {
                DrmError::Access(AccessError {
                    errmsg: "Failed to query property",
                    dev: dev.dev_path(),
                    source,
                })
            })?;
            let (handles, raw_values) = set.as_props_and_values();
            let raw_value = raw_values[handles
                .iter()
                .enumerate()
                .find_map(|(i, handle)| if *handle == prop { Some(i) } else { None })
                .unwrap()];
            if let drm::control::property::Value::Blob(blob) = prop_info.value_type().convert_value(raw_value)
            {
                let data = dev.get_property_blob(blob).map_err(|source| {
                    DrmError::Access(AccessError {
                        errmsg: "Failed to query property blob data",
                        dev: dev.dev_path(),
                        source,
                    })
                })?;
                // be careful here, we have no idea about the alignment inside the blob, so always copy using `read_unaligned`,
                // although slice::from_raw_parts would be so much nicer to iterate and to read.
                unsafe {
                    let fmt_mod_blob_ptr = data.as_ptr() as *const drm_ffi::drm_format_modifier_blob;
                    let fmt_mod_blob = &*fmt_mod_blob_ptr;

                    let formats_ptr: *const u32 = fmt_mod_blob_ptr
                        .cast::<u8>()
                        .offset(fmt_mod_blob.formats_offset as isize)
                        as *const _;
                    let modifiers_ptr: *const drm_ffi::drm_format_modifier = fmt_mod_blob_ptr
                        .cast::<u8>()
                        .offset(fmt_mod_blob.modifiers_offset as isize)
                        as *const _;
                    #[allow(clippy::unnecessary_cast)]
                    let formats_ptr = formats_ptr as *const u32;
                    #[allow(clippy::unnecessary_cast)]
                    let modifiers_ptr = modifiers_ptr as *const drm_ffi::drm_format_modifier;

                    for i in 0..fmt_mod_blob.count_modifiers {
                        let mod_info = modifiers_ptr.offset(i as isize).read_unaligned();
                        for j in 0..64 {
                            if mod_info.formats & (1u64 << j) != 0 {
                                let code = DrmFourcc::try_from(
                                    formats_ptr
                                        .offset((j + mod_info.offset) as isize)
                                        .read_unaligned(),
                                )
                                .ok();
                                let modifier = DrmModifier::from(mod_info.modifier);
                                if let Some(code) = code {
                                    formats.insert(DrmFormat { code, modifier });
                                }
                            }
                        }
                    }
                }
            }
        }
    } else if plane_type(dev, plane)? == PlaneType::Cursor {
        // Force a LINEAR layout for the cursor if the driver doesn't support modifiers
        for format in formats.clone() {
            formats.insert(DrmFormat {
                code: format.code,
                modifier: DrmModifier::Linear,
            });
        }
    }

    if formats.is_empty() {
        formats.insert(DrmFormat {
            code: DrmFourcc::Argb8888,
            modifier: DrmModifier::Invalid,
        });
    }

    trace!(
        "Supported scan-out formats for plane ({:?}): {:?}",
        plane,
        formats
    );

    Ok(FormatSet::from_formats(formats))
}

#[cfg(feature = "backend_gbm")]
fn plane_has_property(
    dev: &(impl drm::control::Device + DevPath),
    plane: plane::Handle,
    name: &str,
) -> Result<bool, DrmError> {
    let props = dev.get_properties(plane).map_err(|source| {
        DrmError::Access(AccessError {
            errmsg: "Failed to get properties of plane",
            dev: dev.dev_path(),
            source,
        })
    })?;
    let (ids, _) = props.as_props_and_values();
    for &id in ids {
        let info = dev.get_property(id).map_err(|source| {
            DrmError::Access(AccessError {
                errmsg: "Failed to get property info",
                dev: dev.dev_path(),
                source,
            })
        })?;
        if info.name().to_str().map(|x| x == name).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[repr(C)]
// FIXME: use definition from drm_ffi once available
struct drm_plane_size_hint {
    width: u16,
    height: u16,
}

fn plane_size_hints(
    dev: &(impl ControlDevice + DevPath),
    plane: plane::Handle,
) -> Result<Option<Vec<Size<u16, Physical>>>, DrmError> {
    let props = dev.get_properties(plane).map_err(|source| {
        DrmError::Access(AccessError {
            errmsg: "Failed to get properties of plane",
            dev: dev.dev_path(),
            source,
        })
    })?;
    let (ids, vals) = props.as_props_and_values();
    for (&id, &val) in ids.iter().zip(vals.iter()) {
        let info = dev.get_property(id).map_err(|source| {
            DrmError::Access(AccessError {
                errmsg: "Failed to get property info",
                dev: dev.dev_path(),
                source,
            })
        })?;
        if info.name().to_str().map(|x| x == "SIZE_HINTS").unwrap_or(false) {
            let size_hints = if let drm::control::property::Value::Blob(blob_id) =
                info.value_type().convert_value(val)
            {
                // Note that property value 0 (ie. no blob) is reserved for potential
                // future use. Current userspace is expected to ignore the property
                // if the value is 0
                if blob_id == 0 {
                    return Ok(None);
                }

                let blob_info = drm_ffi::mode::get_property_blob(dev.as_fd(), blob_id as u32, None).map_err(
                    |source| {
                        DrmError::Access(AccessError {
                            errmsg: "Failed to get SIZE_HINTS blob info",
                            dev: dev.dev_path(),
                            source,
                        })
                    },
                )?;

                let mut data = Vec::with_capacity(blob_info.length as usize);
                drm_ffi::mode::get_property_blob(dev.as_fd(), blob_id as u32, Some(&mut data)).map_err(
                    |source| {
                        DrmError::Access(AccessError {
                            errmsg: "Failed to get SIZE_HINTS blob data",
                            dev: dev.dev_path(),
                            source,
                        })
                    },
                )?;

                let num_size_hints = data.len() / std::mem::size_of::<drm_plane_size_hint>();
                let size_hints = unsafe {
                    std::slice::from_raw_parts(data.as_ptr() as *const drm_plane_size_hint, num_size_hints)
                };
                Some(
                    size_hints
                        .iter()
                        .map(|size_hint| Size::from((size_hint.width, size_hint.height)))
                        .collect(),
                )
            } else {
                tracing::debug!(?plane, "SIZE_HINTS property has wrong value type");
                None
            };
            return Ok(size_hints);
        }
    }
    Ok(None)
}
