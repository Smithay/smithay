use super::{
    pool::{Pool, ResizeError},
    BufferData, ShmState,
};

use std::sync::Arc;
use wayland_server::{
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
    },
    DataInit, DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase,
    Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

impl DelegateGlobalDispatchBase<WlShm> for ShmState {
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<WlShm, D> for ShmState
where
    D: GlobalDispatch<WlShm, GlobalData = ()>,
    D: Dispatch<WlShm, UserData = ()>,
    D: Dispatch<WlShmPool, UserData = ShmPoolUserData>,
    D: AsRef<ShmState>,
    D: 'static,
{
    fn bind(
        state: &mut D,
        handle: &mut wayland_server::DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: New<WlShm>,
        _global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let shm = data_init.init(resource, ());

        // send the formats
        for &f in &state.as_ref().formats[..] {
            shm.format(handle, f);
        }
    }
}

impl DelegateDispatchBase<WlShm> for ShmState {
    type UserData = ();
}

impl<D> DelegateDispatch<WlShm, D> for ShmState
where
    D: Dispatch<WlShm, UserData = ()>
        + Dispatch<WlShmPool, UserData = ShmPoolUserData>
        + AsRef<ShmState>
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        shm: &WlShm,
        request: wl_shm::Request,
        _data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
        data_init: &mut DataInit<'_, D>,
    ) {
        use wl_shm::{Error, Request};

        let (pool, fd, size) = match request {
            Request::CreatePool { id: pool, fd, size } => (pool, fd, size),
            _ => unreachable!(),
        };

        if size <= 0 {
            shm.post_error(dh, Error::InvalidStride, "invalid wl_shm_pool size");
            return;
        }

        let mmap_pool = match Pool::new(fd, size as usize, state.as_ref().log.clone()) {
            Ok(p) => p,
            Err(()) => {
                shm.post_error(dh, wl_shm::Error::InvalidFd, format!("Failed to mmap fd {}", fd));
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

/// User data of WlShmPool
#[derive(Debug)]
pub struct ShmPoolUserData {
    inner: Arc<Pool>,
}

impl DelegateDispatchBase<WlShmPool> for ShmState {
    type UserData = ShmPoolUserData;
}

impl<D> DelegateDispatch<WlShmPool, D> for ShmState
where
    D: Dispatch<WlShmPool, UserData = ShmPoolUserData>
        + Dispatch<WlBuffer, UserData = ShmBufferUserData>
        + AsRef<ShmState>
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        pool: &WlShmPool,
        request: wl_shm_pool::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
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
                    pool.post_error(dh, wl_shm::Error::InvalidStride, message);
                    return;
                }

                match format {
                    WEnum::Value(format) => {
                        if !state.as_ref().formats.contains(&format) {
                            pool.post_error(
                                dh,
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
                            dh,
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
                            pool.post_error(dh, wl_shm::Error::InvalidFd, "cannot shrink wl_shm_pool");
                        }

                        ResizeError::MremapFailed => {
                            pool.post_error(dh, wl_shm::Error::InvalidFd, "mremap failed");
                        }
                    }
                }
            }

            Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}

/*
 * wl_buffer
 */

/// User data of shm WlBuffer
#[derive(Debug)]
pub struct ShmBufferUserData {
    pub(crate) pool: Arc<Pool>,
    pub(crate) data: BufferData,
}

impl DelegateDispatchBase<WlBuffer> for ShmState {
    type UserData = ShmBufferUserData;
}

impl<D> DelegateDispatch<WlBuffer, D> for ShmState
where
    D: Dispatch<WlBuffer, UserData = ShmBufferUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _pool: &WlBuffer,
        _request: wl_buffer::Request,
        _data: &Self::UserData,
        _dh: &mut DisplayHandle<'_>,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }
}
