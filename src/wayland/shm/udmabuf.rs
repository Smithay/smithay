//! Shm buffers direct importing through udmabuf without copying.

use std::io;

use wayland_server::{Resource, protocol::wl_buffer};

use crate::backend::allocator::dmabuf::{Dmabuf, DmabufSyncFlags};

use super::ShmBufferUserData;

#[derive(Debug, Default)]
pub(super) enum UdmabufState {
    #[default]
    NotTried,
    Failed,
    Imported(Dmabuf),
}

/// Returns a [`Dmabuf`] backed by the same memory as this shm buffer, created via udmabuf.
///
/// The result is stored in the buffer's user data, failures as well.
///
/// Also makes the client's writes visible to the GPU, so it must be called on every import.
pub(crate) fn try_udmabuf(buffer: &wl_buffer::WlBuffer) -> Option<Dmabuf> {
    let user_data = buffer.data::<ShmBufferUserData>()?;
    // Keep the lock while creating, so concurrent imports (e.g. from several render threads)
    // share one udmabuf: syncs only cover the udmabuf they're called on.
    let mut state = user_data.udmabuf.lock().unwrap();
    if matches!(*state, UdmabufState::NotTried) {
        *state = match create_udmabuf(user_data) {
            Ok(dmabuf) => UdmabufState::Imported(dmabuf),
            Err(err) => {
                tracing::trace!(
                    ?err,
                    width = user_data.data.width,
                    height = user_data.data.height,
                    stride = user_data.data.stride,
                    offset = user_data.data.offset,
                    "Failed to create udmabuf for shm buffer"
                );
                UdmabufState::Failed
            }
        };
    }
    let UdmabufState::Imported(dmabuf) = &*state else {
        return None;
    };
    let dmabuf = dmabuf.clone();
    drop(state);

    if let Err(err) = dmabuf.sync_plane(0, DmabufSyncFlags::END | DmabufSyncFlags::WRITE) {
        tracing::warn!(?err, "Failed to sync udmabuf before GPU access");
    }
    Some(dmabuf)
}

#[cfg(target_os = "linux")]
fn create_udmabuf(user_data: &ShmBufferUserData) -> io::Result<Dmabuf> {
    use super::shm_format_to_fourcc;
    use crate::backend::allocator::udmabuf::UdmabufAllocator;
    use std::sync::LazyLock;

    static UDMABUF: LazyLock<Option<UdmabufAllocator>> = LazyLock::new(|| {
        UdmabufAllocator::new()
            .inspect_err(|err| tracing::debug!(?err, "udmabuf is unavailable, shm buffers will be copied"))
            .ok()
    });
    let allocator = UDMABUF.as_ref().ok_or(io::ErrorKind::NotFound)?;

    let fourcc = shm_format_to_fourcc(user_data.data.format).ok_or(io::ErrorKind::Unsupported)?;
    let size = user_data.data.height as usize * user_data.data.stride as usize;
    let dmabuf = allocator.create_buffer_from_memfd(
        user_data.pool.fd(),
        user_data.data.offset as usize,
        size,
        fourcc,
        user_data.data.width as u32,
        user_data.data.height as u32,
        user_data.data.stride as u32,
    )?;
    // The kernel only accepts the END in `try_udmabuf` after a first START.
    dmabuf
        .sync_plane(0, DmabufSyncFlags::START | DmabufSyncFlags::WRITE)
        .map_err(io::Error::other)?;
    Ok(dmabuf)
}

#[cfg(not(target_os = "linux"))]
fn create_udmabuf(_user_data: &ShmBufferUserData) -> io::Result<Dmabuf> {
    Err(io::ErrorKind::Unsupported.into())
}

/// Marks the buffer as not importable via udmabuf.
pub(crate) fn mark_udmabuf_failed(buffer: &wl_buffer::WlBuffer) {
    if let Some(user_data) = buffer.data::<ShmBufferUserData>() {
        *user_data.udmabuf.lock().unwrap() = UdmabufState::Failed;
    }
}
