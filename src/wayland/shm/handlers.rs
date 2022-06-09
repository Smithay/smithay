use crate::wayland::{buffer::BufferHandler, shm::ShmBufferUserData};

use super::{
    pool::{Pool, ResizeError},
    BufferData, ShmPoolUserData, ShmState,
};

use std::sync::Arc;
use wayland_server::{
    protocol::{
        wl_buffer,
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
    },
    DataInit, DelegateDispatch, DelegateGlobalDispatch, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource, WEnum,
};

impl<D> DelegateGlobalDispatch<WlShm, (), D> for ShmState
where
    D: GlobalDispatch<WlShm, ()>,
    D: Dispatch<WlShm, ()>,
    D: Dispatch<WlShmPool, ShmPoolUserData>,
    D: AsRef<ShmState>,
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
        for &f in &state.as_ref().formats[..] {
            shm.format(f);
        }
    }
}

impl<D> DelegateDispatch<WlShm, (), D> for ShmState
where
    D: Dispatch<WlShm, ()> + Dispatch<WlShmPool, ShmPoolUserData> + AsRef<ShmState> + 'static,
{
    fn request(
        state: &mut D,
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

        let mmap_pool = match Pool::new(fd, size as usize, state.as_ref().log.clone()) {
            Ok(p) => p,
            Err(()) => {
                shm.post_error(wl_shm::Error::InvalidFd, format!("Failed to mmap fd {}", fd));
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

impl<D> DelegateDispatch<WlShmPool, ShmPoolUserData, D> for ShmState
where
    D: Dispatch<WlShmPool, ShmPoolUserData>
        + Dispatch<wl_buffer::WlBuffer, ShmBufferUserData>
        + BufferHandler
        + AsRef<ShmState>
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
                } else if stride < width {
                    Some(format!(
                        "width must not be larger than stride (width {}, stride {})",
                        width, stride
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
                        if !state.as_ref().formats.contains(&format) {
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
                if let Err(err) = arc_pool.resize(size) {
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

impl<D> DelegateDispatch<wl_buffer::WlBuffer, ShmBufferUserData, D> for ShmState
where
    D: Dispatch<wl_buffer::WlBuffer, ShmBufferUserData> + BufferHandler,
{
    fn request(
        data: &mut D,
        _client: &wayland_server::Client,
        buffer: &wl_buffer::WlBuffer,
        request: wl_buffer::Request,
        _udata: &ShmBufferUserData,
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
