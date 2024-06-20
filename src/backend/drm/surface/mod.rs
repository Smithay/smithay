use std::io;
use std::os::unix::io::{AsFd, BorrowedFd};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use drm::control::{connector, crtc, framebuffer, plane, Device as ControlDevice, Mode, PageFlipFlags};
use drm::Device as BasicDevice;

use libc::dev_t;

pub(super) mod atomic;
#[cfg(feature = "backend_gbm")]
pub(super) mod gbm;
pub(super) mod legacy;
use super::{
    device::PlaneClaimStorage, error::Error, plane_type, DrmDeviceFd, PlaneClaim, PlaneType, Planes,
};
use crate::utils::DevPath;
use crate::utils::{Buffer, Physical, Point, Rectangle, Transform};
use atomic::AtomicDrmSurface;
use legacy::LegacyDrmSurface;

/// An open crtc + plane combination that can be used for scan-out
#[derive(Debug)]
pub struct DrmSurface {
    // This field is only read when 'backend_session' is enabled
    #[allow(dead_code)]
    pub(super) dev_id: dev_t,
    pub(super) crtc: crtc::Handle,
    pub(super) planes: Planes,
    pub(super) internal: Arc<DrmSurfaceInternal>,
    pub(super) plane_claim_storage: PlaneClaimStorage,
}

#[derive(Debug)]
struct PlaneDamageInner {
    drm: DrmDeviceFd,
    blob: Option<drm::control::property::Value<'static>>,
}

impl Drop for PlaneDamageInner {
    fn drop(&mut self) {
        // There is nothing we can do if that fails
        if let Some(drm::control::property::Value::Blob(id)) = self.blob.take() {
            let _ = self.drm.destroy_property_blob(id);
        }
    }
}

#[derive(Debug)]
/// Helper for `FB_DAMAGE_CLIPS`
pub struct PlaneDamageClips {
    inner: Arc<PlaneDamageInner>,
}

impl PlaneDamageClips {
    /// Returns the underlying blob
    pub fn blob(&self) -> drm::control::property::Value<'_> {
        self.inner.blob.unwrap()
    }
}

impl PlaneDamageClips {
    /// Initialize damage clips for a a plane
    #[profiling::function]
    pub fn from_damage(
        device: &DrmDeviceFd,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: impl IntoIterator<Item = Rectangle<i32, Physical>>,
    ) -> io::Result<Option<Self>> {
        let scale = src.size / dst.size.to_logical(1).to_buffer(1, Transform::Normal).to_f64();

        let mut rects = damage
            .into_iter()
            .map(|rect| {
                let mut rect = rect
                    .to_f64()
                    .to_logical(1f64)
                    .to_buffer(
                        1f64,
                        Transform::Normal,
                        &src.size.to_logical(1f64, Transform::Normal),
                    )
                    .upscale(scale);
                rect.loc += src.loc;
                let rect = rect.to_i32_up();

                drm_ffi::drm_mode_rect {
                    x1: rect.loc.x,
                    y1: rect.loc.y,
                    x2: rect.loc.x.saturating_add(rect.size.w),
                    y2: rect.loc.y.saturating_add(rect.size.h),
                }
            })
            .collect::<Vec<_>>();

        if rects.is_empty() {
            return Ok(None);
        }

        let data = unsafe {
            std::slice::from_raw_parts_mut(
                rects.as_mut_ptr() as *mut u8,
                std::mem::size_of::<drm_ffi::drm_mode_rect>() * rects.len(),
            )
        };

        let blob = drm_ffi::mode::create_property_blob(device.as_fd(), data)?;

        Ok(Some(PlaneDamageClips {
            inner: Arc::new(PlaneDamageInner {
                drm: device.clone(),
                blob: Some(drm::control::property::Value::Blob(blob.blob_id as u64)),
            }),
        }))
    }
}

impl Clone for PlaneDamageClips {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// State of a single plane
#[derive(Debug)]
pub struct PlaneState<'a> {
    /// Handle of the plane
    pub handle: plane::Handle,
    /// Configuration for that plane
    ///
    /// Can be `None` if nothing is attached
    pub config: Option<PlaneConfig<'a>>,
}

/// Configuration for a single plane
#[derive(Debug)]
pub struct PlaneConfig<'a> {
    /// Source [`Rectangle`] of the attached framebuffer
    pub src: Rectangle<f64, Buffer>,
    /// Destination [`Rectangle`] on the CRTC
    pub dst: Rectangle<i32, Physical>,
    /// Transform for the attached framebuffer
    pub transform: Transform,
    /// Alpha value for the plane
    pub alpha: f32,
    /// Damage clips of the attached framebuffer
    pub damage_clips: Option<drm::control::property::Value<'a>>,
    /// Framebuffer handle
    pub fb: framebuffer::Handle,
    /// Optional fence
    pub fence: Option<BorrowedFd<'a>>,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DrmSurfaceInternal {
    Atomic(AtomicDrmSurface),
    Legacy(LegacyDrmSurface),
}

impl AsFd for DrmSurface {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.fd.as_fd(),
            DrmSurfaceInternal::Legacy(surf) => surf.fd.as_fd(),
        }
    }
}
impl BasicDevice for DrmSurface {}
impl ControlDevice for DrmSurface {}

impl DrmSurface {
    /// Returns the underlying [`DrmDeviceFd`]
    pub fn device_fd(&self) -> &DrmDeviceFd {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.device_fd(),
            DrmSurfaceInternal::Legacy(surf) => surf.device_fd(),
        }
    }

    /// Returns whether this surface is using the legacy api
    pub fn is_legacy(&self) -> bool {
        matches!(&*self.internal, DrmSurfaceInternal::Legacy(_))
    }

    /// Returns the underlying [`crtc`](drm::control::crtc) of this surface
    pub fn crtc(&self) -> crtc::Handle {
        self.crtc
    }

    /// Returns the underlying primary [`plane`](drm::control::plane) of this surface
    pub fn plane(&self) -> plane::Handle {
        self.planes.primary.handle
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

    /// Test a state given a set of framebuffers.
    ///
    /// *Note*: This will always return `Ok` for legacy devices if `allow_modeset = false`.
    /// The legacy drm api has no way to test a buffer without triggering a modeset.
    #[profiling::function]
    pub fn test_state<'a>(
        &self,
        planes: impl IntoIterator<Item = PlaneState<'a>>,
        allow_modeset: bool,
    ) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.test_state(planes, allow_modeset),
            DrmSurfaceInternal::Legacy(surf) => {
                let fb = ensure_legacy_planes(self, planes)?;

                if allow_modeset {
                    surf.test_buffer(fb, &self.pending_mode())
                } else {
                    // Legacy can not test a buffer without triggering a modeset, so we can
                    // only assume it works and hope for the best. A later call to commit or
                    // page_flip will show the correct result
                    Ok(())
                }
            }
        }
    }

    /// Commit the pending state rendering a given set of framebuffers.
    ///
    /// *Note*: This will trigger a full modeset on the underlying device,
    /// potentially causing some flickering. Check before performing this
    /// operation if a commit really is necessary using [`commit_pending`](DrmSurface::commit_pending).
    ///
    /// This operation is not necessarily blocking until the crtc is in the desired state,
    /// but will trigger a `vblank` event once done.
    /// Make sure to have the device registered in your event loop prior to invoking this, to not miss
    /// any generated event.
    #[profiling::function]
    pub fn commit<'a>(
        &self,
        planes: impl IntoIterator<Item = PlaneState<'a>>,
        flip_flags: PageFlipFlags,
    ) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.commit(planes, flip_flags),
            DrmSurfaceInternal::Legacy(surf) => {
                let fb = ensure_legacy_planes(self, planes)?;
                surf.commit(fb, flip_flags)
            }
        }
    }

    /// Page-flip the underlying [`crtc`](drm::control::crtc)
    /// to a new given set of [`framebuffer`]s.
    ///
    /// This will not cause the crtc to modeset.
    ///
    /// This operation is not blocking and will produce a `vblank` event once swapping is done.
    /// Make sure to have the device registered in your event loop to not miss the event.
    #[profiling::function]
    pub fn page_flip<'a>(
        &self,
        planes: impl IntoIterator<Item = PlaneState<'a>>,
        flip_flags: PageFlipFlags,
    ) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.page_flip(planes, flip_flags),
            DrmSurfaceInternal::Legacy(surf) => {
                let fb = ensure_legacy_planes(self, planes)?;
                surf.page_flip(fb, flip_flags)
            }
        }
    }

    /// Returns a set of available planes for this surface
    pub fn planes(&self) -> &Planes {
        &self.planes
    }

    /// Claim a plane so that it won't be used by a different crtc
    ///  
    /// Returns `None` if the plane could not be claimed
    pub fn claim_plane(&self, plane: plane::Handle) -> Option<PlaneClaim> {
        // Validate that we are called with an plane that belongs to us
        if self.planes.primary.handle == plane
            || self
                .planes
                .cursor
                .as_ref()
                .map(|p| p.handle == plane)
                .unwrap_or(false)
            || self.planes.overlay.iter().any(|p| p.handle == plane)
        {
            self.plane_claim_storage.claim(plane, self.crtc)
        } else {
            None
        }
    }

    /// Re-evaluates the current state of the crtc.
    ///
    /// It is recommended to call this function after this used [`Session`](crate::backend::session::Session)
    /// gets re-activated / VT switched to.
    ///
    /// Usually you do not need to call this in other circumstances, but if
    /// the state of the crtc is modified elsewhere, you may call this function
    /// to reset it's internal state.
    pub fn reset_state(&self) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.reset_state::<Self>(None),
            DrmSurfaceInternal::Legacy(surf) => surf.reset_state::<Self>(None),
        }
    }

    /// Returns if the underlying device is currently paused or not.
    pub fn is_active(&self) -> bool {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.active.load(Ordering::SeqCst),
            DrmSurfaceInternal::Legacy(surf) => surf.active.load(Ordering::SeqCst),
        }
    }

    #[cfg(feature = "backend_gbm")]
    pub(super) fn span(&self) -> &tracing::Span {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => &surf.span,
            DrmSurfaceInternal::Legacy(surf) => &surf.span,
        }
    }
}

fn ensure_legacy_planes<'a>(
    dev: &(impl ControlDevice + DevPath),
    planes: impl IntoIterator<Item = PlaneState<'a>>,
) -> Result<framebuffer::Handle, Error> {
    let state = planes.into_iter().next().ok_or(Error::NoPlane)?;

    if plane_type(dev, state.handle)? != PlaneType::Primary {
        return Err(Error::NonPrimaryPlane(state.handle));
    }

    let Some(config) = state.config else {
        // we need a config on the primary plane
        return Err(Error::NoFramebuffer(state.handle));
    };

    if config.dst.loc != Point::default() {
        // legacy does not support crtc position (technically we could do it,
        // but the position can only be changed by commit, not by page-flip,
        // so we just not allow it)
        return Err(Error::UnsupportedPlaneConfiguration(state.handle));
    }

    if config.src.loc != Point::default()
        || config
            .src
            .size
            .to_logical(1.0, Transform::Normal)
            .to_physical(1.0)
            != config.dst.size.to_f64()
    {
        // legacy does not support crop nor scale
        return Err(Error::UnsupportedPlaneConfiguration(state.handle));
    }

    if config.transform != Transform::Normal {
        // legacy does not support transform
        return Err(Error::UnsupportedPlaneConfiguration(state.handle));
    }

    Ok(config.fb)
}
