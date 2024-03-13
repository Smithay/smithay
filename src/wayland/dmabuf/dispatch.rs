use std::sync::{atomic::AtomicBool, Mutex};

use wayland_protocols::wp::linux_dmabuf::zv1::server::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_server::{
    backend::ClientId, protocol::wl_buffer, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource,
};

use crate::{
    backend::allocator::dmabuf::{Dmabuf, Plane, MAX_PLANES},
    wayland::{buffer::BufferHandler, compositor},
};

use super::{
    DmabufData, DmabufFeedbackData, DmabufGlobal, DmabufGlobalData, DmabufHandler, DmabufParamsData,
    DmabufState, Import, ImportNotifier, Modifier, SurfaceDmabufFeedbackState,
};

impl<D> Dispatch<wl_buffer::WlBuffer, Dmabuf, D> for DmabufState
where
    D: Dispatch<wl_buffer::WlBuffer, Dmabuf> + BufferHandler,
{
    fn request(
        _data: &mut D,
        _client: &Client,
        _buffer: &wl_buffer::WlBuffer,
        request: wl_buffer::Request,
        _udata: &Dmabuf,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_buffer::Request::Destroy => {
                // Handled in the destroyed callback.
            }

            _ => unreachable!(),
        }
    }

    fn destroyed(data: &mut D, _client: ClientId, buffer: &wl_buffer::WlBuffer, _udata: &Dmabuf) {
        data.buffer_destroyed(buffer);
    }
}

impl<D> Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufData, D> for DmabufState
where
    D: Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufData>
        + Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
        + Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, DmabufFeedbackData>
        + DmabufHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        request: zwp_linux_dmabuf_v1::Request,
        data: &DmabufData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_linux_dmabuf_v1::Request::Destroy => {}

            zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                data_init.init(
                    params_id,
                    DmabufParamsData {
                        id: data.id,
                        used: AtomicBool::new(false),
                        formats: data.formats.clone(),
                        modifier: Mutex::new(None),
                        planes: Mutex::new(Vec::with_capacity(MAX_PLANES)),
                    },
                );
            }

            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id } => {
                let feedback = data_init.init(
                    id,
                    DmabufFeedbackData {
                        known_default_feedbacks: data.known_default_feedbacks.clone(),
                        surface: None,
                    },
                );

                data.known_default_feedbacks
                    .lock()
                    .unwrap()
                    .push(feedback.downgrade());

                data.default_feedback
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .send(&feedback);
            }

            zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id, surface } => {
                let feedback = data_init.init(
                    id,
                    DmabufFeedbackData {
                        known_default_feedbacks: data.known_default_feedbacks.clone(),
                        surface: Some(surface.downgrade()),
                    },
                );

                let surface_feedback = compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(SurfaceDmabufFeedbackState::default);
                    let feedback_state = states.data_map.get::<SurfaceDmabufFeedbackState>().unwrap();
                    feedback_state.add_instance(&feedback, || {
                        state
                            .new_surface_feedback(&surface, &DmabufGlobal { id: data.id })
                            .unwrap_or_else(|| {
                                data.default_feedback.as_ref().unwrap().lock().unwrap().clone()
                            })
                    })
                });

                surface_feedback.send(&feedback);
            }

            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, DmabufFeedbackData, D>
    for DmabufState
where
    D: Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufData>
        + Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
        + Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, DmabufFeedbackData>
        + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        resource: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        request: <zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1 as Resource>::Request,
        data: &DmabufFeedbackData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_linux_dmabuf_feedback_v1::Request::Destroy => {
                data.known_default_feedbacks
                    .lock()
                    .unwrap()
                    .retain(|feedback| feedback != resource);

                if let Some(surface) = data.surface.as_ref().and_then(|s| s.upgrade().ok()) {
                    compositor::with_states(&surface, |states| {
                        if let Some(surface_state) = states.data_map.get::<SurfaceDmabufFeedbackState>() {
                            surface_state.remove_instance(resource);
                        }
                    })
                }
            }
            _ => unreachable!(),
        }
    }
}

impl<D> GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData, D> for DmabufState
where
    D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
        + Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufData>
        + Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
        + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
        global_data: &DmabufGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let data = DmabufData {
            formats: global_data.formats.clone(),
            id: global_data.id,
            default_feedback: global_data.default_feedback.clone(),
            known_default_feedbacks: global_data.known_default_feedbacks.clone(),
        };

        let zwp_dmabuf = data_init.init(resource, data);

        // Immediately send format info to the client if we are the correct version.
        //
        // These events are deprecated in version 4 of the protocol.
        if zwp_dmabuf.version() < zwp_linux_dmabuf_v1::REQ_GET_DEFAULT_FEEDBACK_SINCE {
            for (fourcc, modifiers) in &*global_data.formats {
                // Modifier support got added in version 3
                if zwp_dmabuf.version() < zwp_linux_dmabuf_v1::EVT_MODIFIER_SINCE {
                    if modifiers.contains(&Modifier::Invalid) || modifiers.contains(&Modifier::Linear) {
                        zwp_dmabuf.format(*fourcc as u32);
                    }
                    continue;
                }

                for modifier in modifiers {
                    let modifier_hi = (Into::<u64>::into(*modifier) >> 32) as u32;
                    let modifier_lo = Into::<u64>::into(*modifier) as u32;
                    zwp_dmabuf.modifier(*fourcc as u32, modifier_hi, modifier_lo);
                }
            }
        }
    }

    fn can_view(client: Client, global_data: &DmabufGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData, D> for DmabufState
where
    D: Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
        + Dispatch<wl_buffer::WlBuffer, Dmabuf>
        + BufferHandler
        + DmabufHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
        data: &DmabufParamsData,
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_linux_buffer_params_v1::Request::Destroy => {}

            zwp_linux_buffer_params_v1::Request::Add {
                fd,
                plane_idx,
                offset,
                stride,
                modifier_hi,
                modifier_lo,
            } => {
                if !data.ensure_unused(params) {
                    return;
                }

                // Plane index should not be too large
                if plane_idx as usize >= MAX_PLANES {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::PlaneIdx,
                        format!("Plane index {} is out of bounds", plane_idx),
                    );
                    return;
                }

                let mut planes = data.planes.lock().unwrap();

                // Is the index already set?
                if planes.iter().any(|plane| plane.plane_idx == plane_idx) {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::PlaneSet,
                        format!("Plane index {} is already set.", plane_idx),
                    );
                    return;
                }

                planes.push(Plane {
                    fd,
                    plane_idx,
                    offset,
                    stride,
                });

                let modifier = Modifier::from(((modifier_hi as u64) << 32) + (modifier_lo as u64));
                let mut data_modifier = data.modifier.lock().unwrap();
                if let Some(data_modifier) = *data_modifier {
                    if params.version() >= 5 && modifier != data_modifier {
                        params.post_error(
                            zwp_linux_buffer_params_v1::Error::InvalidFormat,
                            format!(
                                "Planes have non-matching modifiers: {:?} != {:?}",
                                modifier, data_modifier
                            ),
                        );
                    }
                } else {
                    *data_modifier = Some(modifier);
                }
            }

            zwp_linux_buffer_params_v1::Request::Create {
                width,
                height,
                format,
                flags,
            } => {
                // create_dmabuf performs an implicit ensure_unused function call.
                if let Some(dmabuf) = data.create_dmabuf(params, width, height, format, flags, None) {
                    if state.dmabuf_state().globals.get(&data.id).is_some() {
                        let notifier = ImportNotifier::new(
                            params.clone(),
                            dh.clone(),
                            dmabuf.clone(),
                            Import::Falliable,
                        );
                        state.dmabuf_imported(&DmabufGlobal { id: data.id }, dmabuf, notifier);
                    } else {
                        // If the dmabuf global was destroyed, we cannot import any buffers.
                        params.failed();
                    }
                }
            }

            zwp_linux_buffer_params_v1::Request::CreateImmed {
                buffer_id,
                width,
                height,
                format,
                flags,
            } => {
                // Client is killed if the if statement is not taken.
                // create_dmabuf performs an implicit ensure_unused function call.
                if let Some(dmabuf) = data.create_dmabuf(params, width, height, format, flags, None) {
                    if state.dmabuf_state().globals.get(&data.id).is_some() {
                        // The buffer isn't technically valid during data_init, but the client is not allowed to use the buffer until ready.
                        let buffer = data_init.init(buffer_id, dmabuf.clone());
                        let notifier = ImportNotifier::new(
                            params.clone(),
                            dh.clone(),
                            dmabuf.clone(),
                            Import::Infallible(buffer),
                        );
                        state.dmabuf_imported(&DmabufGlobal { id: data.id }, dmabuf, notifier);
                    } else {
                        // Buffer import failed. The protocol documentation heavily implies killing the
                        // client is the right thing to do here.
                        params.post_error(
                            zwp_linux_buffer_params_v1::Error::InvalidWlBuffer,
                            "dmabuf global was destroyed on server",
                        );
                    }
                }
            }

            _ => unreachable!(),
        }
    }
}
