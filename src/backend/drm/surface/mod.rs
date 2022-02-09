#[cfg(feature = "backend_session")]
use std::cell::RefCell;
use std::collections::HashSet;
use std::convert::TryFrom;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;

use drm::control::{connector, crtc, framebuffer, plane, property, Device as ControlDevice, Mode};
use drm::{Device as BasicDevice, DriverCapability};

use nix::libc::dev_t;

pub(super) mod atomic;
#[cfg(feature = "backend_gbm")]
pub(super) mod gbm;
pub(super) mod legacy;
use super::{device::DevPath, error::Error, plane_type, planes, PlaneType, Planes};
use crate::backend::allocator::{Format, Fourcc, Modifier};
use atomic::AtomicDrmSurface;
use legacy::LegacyDrmSurface;

use slog::trace;

/// An open crtc + plane combination that can be used for scan-out
#[derive(Debug)]
pub struct DrmSurface<A: AsRawFd + 'static> {
    // This field is only read when 'backend_session' is enabled
    #[allow(dead_code)]
    pub(super) dev_id: dev_t,
    pub(super) crtc: crtc::Handle,
    pub(super) primary: plane::Handle,
    pub(super) internal: Arc<DrmSurfaceInternal<A>>,
    pub(super) has_universal_planes: bool,
    #[cfg(feature = "backend_session")]
    pub(super) links: RefCell<Vec<crate::utils::signaling::SignalToken>>,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DrmSurfaceInternal<A: AsRawFd + 'static> {
    Atomic(AtomicDrmSurface<A>),
    Legacy(LegacyDrmSurface<A>),
}

impl<A: AsRawFd + 'static> AsRawFd for DrmSurface<A> {
    fn as_raw_fd(&self) -> RawFd {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.fd.as_raw_fd(),
            DrmSurfaceInternal::Legacy(surf) => surf.fd.as_raw_fd(),
        }
    }
}
impl<A: AsRawFd + 'static> BasicDevice for DrmSurface<A> {}
impl<A: AsRawFd + 'static> ControlDevice for DrmSurface<A> {}

impl<A: AsRawFd + 'static> DrmSurface<A> {
    /// Returns the underlying [`crtc`](drm::control::crtc) of this surface
    pub fn crtc(&self) -> crtc::Handle {
        self.crtc
    }

    /// Returns the underlying primary [`plane`](drm::control::plane) of this surface
    pub fn plane(&self) -> plane::Handle {
        self.primary
    }

    /// Currently used [`connector`](drm::control::connector)s of this surface
    pub fn current_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.current_connectors(),
            DrmSurfaceInternal::Legacy(surf) => surf.current_connectors(),
        }
    }

    /// Returns the pending [`connector`](drm::control::connector)s
    /// used after the next [`commit`](DrmSurface::commit) of this surface
    pub fn pending_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.pending_connectors(),
            DrmSurfaceInternal::Legacy(surf) => surf.pending_connectors(),
        }
    }

    /// Tries to add a new [`connector`](drm::control::connector)
    /// to be used after the next commit.
    ///
    /// **Warning**: You need to make sure, that the connector is not used with another surface
    /// or was properly removed via `remove_connector` + `commit` before adding it to another surface.
    /// Behavior if failing to do so is undefined, but might result in rendering errors or the connector
    /// getting removed from the other surface without updating it's internal state.
    ///
    /// Fails if the `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).
    pub fn add_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.add_connector(connector),
            DrmSurfaceInternal::Legacy(surf) => surf.add_connector(connector),
        }
    }

    /// Tries to mark a [`connector`](drm::control::connector)
    /// for removal on the next commit.
    pub fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.remove_connector(connector),
            DrmSurfaceInternal::Legacy(surf) => surf.remove_connector(connector),
        }
    }

    /// Tries to replace the current connector set with the newly provided one on the next commit.
    ///
    /// Fails if one new `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).
    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.set_connectors(connectors),
            DrmSurfaceInternal::Legacy(surf) => surf.set_connectors(connectors),
        }
    }

    /// Returns the currently active [`Mode`](drm::control::Mode)
    /// of the underlying [`crtc`](drm::control::crtc)
    pub fn current_mode(&self) -> Mode {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.current_mode(),
            DrmSurfaceInternal::Legacy(surf) => surf.current_mode(),
        }
    }

    /// Returns the currently pending [`Mode`](drm::control::Mode)
    /// to be used after the next commit.
    pub fn pending_mode(&self) -> Mode {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.pending_mode(),
            DrmSurfaceInternal::Legacy(surf) => surf.pending_mode(),
        }
    }

    /// Tries to set a new [`Mode`](drm::control::Mode)
    /// to be used after the next commit.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`](drm::control::crtc) or any of the
    /// pending [`connector`](drm::control::connector)s.
    pub fn use_mode(&self, mode: Mode) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.use_mode(mode),
            DrmSurfaceInternal::Legacy(surf) => surf.use_mode(mode),
        }
    }

    /// Tries to setup a cursor or overlay [`Plane`](drm::control::plane)
    /// to be set at the next commit/page_flip with the given position and size.
    ///
    /// Planes can have arbitrary hardware constraints, that cannot be expressed in the api,
    /// like supporting only positions at even or odd values, allowing only certain sizes or disallowing overlapping planes.
    /// Using planes should therefor be done in a best-efford manner. Failures on `page_flip` or `commit`
    /// should be expected and alternative code paths without the usage of planes prepared.
    ///
    /// Fails if tests for the given plane fail, if the underlying
    /// implementation does not support the use of planes or if the plane
    /// is not supported by this crtc.
    pub fn use_plane(
        &self,
        plane: plane::Handle,
        position: (i32, i32),
        size: (u32, u32),
    ) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.use_plane(plane, position, size),
            DrmSurfaceInternal::Legacy(_) => Err(Error::NonPrimaryPlane(plane)),
        }
    }

    /// Disables the given plane.
    ///
    /// Errors if the plane is not supported by this crtc or if the underlying
    /// implementation does not support the use of planes.
    pub fn clear_plane(&self, plane: plane::Handle) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.clear_plane(plane),
            DrmSurfaceInternal::Legacy(_) => Err(Error::NonPrimaryPlane(plane)),
        }
    }

    /// Returns true whenever any state changes are pending to be commited
    ///
    /// The following functions may trigger a pending commit:
    /// - [`add_connector`](DrmSurface::add_connector)
    /// - [`remove_connector`](DrmSurface::remove_connector)
    /// - [`use_mode`](DrmSurface::use_mode)
    pub fn commit_pending(&self) -> bool {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.commit_pending(),
            DrmSurfaceInternal::Legacy(surf) => surf.commit_pending(),
        }
    }

    /// Commit the pending state rendering a given framebuffer.
    ///
    /// *Note*: This will trigger a full modeset on the underlying device,
    /// potentially causing some flickering. Check before performing this
    /// operation if a commit really is necessary using [`commit_pending`](DrmSurface::commit_pending).
    ///
    /// This operation is not necessarily blocking until the crtc is in the desired state,
    /// but will trigger a `vblank` event once done.
    /// Make sure to have the device registered in your event loop prior to invoking this, to not miss
    /// any generated event.
    pub fn commit<'a>(
        &self,
        mut framebuffers: impl Iterator<Item = &'a (framebuffer::Handle, plane::Handle)>,
        event: bool,
    ) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.commit(framebuffers, event),
            DrmSurfaceInternal::Legacy(surf) => {
                if let Some((fb, plane)) = framebuffers.next() {
                    if plane_type(self, *plane)? != PlaneType::Primary {
                        return Err(Error::NonPrimaryPlane(*plane));
                    }
                    surf.commit(*fb, event)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Page-flip the underlying [`crtc`](drm::control::crtc)
    /// to a new given [`framebuffer`].
    ///
    /// This will not cause the crtc to modeset.
    ///
    /// This operation is not blocking and will produce a `vblank` event once swapping is done.
    /// Make sure to have the device registered in your event loop to not miss the event.
    pub fn page_flip<'a>(
        &self,
        mut framebuffers: impl Iterator<Item = &'a (framebuffer::Handle, plane::Handle)>,
        event: bool,
    ) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.page_flip(framebuffers, event),
            DrmSurfaceInternal::Legacy(surf) => {
                if let Some((fb, plane)) = framebuffers.next() {
                    if plane_type(self, *plane)? != PlaneType::Primary {
                        return Err(Error::NonPrimaryPlane(*plane));
                    }
                    surf.page_flip(*fb, event)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Returns a set of supported pixel formats for attached buffers
    pub fn supported_formats(&self, plane: plane::Handle) -> Result<HashSet<Format>, Error> {
        // get plane formats
        let plane_info = self.get_plane(plane).map_err(|source| Error::Access {
            errmsg: "Error loading plane info",
            dev: self.dev_path(),
            source,
        })?;
        let mut formats = HashSet::new();
        for code in plane_info
            .formats()
            .iter()
            .flat_map(|x| Fourcc::try_from(*x).ok())
        {
            formats.insert(Format {
                code,
                modifier: Modifier::Invalid,
            });
        }

        if let Ok(1) = self.get_driver_capability(DriverCapability::AddFB2Modifiers) {
            let set = self.get_properties(plane).map_err(|source| Error::Access {
                errmsg: "Failed to query properties",
                dev: self.dev_path(),
                source,
            })?;
            let (handles, _) = set.as_props_and_values();
            // for every handle ...
            let prop = handles
                .iter()
                .find(|handle| {
                    // get information of that property
                    if let Ok(info) = self.get_property(**handle) {
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
                let prop_info = self.get_property(prop).map_err(|source| Error::Access {
                    errmsg: "Failed to query property",
                    dev: self.dev_path(),
                    source,
                })?;
                let (handles, raw_values) = set.as_props_and_values();
                let raw_value = raw_values[handles
                    .iter()
                    .enumerate()
                    .find_map(|(i, handle)| if *handle == prop { Some(i) } else { None })
                    .unwrap()];
                if let property::Value::Blob(blob) = prop_info.value_type().convert_value(raw_value) {
                    let data = self.get_property_blob(blob).map_err(|source| Error::Access {
                        errmsg: "Failed to query property blob data",
                        dev: self.dev_path(),
                        source,
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
                        let formats_ptr = formats_ptr as *const u32;
                        let modifiers_ptr = modifiers_ptr as *const drm_ffi::drm_format_modifier;

                        for i in 0..fmt_mod_blob.count_modifiers {
                            let mod_info = modifiers_ptr.offset(i as isize).read_unaligned();
                            for j in 0..64 {
                                if mod_info.formats & (1u64 << j) != 0 {
                                    let code = Fourcc::try_from(
                                        formats_ptr
                                            .offset((j + mod_info.offset) as isize)
                                            .read_unaligned(),
                                    )
                                    .ok();
                                    let modifier = Modifier::from(mod_info.modifier);
                                    if let Some(code) = code {
                                        formats.insert(Format { code, modifier });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if plane_type(self, plane)? == PlaneType::Cursor {
            // Force a LINEAR layout for the cursor if the driver doesn't support modifiers
            for format in formats.clone() {
                formats.insert(Format {
                    code: format.code,
                    modifier: Modifier::Linear,
                });
            }
        }

        if formats.is_empty() {
            formats.insert(Format {
                code: Fourcc::Argb8888,
                modifier: Modifier::Invalid,
            });
        }

        let logger = match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => &surf.logger,
            DrmSurfaceInternal::Legacy(surf) => &surf.logger,
        };
        trace!(
            logger,
            "Supported scan-out formats for plane ({:?}): {:?}",
            plane,
            formats
        );

        Ok(formats)
    }

    /// Returns a set of available planes for this surface
    pub fn planes(&self) -> Result<Planes, Error> {
        planes(self, &self.crtc, self.has_universal_planes)
    }

    /// Tests if a framebuffer can be used with this surface.
    ///
    /// # Arguments
    ///
    /// - `fb` - Framebuffer handle that has an attached buffer, that shall be tested
    /// - `mode` - The mode that should be used to display the buffer
    /// - `allow_screen_change` - If an actual screen change is permitted to carry out this test.
    ///    If the test cannot be performed otherwise, this function returns false.
    pub fn test_buffer(
        &self,
        fb: framebuffer::Handle,
        mode: &Mode,
        allow_screen_change: bool,
    ) -> Result<bool, Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.test_buffer(fb, mode),
            DrmSurfaceInternal::Legacy(surf) => {
                if allow_screen_change {
                    surf.test_buffer(fb, mode)
                } else {
                    Ok(false)
                }
            } // There is no test-commiting with the legacy interface
        }
    }

    /// Tests if a framebuffer can be used with this surface and a given plane.
    ///
    /// # Arguments
    ///
    /// - `fb` - Framebuffer handle that has an attached buffer, that shall be tested
    /// - `plane` - The plane that should be used to display the buffer
    ///     (only works for *cursor* and *overlay* planes - for primary planes use `test_buffer`)
    /// - `position` - The position of the plane
    /// - `size` - The size of the plane
    ///
    /// If the test cannot be performed, this function returns false.
    /// This is always the case for non-atomic surfaces.
    pub fn test_plane_buffer(
        &self,
        fb: framebuffer::Handle,
        plane: plane::Handle,
        position: (i32, i32),
        size: (u32, u32),
    ) -> Result<bool, Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.test_plane_buffer(fb, plane, position, size),
            DrmSurfaceInternal::Legacy(_) => Ok(false), // There is no test-commiting with the legacy interface
        }
    }

    /// Re-evaluates the current state of the crtc.
    ///
    /// Usually you do not need to call this, but if the state of
    /// the crtc is modified elsewhere and you need to reset the
    /// initial state of this surface, you may call this function.
    pub fn reset_state(&self) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.reset_state::<Self>(None),
            DrmSurfaceInternal::Legacy(surf) => surf.reset_state::<Self>(None),
        }
    }
}
