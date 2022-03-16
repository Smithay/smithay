use std::sync::{atomic::AtomicBool, Mutex};

use wayland_protocols::unstable::linux_dmabuf::v1::server::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};
use wayland_server::{
    Client, DataInit, DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch,
    DelegateGlobalDispatchBase, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::{
    backend::allocator::dmabuf::{Plane, MAX_PLANES},
    wayland::buffer::{Buffer, BufferHandler},
};

use super::{
    DmabufData, DmabufGlobal, DmabufGlobalData, DmabufHandler, DmabufParamsData, DmabufState, ImportError,
    Modifier,
};

impl DelegateDispatchBase<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1> for DmabufState {
    type UserData = DmabufData;
}

impl<D> DelegateDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, D> for DmabufState
where
    D: Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, UserData = Self::UserData>
        + Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, UserData = DmabufParamsData>
        + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        request: zwp_linux_dmabuf_v1::Request,
        data: &Self::UserData,
        _dh: &mut DisplayHandle<'_>,
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
                        planes: Mutex::new(Vec::with_capacity(MAX_PLANES)),
                        logger: data.logger.clone(),
                    },
                );
            }

            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id: _ } => unimplemented!("v4"),

            zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id: _, surface: _ } => unimplemented!("v4"),

            _ => unreachable!(),
        }
    }
}

impl DelegateGlobalDispatchBase<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1> for DmabufState {
    type GlobalData = DmabufGlobalData;
}

impl<D> DelegateGlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, D> for DmabufState
where
    D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, GlobalData = Self::GlobalData>
        + Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, UserData = Self::UserData>
        + Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, UserData = DmabufParamsData>
        + 'static,
{
    fn bind(
        _state: &mut D,
        dh: &mut DisplayHandle<'_>,
        _client: &Client,
        resource: New<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let data = DmabufData {
            formats: global_data.formats.clone(),
            id: global_data.id,
            logger: global_data.logger.clone(),
        };

        let zwp_dmabuf = data_init.init(resource, data);

        // Immediately send format info to the client if we are the correct version.
        //
        // These events are deprecated in version 4 of the protocol.
        if zwp_dmabuf.version() <= 3 {
            for format in &*global_data.formats {
                zwp_dmabuf.format(dh, format.code as u32);

                if zwp_dmabuf.version() == 3 {
                    let modifier_hi = (Into::<u64>::into(format.modifier) >> 32) as u32;
                    let modifier_lo = Into::<u64>::into(format.modifier) as u32;

                    zwp_dmabuf.modifier(dh, format.code as u32, modifier_hi, modifier_lo);
                }
            }
        }
    }

    fn can_view(client: Client, global_data: &Self::GlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl DelegateDispatchBase<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1> for DmabufState {
    type UserData = DmabufParamsData;
}

impl<D> DelegateDispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, D> for DmabufState
where
    D: Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, UserData = Self::UserData>
        + BufferHandler
        + DmabufHandler,
{
    fn request(
        state: &mut D,
        client: &Client,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
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
                if !data.ensure_unused(dh, params) {
                    return;
                }

                // Plane index should not be too large
                if plane_idx as usize >= MAX_PLANES {
                    params.post_error(
                        dh,
                        zwp_linux_buffer_params_v1::Error::PlaneIdx,
                        format!("Plane index {} is out of bounds", plane_idx),
                    );
                    return;
                }

                let mut planes = data.planes.lock().unwrap();

                // Is the index already set?
                if planes.iter().any(|plane| plane.plane_idx == plane_idx) {
                    params.post_error(
                        dh,
                        zwp_linux_buffer_params_v1::Error::PlaneSet,
                        format!("Plane index {} is already set.", plane_idx),
                    );
                    return;
                }

                let modifier = ((modifier_hi as u64) << 32) + (modifier_lo as u64);
                planes.push(Plane {
                    fd: Some(fd),
                    plane_idx,
                    offset,
                    stride,
                    modifier: Modifier::from(modifier),
                });
            }

            zwp_linux_buffer_params_v1::Request::Create {
                width,
                height,
                format,
                flags,
            } => {
                // create_dmabuf performs an implicit ensure_unused function call.
                if let Some(dmabuf) = data.create_dmabuf(dh, params, width, height, format, flags) {
                    if state.dmabuf_state().globals.get(&data.id).is_some() {
                        match state.dmabuf_imported(&DmabufGlobal { id: data.id }, dmabuf.clone()) {
                            Ok(_) => {
                                match Buffer::create_buffer::<D, _>(dh, client, dmabuf) {
                                    Ok((wl_buffer, _)) => {
                                        params.created(dh, &wl_buffer);
                                    }

                                    Err(_) => {
                                        slog::error!(
                                            data.logger,
                                            "failed to create protocol object for \"create\" request"
                                        );
                                        // Failed to import since the buffer protocol object could not be created.
                                        params.failed(dh);
                                    }
                                }
                            }

                            Err(ImportError::InvalidFormat) => {
                                params.post_error(
                                    dh,
                                    zwp_linux_buffer_params_v1::Error::InvalidFormat,
                                    "format and plane combination are not valid",
                                );
                            }

                            Err(ImportError::Failed) => {
                                params.failed(dh);
                            }
                        }
                    } else {
                        // If the dmabuf global was destroyed, we cannot import any buffers.
                        params.failed(dh);
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
                if let Some(dmabuf) = data.create_dmabuf(dh, params, width, height, format, flags) {
                    if state.dmabuf_state().globals.get(&data.id).is_some() {
                        match state.dmabuf_imported(&DmabufGlobal { id: data.id }, dmabuf.clone()) {
                            Ok(_) => {
                                // Import was successful, initialize the dmabuf data
                                Buffer::init_buffer(data_init, buffer_id, dmabuf);
                            }

                            Err(ImportError::InvalidFormat) => {
                                params.post_error(
                                    dh,
                                    zwp_linux_buffer_params_v1::Error::InvalidFormat,
                                    "format and plane combination are not valid",
                                );
                            }

                            Err(ImportError::Failed) => {
                                // Buffer import failed. The protocol documentation heavily implies killing the
                                // client is the right thing to do here.
                                params.post_error(
                                    dh,
                                    zwp_linux_buffer_params_v1::Error::InvalidWlBuffer,
                                    "buffer import failed",
                                );
                            }
                        }
                    } else {
                        // Buffer import failed. The protocol documentation heavily implies killing the
                        // client is the right thing to do here.
                        params.post_error(
                            dh,
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
