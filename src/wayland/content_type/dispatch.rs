use wayland_protocols::wp::content_type::v1::server::{
    wp_content_type_manager_v1::{self, WpContentTypeManagerV1},
    wp_content_type_v1::{self, WpContentTypeV1},
};
use wayland_server::{
    backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use super::{ContentTypeState, ContentTypeSurfaceCachedState, ContentTypeSurfaceData, ContentTypeUserData};
use crate::wayland::compositor;

impl<D> GlobalDispatch<WpContentTypeManagerV1, (), D> for ContentTypeState
where
    D: GlobalDispatch<WpContentTypeManagerV1, ()>,
    D: Dispatch<WpContentTypeManagerV1, ()>,
    D: Dispatch<WpContentTypeV1, ContentTypeUserData>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WpContentTypeManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<WpContentTypeManagerV1, (), D> for ContentTypeState
where
    D: Dispatch<WpContentTypeManagerV1, ()>,
    D: Dispatch<WpContentTypeV1, ContentTypeUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _: &Client,
        manager: &wp_content_type_manager_v1::WpContentTypeManagerV1,
        request: wp_content_type_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_content_type_manager_v1::Request::GetSurfaceContentType { id, surface } => {
                let already_taken = compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(ContentTypeSurfaceData::new);
                    let data = states.data_map.get::<ContentTypeSurfaceData>().unwrap();

                    let already_taken = data.is_resource_attached();

                    if !already_taken {
                        data.set_is_resource_attached(true);
                    }

                    already_taken
                });

                if already_taken {
                    manager.post_error(
                        wp_content_type_manager_v1::Error::AlreadyConstructed,
                        "WlSurface already has WpSurfaceContentType attached",
                    )
                } else {
                    data_init.init(id, ContentTypeUserData::new(surface));
                }
            }

            wp_content_type_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<WpContentTypeV1, ContentTypeUserData, D> for ContentTypeState
where
    D: Dispatch<WpContentTypeV1, ContentTypeUserData>,
{
    fn request(
        _state: &mut D,
        _: &Client,
        _: &WpContentTypeV1,
        request: wp_content_type_v1::Request,
        data: &ContentTypeUserData,
        _dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_content_type_v1::Request::SetContentType { content_type } => {
                let wayland_server::WEnum::Value(content_type) = content_type else {
                    return;
                };
                let Some(surface) = data.wl_surface() else {
                    return;
                };

                compositor::with_states(&surface, |states| {
                    states
                        .cached_state
                        .pending::<ContentTypeSurfaceCachedState>()
                        .content_type = content_type;
                })
            }
            // Switch back to not specifying the content type of this surface.
            // This is equivalent to setting the content type to none,
            // including double buffering semantics.
            wp_content_type_v1::Request::Destroy => {
                let Some(surface) = data.wl_surface() else {
                    return;
                };

                compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<ContentTypeSurfaceData>()
                        .unwrap()
                        .set_is_resource_attached(false);

                    states
                        .cached_state
                        .pending::<ContentTypeSurfaceCachedState>()
                        .content_type = wp_content_type_v1::Type::None;
                });
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, _object: &WpContentTypeV1, _data: &ContentTypeUserData) {
        // Nothing to do here, graceful Destroy is already handled with double buffering
        // and in case of client close WlSurface destroyed handler will clean up the data anyway,
        // so there is no point in queuing new update
    }
}
