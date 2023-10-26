use std::{ffi::CString, os::unix::prelude::AsFd, sync::Mutex};

use crate::{
    output::{Output, WeakOutput},
    utils::sealed_file::SealedFile,
    wayland::compositor,
};

use super::{
    wp_color_management_output_v1, wp_color_management_surface_v1, wp_color_manager_v1,
    wp_image_description_creator_icc_v1, wp_image_description_creator_params_v1, wp_image_description_v1,
    ColorManagementHandler, ColorManagementOutput, ColorManagementState, ColorManagementSurfaceCachedState,
    ColorManagementSurfaceData, DescriptionError, Feature, IccData, ImageDescriptionData,
    ImageDescriptionIccBuilder, ImageDescriptionParametricBuilder, ParametricPrimaries, Primaries,
    TransferFunction,
};
use tracing::warn;
use wayland_server::{
    protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource, WEnum, Weak,
};

impl<D> GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D> for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn bind(
        state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<wp_color_manager_v1::WpColorManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let state = state.color_management_state();
        let instance = data_init.init(resource, ());

        for feature in &state.supported_features {
            instance.supported_feature(*feature);
        }
        for intent in &state.supported_rendering_intents {
            instance.supported_intent(*intent);
        }
        for code_point in &state.supported_tf_cicp {
            instance.supported_tf_cicp(*code_point);
        }
        for code_point in &state.supported_primaries_cicp {
            instance.supported_primaries_cicp(*code_point);
        }
    }
}

impl<D> Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D> for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &wp_color_manager_v1::WpColorManagerV1,
        request: wp_color_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_color_manager_v1::Request::GetColorManagementOutput { id, output } => {
                let Some(output) = Output::from_resource(&output) else {
                    resource.post_error(wp_color_manager_v1::Error::UnsupportedFeature, "WlOutput has no associated `Output`");
                    return
                };

                let color_output = output
                    .user_data()
                    .get_or_insert(|| ColorManagementOutput::new(state.description_for_output(&output)));

                let instance = data_init.init(id, output.downgrade());
                color_output.add_instance(instance);
            }
            wp_color_manager_v1::Request::GetColorManagementSurface { id, surface } => {
                compositor::with_states(&surface, |states| {
                    let data = states.data_map.get_or_insert_threadsafe(|| {
                        ColorManagementSurfaceData::new(state.preferred_description_for_surface(&surface))
                    });

                    let instance = data_init.init(id, surface.downgrade());
                    data.add_instance(instance);
                });
            }
            wp_color_manager_v1::Request::NewIccCreator { obj } => {
                let state = state.color_management_state();
                if !state.supported_features.contains(&Feature::IccV2V4) {
                    resource.post_error(
                        wp_color_manager_v1::Error::UnsupportedFeature,
                        "Compositor doesn't support the ICC image description creator",
                    );
                    return;
                }

                data_init.init(obj, Mutex::new(Some(ImageDescriptionIccBuilder::default())));
            }
            wp_color_manager_v1::Request::NewParametricCreator { obj } => {
                let state = state.color_management_state();
                if !state.supported_features.contains(&Feature::Parametric) {
                    resource.post_error(
                        wp_color_manager_v1::Error::UnsupportedFeature,
                        "Compositor doesn't support the Parametric image description creator",
                    );
                    return;
                }

                data_init.init(
                    obj,
                    Mutex::new(Some(ImageDescriptionParametricBuilder::default())),
                );
            }
            _ => {}
        }
    }
}

impl<D> Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
    for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &wp_color_management_output_v1::WpColorManagementOutputV1,
        request: wp_color_management_output_v1::Request,
        data: &WeakOutput,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_color_management_output_v1::Request::GetImageDescription { image_description } => {
                if let Some(output) = data.upgrade() {
                    let data = output.user_data().get::<ColorManagementOutput>().unwrap();
                    let info = data.description.lock().unwrap().clone();
                    let instance = data_init.init(
                        image_description,
                        ImageDescriptionData {
                            get_information: true,
                            info: info.clone(),
                        },
                    );
                    instance.ready(info.0.id as u32);
                } else {
                    let failed_desc = data_init.init(image_description, ());
                    failed_desc.failed(
                        wp_image_description_v1::Cause::LowVersion,
                        "Output was destroyed".into(),
                    );
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: wayland_backend::server::ClientId,
        resource: &wp_color_management_output_v1::WpColorManagementOutputV1,
        data: &WeakOutput,
    ) {
        if let Some(output) = data.upgrade() {
            if let Some(data) = output.user_data().get::<ColorManagementOutput>() {
                data.remove_instance(resource);
            }
        }
    }
}

impl<D> Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
    for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &wp_color_management_surface_v1::WpColorManagementSurfaceV1,
        request: wp_color_management_surface_v1::Request,
        data: &Weak<WlSurface>,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_color_management_surface_v1::Request::GetPreferred { image_description } => {
                if let Ok(surface) = data.upgrade() {
                    compositor::with_states(&surface, |states| {
                        let data = states.data_map.get::<ColorManagementSurfaceData>().unwrap();
                        let info = data.preferred.lock().unwrap().clone();
                        let instance = data_init.init(
                            image_description,
                            ImageDescriptionData {
                                get_information: true,
                                info: info.clone(),
                            },
                        );
                        instance.ready(info.0.id as u32);
                    });
                } else {
                    let failed_desc = data_init.init(image_description, ());
                    failed_desc.failed(
                        wp_image_description_v1::Cause::LowVersion,
                        "Surface was destroyed".into(),
                    );
                }
            }
            wp_color_management_surface_v1::Request::SetDefaultImageDescription => {
                if let Ok(surface) = data.upgrade() {
                    compositor::with_states(&surface, |states| {
                        *states.cached_state.pending::<ColorManagementSurfaceCachedState>() =
                            ColorManagementSurfaceCachedState::default();
                    })
                }
            }
            wp_color_management_surface_v1::Request::SetImageDescription {
                image_description,
                render_intent,
            } => {
                if let Ok(surface) = data.upgrade() {
                    if let Some(data) = image_description.data::<ImageDescriptionData>() {
                        compositor::with_states(&surface, |states| {
                            *states.cached_state.pending::<ColorManagementSurfaceCachedState>() =
                                ColorManagementSurfaceCachedState {
                                    description: Some(data.info.clone()),
                                    render_intent: match render_intent {
                                        WEnum::Value(val) => {
                                            let state = state.color_management_state();
                                            if state.supported_rendering_intents.contains(&val) {
                                                val
                                            } else {
                                                resource.post_error(
                                                    wp_color_management_surface_v1::Error::RenderIntent,
                                                    "Unsupported render intent",
                                                );
                                                return;
                                            }
                                        }
                                        WEnum::Unknown(_) => {
                                            resource.post_error(
                                                wp_color_management_surface_v1::Error::RenderIntent,
                                                "Unknown render intent (wrong version?)",
                                            );
                                            return;
                                        }
                                    },
                                };
                        })
                    } else {
                        image_description.post_error(
                            wp_image_description_v1::Error::NotReady,
                            "Tried to set a failed image description on a surface",
                        );
                        return;
                    }
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: wayland_backend::server::ClientId,
        resource: &wp_color_management_surface_v1::WpColorManagementSurfaceV1,
        data: &Weak<WlSurface>,
    ) {
        if let Ok(surface) = data.upgrade() {
            compositor::with_states(&surface, |states| {
                if let Some(data) = states.data_map.get::<ColorManagementSurfaceData>() {
                    data.remove_instance(resource);
                }
            })
        }
    }
}

impl<D> Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D> for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        resource: &wp_image_description_v1::WpImageDescriptionV1,
        request: <wp_image_description_v1::WpImageDescriptionV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_image_description_v1::Request::Destroy => {}
            _ => resource.post_error(
                wp_image_description_v1::Error::NotReady,
                "Image description had failed",
            ),
        }
    }
}

impl<D> Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
    for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        resource: &wp_image_description_v1::WpImageDescriptionV1,
        request: wp_image_description_v1::Request,
        data: &ImageDescriptionData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_image_description_v1::Request::GetInformation => {
                if !data.get_information {
                    resource.post_error(
                        wp_image_description_v1::Error::NoInformation,
                        "Constructor doesn't allow get_information",
                    );
                    return;
                }

                match &data.info.0.contents {
                    super::ImageDescriptionContents::ICC(IccData { data, file }) => {
                        let mut file = file.lock().unwrap();
                        if file.is_none() {
                            match SealedFile::with_data(CString::new("icc").unwrap(), data) {
                                Ok(new_file) => {
                                    *file = Some(new_file);
                                }
                                Err(err) => {
                                    warn!(?err, "File to create memory map for icc file");
                                    resource.failed(
                                        wp_image_description_v1::Cause::Unsupported,
                                        "Internal error".into(),
                                    );
                                    return;
                                }
                            };
                        }
                        if let Some(file) = file.as_ref() {
                            resource.icc_file(file.as_fd(), file.size() as u32);
                        }
                    }
                    super::ImageDescriptionContents::Parametric {
                        tf,
                        primaries,
                        target_primaries,
                        target_luminance,
                        max_cll: max_ccl,
                        max_fall,
                    } => {
                        match tf {
                            TransferFunction::CICP(code_point) => resource.tf_cicp(*code_point),
                            TransferFunction::Power(pow) => resource.tf_power(*pow),
                        };
                        match primaries {
                            Primaries::CICP(code_point) => resource.primaries_cicp(*code_point),
                            Primaries::Parametric(ParametricPrimaries {
                                red,
                                green,
                                blue,
                                white,
                            }) => resource
                                .primaries(red.0, red.1, green.0, green.1, blue.0, blue.1, white.0, white.1),
                        };
                        if let Some(ParametricPrimaries {
                            red,
                            green,
                            blue,
                            white,
                        }) = target_primaries
                        {
                            resource.target_primaries(
                                red.0, red.1, green.0, green.1, blue.0, blue.1, white.0, white.1,
                            );
                            if let Some((min_lum, max_lum)) = target_luminance {
                                resource.target_luminance(*min_lum, *max_lum);
                            }
                        }
                        if let Some(max_ccl) = max_ccl {
                            resource.target_maxCLL(*max_ccl);
                        }
                        if let Some(max_fall) = max_fall {
                            resource.target_maxFALL(*max_fall);
                        }
                    }
                }

                resource.done();
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_backend::server::ClientId,
        _resource: &wp_image_description_v1::WpImageDescriptionV1,
        data: &ImageDescriptionData,
    ) {
        state
            .color_management_state()
            .known_image_descriptions
            .retain(|_, v| {
                if let Some(v) = v.upgrade() {
                    !std::sync::Arc::ptr_eq(&v, &data.info.0)
                } else {
                    false
                }
            })
    }
}

impl<D>
    Dispatch<
        wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
        Mutex<Option<ImageDescriptionIccBuilder>>,
        D,
    > for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
        request: wp_image_description_creator_icc_v1::Request,
        data: &Mutex<Option<ImageDescriptionIccBuilder>>,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_image_description_creator_icc_v1::Request::SetIccFile {
                icc_profile,
                offset,
                length,
            } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_icc_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if length > 1024 * 1024 * 4 {
                    resource.post_error(
                        wp_image_description_creator_icc_v1::Error::BadSize,
                        "Size larger than 4MiB",
                    );
                    return;
                }

                let file = std::fs::File::from(icc_profile);
                match data.with_file(file, offset as usize, length as usize) {
                    Ok(true) => {
                        resource.post_error(
                            wp_image_description_creator_icc_v1::Error::AlreadySet,
                            "ICC file was already set",
                        );
                        return;
                    }
                    Err(err) => {
                        resource.post_error(
                            wp_image_description_creator_icc_v1::Error::BadFd,
                            format!("Failed to read ICC file: {}", err),
                        );
                        return;
                    }
                    Ok(false) => {}
                }
            }
            wp_image_description_creator_icc_v1::Request::Create { image_description } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.take() else {
                    resource.post_error(wp_image_description_creator_icc_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                let color_state = state.color_management_state();
                match color_state.build_description(data) {
                    Ok(desc) => {
                        if state.verify_icc(match &desc.0.contents {
                            super::ImageDescriptionContents::ICC(IccData { data, .. }) => data,
                            _ => unreachable!(),
                        }) {
                            let instance = data_init.init(
                                image_description,
                                ImageDescriptionData {
                                    get_information: false,
                                    info: desc.clone(),
                                },
                            );
                            instance.ready(desc.0.id as u32);
                        } else {
                            let instance = data_init.init(image_description, ());
                            instance.failed(
                                wp_image_description_v1::Cause::Unsupported,
                                "ICC file failed to parse".into(),
                            );
                            return;
                        }
                    }
                    Err(DescriptionError::IncompleteSet) => {
                        resource.post_error(
                            wp_image_description_creator_icc_v1::Error::IncompleteSet,
                            "incomplete parameter set",
                        );
                        return;
                    }
                    Err(DescriptionError::InconsistentSet) => {
                        resource.post_error(
                            wp_image_description_creator_icc_v1::Error::InconsistentSet,
                            "invalid combination of parameters",
                        );
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

impl<D>
    Dispatch<
        wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
        Mutex<Option<ImageDescriptionParametricBuilder>>,
        D,
    > for ColorManagementState
where
    D: ColorManagementHandler
        + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
        + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
        + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
        + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
        + Dispatch<
            wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
            Mutex<Option<ImageDescriptionIccBuilder>>,
            D,
        > + Dispatch<
            wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
            Mutex<Option<ImageDescriptionParametricBuilder>>,
            D,
        > + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
        request: wp_image_description_creator_params_v1::Request,
        data: &Mutex<Option<ImageDescriptionParametricBuilder>>,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let color_state = state.color_management_state();
        match request {
            wp_image_description_creator_params_v1::Request::SetTfCicp { tf_code } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state.supported_tf_cicp.contains(&tf_code) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InvalidTf,
                        "Unsupported transfer function code point",
                    );
                    return;
                }

                if data.set_tf(TransferFunction::CICP(tf_code)) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Transfer function was already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::SetTfPower { eexp } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state.supported_features.contains(&Feature::SetTfPower) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InvalidTf,
                        "Unsupported feature set_tf_power",
                    );
                    return;
                }

                if eexp < 10000 || eexp > 100000 {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InvalidTf,
                        "Transfer function exponent out of range",
                    );
                    return;
                }

                if data.set_tf(TransferFunction::Power(eexp)) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Transfer function was already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::SetPrimariesCicp { primaries_code } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state.supported_primaries_cicp.contains(&primaries_code) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InvalidPrimaries,
                        "Unsupported primaries code point",
                    );
                    return;
                }

                if data.set_primaries(Primaries::CICP(primaries_code)) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Primaries were already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::SetPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state.supported_features.contains(&Feature::SetPrimaries) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InvalidPrimaries,
                        "Unsupported feature set_primaries",
                    );
                    return;
                }

                if data.set_primaries(Primaries::Parametric(ParametricPrimaries {
                    red: (r_x, r_y),
                    green: (g_x, g_y),
                    blue: (b_x, b_y),
                    white: (w_x, w_y),
                })) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Primaries were already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::SetMasteringDisplayPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state
                    .supported_features
                    .contains(&Feature::SetMasteringDisplayPrimaries)
                {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InvalidMastering,
                        "Unsupported feature set_mastering_display_primaries",
                    );
                    return;
                }

                if data.set_target_primaries(ParametricPrimaries {
                    red: (r_x, r_y),
                    green: (g_x, g_y),
                    blue: (b_x, b_y),
                    white: (w_x, w_y),
                }) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Mastering Primaries were already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::SetMasteringLuminance { min_lum, max_lum } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state.supported_tf_cicp.contains(&16)
                    && !color_state.supported_tf_cicp.contains(&18)
                {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InconsistentSet,
                        "Mastering Luminance is only valid for Rec. ITU-R BT.2100-2, which is unsupported",
                    );
                    return;
                }

                if max_lum * 1000 <= min_lum {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InconsistentSet,
                        "MaxLUM <= MinLUM",
                    );
                    return;
                }

                if data.set_target_luminance((min_lum, max_lum)) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Mastering Primaries were already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::SetMaxCLL { maxCLL } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state.supported_tf_cicp.contains(&16)
                    && !color_state.supported_tf_cicp.contains(&18)
                {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InconsistentSet,
                        "Max CCL is only valid for Rec. ITU-R BT.2100-2, which is unsupported",
                    );
                    return;
                }

                if data.set_max_cll(maxCLL) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Max CCL was already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::SetMaxFALL { maxFALL } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.as_mut() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                if !color_state.supported_tf_cicp.contains(&16)
                    && !color_state.supported_tf_cicp.contains(&18)
                {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::InconsistentSet,
                        "Max FALL is only valid for Rec. ITU-R BT.2100-2, which is unsupported",
                    );
                    return;
                }

                if data.set_max_fall(maxFALL) {
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::AlreadySet,
                        "Max CCL was already set",
                    );
                    return;
                }
            }
            wp_image_description_creator_params_v1::Request::Create { image_description } => {
                let mut data_guard = data.lock().unwrap();
                let Some(data) = data_guard.take() else {
                    resource.post_error(wp_image_description_creator_params_v1::Error::AlreadyUsed, "Creator was already used");
                    return
                };

                match color_state.build_description(data) {
                    Ok(desc) => {
                        let instance = data_init.init(
                            image_description,
                            ImageDescriptionData {
                                get_information: false,
                                info: desc.clone(),
                            },
                        );
                        instance.ready(desc.0.id as u32);
                    }
                    Err(DescriptionError::IncompleteSet) => {
                        resource.post_error(
                            wp_image_description_creator_icc_v1::Error::IncompleteSet,
                            "incomplete parameter set",
                        );
                        return;
                    }
                    Err(DescriptionError::InconsistentSet) => {
                        resource.post_error(
                            wp_image_description_creator_icc_v1::Error::InconsistentSet,
                            "invalid combination of parameters",
                        );
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}
