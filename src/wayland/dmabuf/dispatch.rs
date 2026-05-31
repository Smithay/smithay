use std::sync::{Arc, Mutex, atomic::AtomicBool};

use wayland_protocols::wp::linux_dmabuf::zv1::server::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, New, Resource, backend::ClientId, protocol::wl_buffer,
};

use crate::{
    backend::allocator::dmabuf::{Dmabuf, MAX_PLANES, Plane},
    wayland::{
        Dispatch2, GlobalDispatch2, buffer::BufferHandler, compositor,
        dmabuf::SurfaceDmabufFeedbackStateInner,
    },
};

use super::{
    DmabufData, DmabufFeedbackData, DmabufGlobal, DmabufGlobalData, DmabufHandler, DmabufParamsData, Import,
    ImportNotifier, Modifier, SurfaceDmabufFeedbackState,
};

impl<D> Dispatch2<wl_buffer::WlBuffer, D> for Dmabuf
where
    D: BufferHandler,
{
    fn request(
        &self,
        _data: &mut D,
        _client: &Client,
        _buffer: &wl_buffer::WlBuffer,
        request: wl_buffer::Request,
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

    fn destroyed(&self, data: &mut D, _client: ClientId, buffer: &wl_buffer::WlBuffer) {
        data.buffer_destroyed(buffer);
    }
}

impl<D> Dispatch2<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, D> for DmabufData
where
    D: Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
        + Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, DmabufFeedbackData>
        + DmabufHandler
        + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _resource: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        request: zwp_linux_dmabuf_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_linux_dmabuf_v1::Request::Destroy => {}

            zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                data_init.init(
                    params_id,
                    DmabufParamsData {
                        id: self.id,
                        used: AtomicBool::new(false),
                        formats: self.formats.clone(),
                        modifier: Mutex::new(None),
                        planes: Mutex::new(Vec::with_capacity(MAX_PLANES)),
                    },
                );
            }

            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id } => {
                let feedback = data_init.init(
                    id,
                    DmabufFeedbackData {
                        known_default_feedbacks: self.known_default_feedbacks.clone(),
                        surface: None,
                    },
                );

                self.known_default_feedbacks
                    .lock()
                    .unwrap()
                    .push(feedback.downgrade());

                self.default_feedback
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
                        known_default_feedbacks: self.known_default_feedbacks.clone(),
                        surface: Some(surface.downgrade()),
                    },
                );

                if compositor::with_states(&surface, |states| {
                    states.data_map.get::<SurfaceDmabufFeedbackState>().is_none()
                }) {
                    let new_feedback = state
                        .new_surface_feedback(&surface, &DmabufGlobal { id: self.id })
                        .unwrap_or_else(|| self.default_feedback.as_ref().unwrap().lock().unwrap().clone());
                    compositor::with_states(&surface, |states| {
                        states
                            .data_map
                            .insert_if_missing_threadsafe(|| SurfaceDmabufFeedbackState {
                                inner: Arc::new(Mutex::new(SurfaceDmabufFeedbackStateInner {
                                    feedback: new_feedback,
                                    known_instances: Vec::new(),
                                })),
                            });
                    });
                }

                let surface_feedback = compositor::with_states(&surface, |states| {
                    let feedback_state = states.data_map.get::<SurfaceDmabufFeedbackState>().unwrap();
                    feedback_state.add_instance(&feedback)
                });

                surface_feedback.send(&feedback);
            }

            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch2<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, D> for DmabufFeedbackData {
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        resource: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        request: <zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_linux_dmabuf_feedback_v1::Request::Destroy => {
                self.known_default_feedbacks
                    .lock()
                    .unwrap()
                    .retain(|feedback| feedback != resource);

                if let Some(surface) = self.surface.as_ref().and_then(|s| s.upgrade().ok()) {
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

impl<D> GlobalDispatch2<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, D> for DmabufGlobalData
where
    D: Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufData> + 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let data = DmabufData {
            formats: self.formats.clone(),
            id: self.id,
            default_feedback: self.default_feedback.clone(),
            known_default_feedbacks: self.known_default_feedbacks.clone(),
        };

        let zwp_dmabuf = data_init.init(resource, data);

        // Immediately send format info to the client if we are the correct version.
        //
        // These events are deprecated in version 4 of the protocol.
        if zwp_dmabuf.version() < zwp_linux_dmabuf_v1::REQ_GET_DEFAULT_FEEDBACK_SINCE {
            for (fourcc, modifiers) in &*self.formats {
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

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, D> for DmabufParamsData
where
    D: Dispatch<wl_buffer::WlBuffer, Dmabuf> + BufferHandler + DmabufHandler,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
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
                if !self.ensure_unused(params) {
                    return;
                }

                // Plane index should not be too large
                if plane_idx as usize >= MAX_PLANES {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::PlaneIdx,
                        format!("Plane index {plane_idx} is out of bounds"),
                    );
                    return;
                }

                let mut planes = self.planes.lock().unwrap();

                // Is the index already set?
                if planes.iter().any(|plane| plane.plane_idx == plane_idx) {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::PlaneSet,
                        format!("Plane index {plane_idx} is already set."),
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
                let mut data_modifier = self.modifier.lock().unwrap();
                if let Some(data_modifier) = *data_modifier {
                    if params.version() >= 5 && modifier != data_modifier {
                        params.post_error(
                            zwp_linux_buffer_params_v1::Error::InvalidFormat,
                            format!("Planes have non-matching modifiers: {modifier:?} != {data_modifier:?}",),
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
                if let Some(dmabuf) = self.create_dmabuf(params, width, height, format, flags, None) {
                    if state.dmabuf_state().globals.contains_key(&self.id) {
                        let notifier = ImportNotifier::new(
                            params.clone(),
                            dh.clone(),
                            dmabuf.clone(),
                            Import::Falliable,
                        );
                        state.dmabuf_imported(&DmabufGlobal { id: self.id }, dmabuf, notifier);
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
                if let Some(dmabuf) = self.create_dmabuf(params, width, height, format, flags, None) {
                    if state.dmabuf_state().globals.contains_key(&self.id) {
                        // The buffer isn't technically valid during data_init, but the client is not allowed to use the buffer until ready.
                        let buffer = data_init.init(buffer_id, dmabuf.clone());
                        let notifier = ImportNotifier::new(
                            params.clone(),
                            dh.clone(),
                            dmabuf.clone(),
                            Import::Infallible(buffer),
                        );
                        state.dmabuf_imported(&DmabufGlobal { id: self.id }, dmabuf, notifier);
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
