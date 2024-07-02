//! DRM syncobj protocol
//!
//! This module implement the `linux-drm-syncobj-v1` protocol, used to support
//! explicit sync.
//!
//! Currently, the implementation here assumes
//!
//! ```no_run
//!     pub struct State {
//! }
//! ```

use std::{cell::RefCell, os::unix::io::AsFd};
use wayland_protocols::wp::linux_drm_syncobj::v1::server::{
    wp_linux_drm_syncobj_manager_v1::{self, WpLinuxDrmSyncobjManagerV1},
    wp_linux_drm_syncobj_surface_v1::{self, WpLinuxDrmSyncobjSurfaceV1},
    wp_linux_drm_syncobj_timeline_v1::{self, WpLinuxDrmSyncobjTimelineV1},
};
use wayland_server::{
    protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource, Weak,
};

use super::{
    compositor::{self, with_states, BufferAssignment, Cacheable, SurfaceAttributes},
    dmabuf::get_dmabuf,
};
use crate::backend::drm::DrmDeviceFd;

mod sync_point;
pub use sync_point::{DrmSyncPoint, DrmTimeline};

/// Test if DRM device supports `syncobj_eventfd`.
// Similar to test used in Mutter
pub fn supports_syncobj_eventfd(device: &DrmDeviceFd) -> bool {
    // Pass device as palceholder for eventfd as well, since `drm_ffi` requires
    // a valid fd.
    match drm_ffi::syncobj::eventfd(device.as_fd(), 0, 0, device.as_fd(), false) {
        Ok(_) => unreachable!(),
        Err(err) => err.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Handler trait for DRM syncobj protocol.
pub trait DrmSyncobjHandler {
    /// Returns a mutable reference to the [`DrmSyncobjState`] delegate type
    fn drm_syncobj_state(&mut self) -> &mut DrmSyncobjState;
}

/// Data associated with a drm syncobj global
#[allow(missing_debug_implementations)]
pub struct DrmSyncobjGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

/// Pending DRM syncobj sync point state
#[derive(Debug, Default)]
pub struct DrmSyncobjCachedState {
    /// Timeline point signaled when buffer is ready to read
    pub acquire_point: Option<DrmSyncPoint>,
    /// Timeline point to be signaled when server is done with buffer
    pub release_point: Option<DrmSyncPoint>,
}

impl Cacheable for DrmSyncobjCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        Self {
            acquire_point: self.acquire_point.take(),
            release_point: self.release_point.take(),
        }
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        if self.acquire_point.is_some() && self.release_point.is_some() {
            if let Some(release_point) = &into.release_point {
                if let Err(err) = release_point.signal() {
                    tracing::error!("Failed to signal syncobj release point: {}", err);
                }
            }
            into.acquire_point = self.acquire_point;
            into.release_point = self.release_point;
        }
    }
}

/// Delegate type for a `wp_linux_drm_syncobj_manager_v1` global
#[derive(Debug)]
pub struct DrmSyncobjState {
    import_device: DrmDeviceFd,
}

impl DrmSyncobjState {
    /// Create a new `wp_linux_drm_syncobj_manager_v1` global
    ///
    /// The `import_device` will be used to import the syncobj fds, and wait on them.
    pub fn new<D>(display: &DisplayHandle, import_device: DrmDeviceFd) -> Self
    where
        D: GlobalDispatch<WpLinuxDrmSyncobjManagerV1, DrmSyncobjGlobalData>,
        D: 'static,
    {
        Self::new_with_filter::<D, _>(display, import_device, |_| true)
    }

    /// Create a new `wp_linuxdrm_syncobj_manager_v1` global with a client filter
    ///
    /// The `import_device` will be used to import the syncobj fds, and wait on them.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, import_device: DrmDeviceFd, filter: F) -> Self
    where
        D: GlobalDispatch<WpLinuxDrmSyncobjManagerV1, DrmSyncobjGlobalData>,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let _global = display.create_global::<D, WpLinuxDrmSyncobjManagerV1, DrmSyncobjGlobalData>(
            1,
            DrmSyncobjGlobalData {
                filter: Box::new(filter),
            },
        );

        Self { import_device }
    }
}

impl<D> GlobalDispatch<WpLinuxDrmSyncobjManagerV1, DrmSyncobjGlobalData, D> for DrmSyncobjState
where
    D: Dispatch<WpLinuxDrmSyncobjManagerV1, ()>,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<WpLinuxDrmSyncobjManagerV1>,
        _global_data: &DrmSyncobjGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init::<_, _>(resource, ());
    }

    fn can_view(client: Client, global_data: &DrmSyncobjGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

fn commit_hook<D: DrmSyncobjHandler>(_data: &mut D, _dh: &DisplayHandle, surface: &WlSurface) {
    compositor::with_states(surface, |states| {
        let mut cached = states.cached_state.get::<SurfaceAttributes>();
        let pending = cached.pending();
        let new_buffer = pending.buffer.as_ref().and_then(|buffer| match buffer {
            BufferAssignment::NewBuffer(buffer) => Some(buffer),
            _ => None,
        });
        if let Some(data) = states
            .data_map
            .get::<RefCell<Option<WpLinuxDrmSyncobjSurfaceV1>>>()
        {
            if let Some(syncobj_surface) = data.borrow().as_ref() {
                let mut cached = states.cached_state.get::<DrmSyncobjCachedState>();
                let pending = cached.pending();
                if pending.acquire_point.is_some() && new_buffer.is_none() {
                    syncobj_surface.post_error(
                        wp_linux_drm_syncobj_surface_v1::Error::NoBuffer as u32,
                        "acquire point without buffer".to_string(),
                    );
                } else if pending.acquire_point.is_some() && pending.release_point.is_none() {
                    syncobj_surface.post_error(
                        wp_linux_drm_syncobj_surface_v1::Error::NoReleasePoint as u32,
                        "acquire point without release point".to_string(),
                    );
                } else if pending.acquire_point.is_none() && pending.release_point.is_some() {
                    syncobj_surface.post_error(
                        wp_linux_drm_syncobj_surface_v1::Error::NoAcquirePoint as u32,
                        "release point without acquire point".to_string(),
                    );
                } else if let (Some(acquire), Some(release)) =
                    (pending.acquire_point.as_ref(), pending.release_point.as_ref())
                {
                    if acquire.timeline == release.timeline && acquire.point <= release.point {
                        syncobj_surface.post_error(
                            wp_linux_drm_syncobj_surface_v1::Error::ConflictingPoints as u32,
                            format!(
                                "release point '{}' is not greater than acquire point '{}'",
                                release.point, acquire.point
                            ),
                        );
                    }
                    if let Some(buffer) = new_buffer {
                        if get_dmabuf(buffer).is_err() {
                            syncobj_surface.post_error(
                                wp_linux_drm_syncobj_surface_v1::Error::UnsupportedBuffer as u32,
                                "sync points with non-dmabuf buffer".to_string(),
                            );
                        }
                    }
                }
            }
        }
    });
}

impl<D> Dispatch<WpLinuxDrmSyncobjManagerV1, (), D> for DrmSyncobjState
where
    D: Dispatch<WpLinuxDrmSyncobjSurfaceV1, DrmSyncobjSurfaceData>,
    D: Dispatch<WpLinuxDrmSyncobjTimelineV1, DrmSyncobjTimelineData>,
    D: DrmSyncobjHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &WpLinuxDrmSyncobjManagerV1,
        request: wp_linux_drm_syncobj_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_linux_drm_syncobj_manager_v1::Request::GetSurface { id, surface } => {
                let already_exists = with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<RefCell<Option<WpLinuxDrmSyncobjSurfaceV1>>>()
                        .map(|v| v.borrow().is_some())
                        .unwrap_or(false)
                });
                if already_exists {
                    resource.post_error(
                        wp_linux_drm_syncobj_manager_v1::Error::SurfaceExists as u32,
                        "the surface already has a syncobj_surface object associated".to_string(),
                    );
                    return;
                }
                let syncobj_surface = data_init.init::<_, _>(
                    id,
                    DrmSyncobjSurfaceData {
                        surface: surface.downgrade(),
                    },
                );
                with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing(|| RefCell::new(Some(syncobj_surface)))
                });
                compositor::add_pre_commit_hook::<D, _>(&surface, commit_hook);
            }
            wp_linux_drm_syncobj_manager_v1::Request::ImportTimeline { id, fd } => {
                match DrmTimeline::new(&state.drm_syncobj_state().import_device, fd.as_fd()) {
                    Ok(timeline) => {
                        data_init.init::<_, _>(id, DrmSyncobjTimelineData { timeline });
                    }
                    Err(err) => {
                        resource.post_error(
                            wp_linux_drm_syncobj_manager_v1::Error::InvalidTimeline as u32,
                            format!("failed to import syncobj timeline: {}", err),
                        );
                    }
                }
            }
            wp_linux_drm_syncobj_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

/// Data attached to wp_linux_drm_syncobj_surface_v1 objects
#[derive(Debug)]
pub struct DrmSyncobjSurfaceData {
    surface: Weak<WlSurface>,
}

impl<D> Dispatch<WpLinuxDrmSyncobjSurfaceV1, DrmSyncobjSurfaceData, D> for DrmSyncobjState {
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &WpLinuxDrmSyncobjSurfaceV1,
        request: wp_linux_drm_syncobj_surface_v1::Request,
        data: &DrmSyncobjSurfaceData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let Ok(surface) = data.surface.upgrade() else {
            return;
        };
        match request {
            wp_linux_drm_syncobj_surface_v1::Request::Destroy => {}
            wp_linux_drm_syncobj_surface_v1::Request::SetAcquirePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                let sync_point = DrmSyncPoint {
                    timeline: timeline
                        .data::<DrmSyncobjTimelineData>()
                        .unwrap()
                        .timeline
                        .clone(),
                    point: ((point_hi as u64) << 32) + (point_lo as u64),
                };
                with_states(&surface, |states| {
                    let mut cached = states.cached_state.get::<DrmSyncobjCachedState>();
                    let cached_state = cached.pending();
                    cached_state.acquire_point = Some(sync_point);
                });
            }
            wp_linux_drm_syncobj_surface_v1::Request::SetReleasePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                let sync_point = DrmSyncPoint {
                    timeline: timeline
                        .data::<DrmSyncobjTimelineData>()
                        .unwrap()
                        .timeline
                        .clone(),
                    point: ((point_hi as u64) << 32) + (point_lo as u64),
                };
                with_states(&surface, |states| {
                    let mut cached = states.cached_state.get::<DrmSyncobjCachedState>();
                    let cached_state = cached.pending();
                    cached_state.release_point = Some(sync_point);
                });
            }
            _ => unreachable!(),
        }
    }
}

/// Data attached to wp_linux_drm_syncobj_timeline_v1 objects
#[derive(Debug)]
pub struct DrmSyncobjTimelineData {
    timeline: DrmTimeline,
}

impl<D> Dispatch<WpLinuxDrmSyncobjTimelineV1, DrmSyncobjTimelineData, D> for DrmSyncobjState {
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &WpLinuxDrmSyncobjTimelineV1,
        request: wp_linux_drm_syncobj_timeline_v1::Request,
        _data: &DrmSyncobjTimelineData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_linux_drm_syncobj_timeline_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

/// Macro to delegate implementation of the drm syncobj protocol to [`DrmSyncobjState`].
///
/// You must also implement [`DrmSyncobjHandler`] to use this.
#[macro_export]
macro_rules! delegate_drm_syncobj {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1: $crate::wayland::drm_syncobj::DrmSyncobjGlobalData
        ] => $crate::wayland::drm_syncobj::DrmSyncobjState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1: ()
        ] => $crate::wayland::drm_syncobj::DrmSyncobjState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1: $crate::wayland::drm_syncobj::DrmSyncobjSurfaceData
        ] => $crate::wayland::drm_syncobj::DrmSyncobjState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1: $crate::wayland::drm_syncobj::DrmSyncobjTimelineData
        ] => $crate::wayland::drm_syncobj::DrmSyncobjState);
    }
}
