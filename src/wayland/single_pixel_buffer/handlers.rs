use crate::wayland::{GlobalData, buffer::BufferHandler};

use super::SinglePixelBufferUserData;
use wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::{
    self, WpSinglePixelBufferManagerV1,
};
use wayland_server::{
    DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    protocol::wl_buffer::{self, WlBuffer},
};

impl<D> GlobalDispatch<WpSinglePixelBufferManagerV1, D> for GlobalData
where
    D: BufferHandler,
{
    fn bind(
        &self,
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WpSinglePixelBufferManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D> Dispatch<WpSinglePixelBufferManagerV1, D> for GlobalData
where
    D: BufferHandler,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        _manager: &WpSinglePixelBufferManagerV1,
        request: wp_single_pixel_buffer_manager_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_single_pixel_buffer_manager_v1::Request::CreateU32RgbaBuffer {
                id: buffer,
                r,
                g,
                b,
                a,
            } => {
                data_init.init(buffer, SinglePixelBufferUserData { r, g, b, a });
            }
            wp_single_pixel_buffer_manager_v1::Request::Destroy => {}
            _ => todo!(),
        }
    }
}

impl<D> Dispatch<WlBuffer, D> for SinglePixelBufferUserData
where
    D: BufferHandler,
{
    fn request(
        &self,
        data: &mut D,
        _client: &wayland_server::Client,
        buffer: &wl_buffer::WlBuffer,
        request: wl_buffer::Request,
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
