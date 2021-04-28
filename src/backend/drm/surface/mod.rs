use std::collections::HashSet;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;

use drm::Device as BasicDevice;
use drm::control::{Device as ControlDevice, Mode, crtc, connector, framebuffer, plane};

pub(super) mod atomic;
pub(super) mod legacy;
use super::error::Error;
use atomic::AtomicDrmSurface;
use legacy::LegacyDrmSurface;
use crate::backend::allocator::Format;

pub struct DrmSurface<A: AsRawFd + 'static>
{
    pub(super) crtc: crtc::Handle,
    pub(super) plane: plane::Handle,
    pub(super) internal: Arc<DrmSurfaceInternal<A>>,
    pub(super) formats: HashSet<Format>,
}

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

    /// Returns the underlying [`plane`](drm::control::plane) of this surface
    pub fn plane(&self) -> plane::Handle {
        self.plane
    }
    
    /// Currently used [`connector`](drm::control::connector)s of this `Surface`
    pub fn current_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.current_connectors(),
            DrmSurfaceInternal::Legacy(surf) => surf.current_connectors(),
        }
    }

    /// Returns the pending [`connector`](drm::control::connector)s
    /// used after the next [`commit`](Surface::commit) of this [`Surface`]
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

    /// Returns true whenever any state changes are pending to be commited
    ///
    /// The following functions may trigger a pending commit:
    /// - [`add_connector`](Surface::add_connector)
    /// - [`remove_connector`](Surface::remove_connector)
    /// - [`use_mode`](Surface::use_mode)
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
    /// operation if a commit really is necessary using [`commit_pending`](RawSurface::commit_pending).
    ///
    /// This operation is not necessarily blocking until the crtc is in the desired state,
    /// but will trigger a `vblank` event once done.
    /// Make sure to [set a `DeviceHandler`](Device::set_handler) and
    /// [register the belonging `Device`](device_bind) before to receive the event in time.
    pub fn commit(&self, framebuffer: framebuffer::Handle, event: bool) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.commit(framebuffer, event),
            DrmSurfaceInternal::Legacy(surf) => surf.commit(framebuffer, event),
        }
    }

    /// Page-flip the underlying [`crtc`](drm::control::crtc)
    /// to a new given [`framebuffer`].
    ///
    /// This will not cause the crtc to modeset.
    ///
    /// This operation is not blocking and will produce a `vblank` event once swapping is done.
    /// Make sure to [set a `DeviceHandler`](Device::set_handler) and
    /// [register the belonging `Device`](device_bind) before to receive the event in time.
    pub fn page_flip(&self, framebuffer: framebuffer::Handle, event: bool) -> Result<(), Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.page_flip(framebuffer, event),
            DrmSurfaceInternal::Legacy(surf) => surf.page_flip(framebuffer, event),
        }
    }

    pub fn supported_formats(&self) -> &HashSet<Format> {
        &self.formats
    }

    pub fn test_buffer(&self, fb: framebuffer::Handle, mode: &Mode, allow_screen_change: bool) -> Result<bool, Error> {
        match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.test_buffer(fb, mode),
            DrmSurfaceInternal::Legacy(surf) => if allow_screen_change {
                surf.test_buffer(fb, mode)
            } else { Ok(false) } // There is no test-commiting with the legacy interface
        }
    }
}