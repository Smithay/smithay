use std::{os::unix::io::OwnedFd, sync::Arc};
use wayland_protocols::wp::linux_drm_syncobj::v1::server::{
    wp_linux_drm_syncobj_manager_v1::{self, WpLinuxDrmSyncobjManagerV1},
    wp_linux_drm_syncobj_surface_v1::{self, WpLinuxDrmSyncobjSurfaceV1},
    wp_linux_drm_syncobj_timeline_v1::{self, WpLinuxDrmSyncobjTimelineV1},
};
use wayland_server::{
    protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

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

impl<D> Dispatch<WpLinuxDrmSyncobjManagerV1, (), D> for DrmSyncobjState {
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &WpLinuxDrmSyncobjManagerV1,
        request: wp_linux_drm_syncobj_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_linux_drm_syncobj_manager_v1::Request::GetSurface { id, surface } => {
                // XXX protocol error if already exists for surface
                data_init.init_delegated::<_, _, Self>(id, DrmSyncobjSurfaceData { surface });
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
    surface: WlSurface,
}

impl<D> Dispatch<WpLinuxDrmSyncobjSurfaceV1, DrmSyncobjSurfaceData, D> for DrmSyncobjState {
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &WpLinuxDrmSyncobjSurfaceV1,
        request: wp_linux_drm_syncobj_surface_v1::Request,
        _data: &DrmSyncobjSurfaceData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_linux_drm_syncobj_surface_v1::Request::Destroy => {}
            wp_linux_drm_syncobj_surface_v1::Request::SetAcquirePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                let fd = &timeline.data::<DrmSyncobjTimelineData>().unwrap().fd;
                let point = ((point_hi as u64) << 32) + (point_lo as u64);
            }
            wp_linux_drm_syncobj_surface_v1::Request::SetReleasePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                let fd = &timeline.data::<DrmSyncobjTimelineData>().unwrap().fd;
                let point = ((point_hi as u64) << 32) + (point_lo as u64);
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
