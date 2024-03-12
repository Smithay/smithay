use std::{cell::RefCell, os::unix::io::OwnedFd, sync::Arc};
use wayland_protocols::wp::linux_drm_syncobj::v1::server::{
    wp_linux_drm_syncobj_manager_v1::{self, WpLinuxDrmSyncobjManagerV1},
    wp_linux_drm_syncobj_surface_v1::{self, WpLinuxDrmSyncobjSurfaceV1},
    wp_linux_drm_syncobj_timeline_v1::{self, WpLinuxDrmSyncobjTimelineV1},
};
use wayland_server::{
    protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource, Weak,
};

use super::compositor::{self, with_states, BufferAssignment, Cacheable, SurfaceAttributes};

#[derive(Clone)]
struct SyncPoint {
    fd: Arc<OwnedFd>,
    point: u64,
}

#[derive(Default)]
struct DrmSyncobjCachedState {
    acquire_point: Option<SyncPoint>,
    release_point: Option<SyncPoint>,
}

impl Cacheable for DrmSyncobjCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        Self {
            acquire_point: None,
            release_point: None,
        }
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        if self.acquire_point.is_some() && self.release_point.is_some() {
            into.acquire_point = self.acquire_point;
            into.release_point = self.release_point;
        } else {
            into.acquire_point = None;
            into.release_point = None;
        }
    }
}

pub struct DrmSyncobjState {}

impl DrmSyncobjState {
    pub fn new<D: 'static>(display: &DisplayHandle) -> Self {
        display.create_delegated_global::<D, WpLinuxDrmSyncobjManagerV1, _, Self>(1, ());
        Self {}
    }

    // TODO new_with_filter
}

impl<D> GlobalDispatch<WpLinuxDrmSyncobjManagerV1, (), D> for DrmSyncobjState {
    fn bind(
        state: &mut D,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<WpLinuxDrmSyncobjManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }
}

fn commit_hook<D>(_: &mut D, _dh: &DisplayHandle, surface: &WlSurface) {
    compositor::with_states(&surface, |states| {
        let cached = &states.cached_state;
        let has_new_buffer = matches!(
            cached.pending::<SurfaceAttributes>().buffer,
            Some(BufferAssignment::NewBuffer(_))
        );
        // TODO what if syncobj surface is destroyed?
        if let Some(data) = states
            .data_map
            .get::<RefCell<Option<WpLinuxDrmSyncobjSurfaceV1>>>()
        {
            if let Some(syncobj_surface) = data.borrow().as_ref() {
                let pending = cached.pending::<DrmSyncobjCachedState>();
                if pending.acquire_point.is_some() && !has_new_buffer {
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
                    if Arc::ptr_eq(&acquire.fd, &release.fd) && acquire.point <= release.point {
                        syncobj_surface.post_error(
                            wp_linux_drm_syncobj_surface_v1::Error::ConflictingPoints as u32,
                            format!(
                                "release point '{}' is not greater than acquire point '{}'",
                                release.point, acquire.point
                            ),
                        );
                    }
                }
                // TODO unsupported buffer error
            }
        }
    });
}

impl<D> Dispatch<WpLinuxDrmSyncobjManagerV1, (), D> for DrmSyncobjState {
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
                let syncobj_surface = data_init.init_delegated::<_, _, Self>(
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
                data_init.init_delegated::<_, _, Self>(id, DrmSyncobjTimelineData { fd: Arc::new(fd) });
                // TODO import, protocol error if it fails? On which GPU?
            }
            wp_linux_drm_syncobj_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

struct DrmSyncobjSurfaceData {
    surface: Weak<WlSurface>,
}

impl<D> Dispatch<WpLinuxDrmSyncobjSurfaceV1, DrmSyncobjSurfaceData, D> for DrmSyncobjState {
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &WpLinuxDrmSyncobjSurfaceV1,
        request: wp_linux_drm_syncobj_surface_v1::Request,
        data: &DrmSyncobjSurfaceData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let Ok(surface) = data.surface.upgrade() else {
            return;
        };
        match request {
            wp_linux_drm_syncobj_surface_v1::Request::Destroy => {
                // TODO
            }
            wp_linux_drm_syncobj_surface_v1::Request::SetAcquirePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                let sync_point = SyncPoint {
                    fd: timeline.data::<DrmSyncobjTimelineData>().unwrap().fd.clone(),
                    point: ((point_hi as u64) << 32) + (point_lo as u64),
                };
                with_states(&surface, |states| {
                    let mut cached_state = states.cached_state.pending::<DrmSyncobjCachedState>();
                    cached_state.acquire_point = Some(sync_point);
                });
            }
            wp_linux_drm_syncobj_surface_v1::Request::SetReleasePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                let sync_point = SyncPoint {
                    fd: timeline.data::<DrmSyncobjTimelineData>().unwrap().fd.clone(),
                    point: ((point_hi as u64) << 32) + (point_lo as u64),
                };
                with_states(&surface, |states| {
                    let mut cached_state = states.cached_state.pending::<DrmSyncobjCachedState>();
                    cached_state.release_point = Some(sync_point);
                });
            }
            _ => unreachable!(),
        }
    }
}

struct DrmSyncobjTimelineData {
    fd: Arc<OwnedFd>,
}

impl<D> Dispatch<WpLinuxDrmSyncobjTimelineV1, DrmSyncobjTimelineData, D> for DrmSyncobjState {
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &WpLinuxDrmSyncobjTimelineV1,
        request: wp_linux_drm_syncobj_timeline_v1::Request,
        _data: &DrmSyncobjTimelineData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_linux_drm_syncobj_timeline_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}
