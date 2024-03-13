use crate::wayland::{
    buffer::BufferHandler,
    shm::{wl_bytes_per_pixel, ShmBufferUserData},
};

use super::{
    pool::{Pool, ResizeError},
    BufferData, ShmHandler, ShmPoolUserData, ShmState,
};

use std::{num::NonZeroUsize, os::unix::io::AsRawFd, sync::Arc};
use wayland_server::{
    backend::ClientId,
    protocol::{
        wl_buffer,
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
    },
    DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

impl<D> GlobalDispatch<WlShm, (), D> for ShmState
where
    D: GlobalDispatch<WlShm, ()>,
    D: Dispatch<WlShm, ()>,
    D: Dispatch<WlShmPool, ShmPoolUserData>,
    D: ShmHandler,
    D: 'static,
{
    fn bind(
        state: &mut D,
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WlShm>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let shm = data_init.init(resource, ());

        // send the formats
        for &f in &state.shm_state().formats {
            shm.format(f);
        }
    }
}

impl<D> Dispatch<WlShm, (), D> for ShmState
where
    D: Dispatch<WlShm, ()> + Dispatch<WlShmPool, ShmPoolUserData> + ShmHandler + 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        shm: &WlShm,
        request: wl_shm::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use wl_shm::{Error, Request};

        let (pool, fd, size) = match request {
            Request::CreatePool { id: pool, fd, size } => (pool, fd, size),
            _ => unreachable!(),
        };

        if size <= 0 {
            shm.post_error(Error::InvalidStride, "invalid wl_shm_pool size");
            return;
        }

        let mmap_pool = match Pool::new(fd, NonZeroUsize::try_from(size as usize).unwrap()) {
            Ok(p) => p,
            Err(fd) => {
                shm.post_error(
                    wl_shm::Error::InvalidFd,
                    format!("Failed to mmap fd {}", fd.as_raw_fd()),
                );
                return;
            }
        };

        data_init.init(
            pool,
            ShmPoolUserData {
                inner: Arc::new(mmap_pool),
            },
        );
    }
}

/*
 * wl_shm_pool
 */

impl<D> Dispatch<WlShmPool, ShmPoolUserData, D> for ShmState
where
    D: Dispatch<WlShmPool, ShmPoolUserData>
        + Dispatch<wl_buffer::WlBuffer, ShmBufferUserData>
        + BufferHandler
        + ShmHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        pool: &WlShmPool,
        request: wl_shm_pool::Request,
        data: &ShmPoolUserData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::wl_shm_pool::Request;

        let arc_pool = &data.inner;

        match request {
            Request::CreateBuffer {
                id: buffer,
                offset,
                width,
                height,
                stride,
                format,
            } => {
                // Validate client parameters
                let message = if offset < 0 {
                    Some("offset must not be negative".to_string())
                } else if width <= 0 || height <= 0 {
                    Some(format!("invalid width or height ({}x{})", width, height))
                } else if stride.checked_div(wl_bytes_per_pixel(format)).unwrap_or(0) < width {
                    // stride is in bytes...
                    Some(format!(
                        "width must not be larger than stride (width {}, stride {})",
                        width,
                        stride.checked_div(wl_bytes_per_pixel(format)).unwrap_or(0)
                    ))
                } else if (i32::MAX / stride) < height {
                    Some(format!(
                        "height is too large for stride (max {})",
                        i32::MAX / stride
                    ))
                } else if offset > arc_pool.size() as i32 - (stride * height) {
                    Some("offset is too large".to_string())
                } else {
                    None
                };

                if let Some(message) = message {
                    pool.post_error(wl_shm::Error::InvalidStride, message);
                    return;
                }

                match format {
                    WEnum::Value(format) => {
                        if !state.shm_state().formats.contains(&format) {
                            pool.post_error(
                                wl_shm::Error::InvalidFormat,
                                format!("format {:?} not supported", format),
                            );

                            return;
                        }

                        let data = ShmBufferUserData {
                            pool: arc_pool.clone(),
                            data: BufferData {
                                offset,
                                width,
                                height,
                                stride,
                                format,
                            },
                        };

                        data_init.init(buffer, data);
                    }

                    WEnum::Unknown(unknown) => {
                        pool.post_error(
                            wl_shm::Error::InvalidFormat,
                            format!("unknown format 0x{:x}", unknown),
                        );
                    }
                }
            }

            Request::Resize { size } => {
                if size <= 0 {
                    pool.post_error(wl_shm::Error::InvalidFd, "invalid wl_shm_pool size");
                }

                if let Err(err) = arc_pool.resize(NonZeroUsize::try_from(size as usize).unwrap()) {
                    match err {
                        ResizeError::InvalidSize => {
                            pool.post_error(wl_shm::Error::InvalidFd, "cannot shrink wl_shm_pool");
                        }

                        ResizeError::MremapFailed => {
                            pool.post_error(wl_shm::Error::InvalidFd, "mremap failed");
                        }
                    }
                }
            }

            Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<wl_buffer::WlBuffer, ShmBufferUserData, D> for ShmState
where
    D: Dispatch<wl_buffer::WlBuffer, ShmBufferUserData> + BufferHandler,
{
    fn request(
        _data: &mut D,
        _client: &wayland_server::Client,
        _buffer: &wl_buffer::WlBuffer,
        request: wl_buffer::Request,
        _udata: &ShmBufferUserData,
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

    fn destroyed(data: &mut D, _client: ClientId, buffer: &wl_buffer::WlBuffer, _udata: &ShmBufferUserData) {
        data.buffer_destroyed(buffer);
    }
}
