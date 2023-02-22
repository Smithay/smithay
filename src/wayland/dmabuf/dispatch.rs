use std::sync::{atomic::AtomicBool, Mutex};

use tracing::error;

use wayland_protocols::wp::linux_dmabuf::zv1::server::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1};
use wayland_server::{
    protocol::wl_buffer, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::{
    backend::allocator::dmabuf::{Dmabuf, Plane, MAX_PLANES},
    wayland::buffer::BufferHandler,
};

use super::{
    DmabufData, DmabufGlobal, DmabufGlobalData, DmabufHandler, DmabufParamsData, DmabufState, ImportError,
    Modifier,
};

impl<D> Dispatch<wl_buffer::WlBuffer, Dmabuf, D> for DmabufState
where
    D: Dispatch<wl_buffer::WlBuffer, Dmabuf> + BufferHandler,
{
    fn request(
        data: &mut D,
        _client: &Client,
        buffer: &wl_buffer::WlBuffer,
        request: wl_buffer::Request,
        _udata: &Dmabuf,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_buffer::Request::Destroy => {
                data.buffer_destroyed(buffer);
            }

            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufData, D> for DmabufState
where
    D: Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufData>
        + Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
        + 'static,
{
    fn request(
        _state: &mut D,
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
                        planes: Mutex::new(Vec::with_capacity(MAX_PLANES)),
                    },
                );
            }

            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id: _ } => unimplemented!("v4"),

            zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id: _, surface: _ } => unimplemented!("v4"),

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
        };

        let zwp_dmabuf = data_init.init(resource, data);

        // Immediately send format info to the client if we are the correct version.
        //
        // These events are deprecated in version 4 of the protocol.
        if zwp_dmabuf.version() <= 3 {
            for format in &*global_data.formats {
                zwp_dmabuf.format(format.code as u32);

                if zwp_dmabuf.version() == 3 {
                    let modifier_hi = (Into::<u64>::into(format.modifier) >> 32) as u32;
                    let modifier_lo = Into::<u64>::into(format.modifier) as u32;

                    zwp_dmabuf.modifier(format.code as u32, modifier_hi, modifier_lo);
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
        client: &Client,
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

                let modifier = ((modifier_hi as u64) << 32) + (modifier_lo as u64);
                planes.push(Plane {
                    fd,
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
                if let Some(dmabuf) = data.create_dmabuf(params, width, height, format, flags) {
                    if state.dmabuf_state().globals.get(&data.id).is_some() {
                        match state.dmabuf_imported(&DmabufGlobal { id: data.id }, dmabuf.clone()) {
                            Ok(_) => {
                                match client.create_resource::<wl_buffer::WlBuffer, Dmabuf, D>(dh, 1, dmabuf)
                                {
                                    Ok(buffer) => {
                                        params.created(&buffer);
                                    }

                                    Err(_) => {
                                        error!("failed to create protocol object for \"create\" request");
                                        // Failed to import since the buffer protocol object could not be created.
                                        params.failed();
                                    }
                                }
                            }

                            Err(ImportError::InvalidFormat) => {
                                params.post_error(
                                    zwp_linux_buffer_params_v1::Error::InvalidFormat,
                                    "format and plane combination are not valid",
                                );
                            }

                            Err(ImportError::Failed) => {
                                params.failed();
                            }
                        }
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
                if let Some(dmabuf) = data.create_dmabuf(params, width, height, format, flags) {
                    if state.dmabuf_state().globals.get(&data.id).is_some() {
                        match state.dmabuf_imported(&DmabufGlobal { id: data.id }, dmabuf.clone()) {
                            Ok(_) => {
                                // Import was successful, initialize the dmabuf data
                                data_init.init(buffer_id, dmabuf);
                            }

                            Err(ImportError::InvalidFormat) => {
                                params.post_error(
                                    zwp_linux_buffer_params_v1::Error::InvalidFormat,
                                    "format and plane combination are not valid",
                                );
                            }

                            Err(ImportError::Failed) => {
                                // Buffer import failed. The protocol documentation heavily implies killing the
                                // client is the right thing to do here.
                                params.post_error(
                                    zwp_linux_buffer_params_v1::Error::InvalidWlBuffer,
                                    "buffer import failed",
                                );
                            }
                        }
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
