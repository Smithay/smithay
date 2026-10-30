//! Shm buffers direct importing through udmabuf without copying.

use std::io;

use wayland_server::{Resource, protocol::wl_buffer};

use crate::backend::allocator::dmabuf::{Dmabuf, DmabufSyncFlags};

use super::{BufferData, ShmBufferUserData};

#[derive(Debug, Default)]
pub(super) enum UdmabufState {
    #[default]
    NotTried,
    Failed,
    Imported(Dmabuf),
    /// A renderer failed to import it. Kept, as another renderer may have imported it
    /// earlier and still read it.
    Rejected(Dmabuf),
}

/// Udmabufs kept around for new buffers.
const POOL_UDMABUFS: usize = 4;

/// Udmabufs created in a single pool, reused by new buffers with the same layout,
/// as some clients (e.g. Firefox, sctk) create a new `wl_buffer` every frame.
///
/// `None` indicates a failed layout import, new buffers will be immediately copied.
#[derive(Debug, Default)]
pub(crate) struct PoolUdmabufs(Vec<(BufferData, Option<Dmabuf>)>);

impl PoolUdmabufs {
    fn get_or_create(&mut self, user_data: &ShmBufferUserData) -> io::Result<Dmabuf> {
        let dmabuf = match self.0.iter().position(|(data, _)| *data == user_data.data) {
            Some(idx) => self.0.remove(idx).1,
            None => {
                let dmabuf = create_udmabuf(user_data)?;
                if self.0.len() == POOL_UDMABUFS {
                    self.0.remove(0);
                }
                Some(dmabuf)
            }
        };
        self.0.push((user_data.data, dmabuf.clone()));
        dmabuf.ok_or_else(|| io::Error::other("a renderer failed to import this layout before"))
    }

    fn reject(&mut self, dmabuf: &Dmabuf) {
        if let Some((_, cached)) = self
            .0
            .iter_mut()
            .find(|(_, cached)| cached.as_ref() == Some(dmabuf))
        {
            *cached = None;
        }
    }
}

/// Returns a [`Dmabuf`] backed by the same memory as this shm buffer, created via udmabuf.
///
/// The result is stored in the buffer's user data, failures as well, and reused through [`PoolUdmabufs`].
///
/// Also makes the client's writes visible to the GPU, so it must be called on every import.
pub(crate) fn try_udmabuf(buffer: &wl_buffer::WlBuffer) -> Option<Dmabuf> {
    let user_data = buffer.data::<ShmBufferUserData>()?;
    // Keep the lock while creating, so concurrent imports (e.g. from several render threads)
    // share one udmabuf: syncs only cover the udmabuf they're called on.
    let mut state = user_data.udmabuf.lock().unwrap();
    if matches!(*state, UdmabufState::NotTried) {
        let udmabuf = user_data.pool.udmabufs().lock().unwrap().get_or_create(user_data);
        *state = match udmabuf {
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

/// Marks the buffer and its layout in the pool as not importable via udmabuf.
pub(crate) fn mark_udmabuf_failed(buffer: &wl_buffer::WlBuffer) {
    if let Some(user_data) = buffer.data::<ShmBufferUserData>() {
        let mut state = user_data.udmabuf.lock().unwrap();
        *state = match std::mem::take(&mut *state) {
            UdmabufState::Imported(dmabuf) | UdmabufState::Rejected(dmabuf) => {
                user_data.pool.udmabufs().lock().unwrap().reject(&dmabuf);
                UdmabufState::Rejected(dmabuf)
            }
            UdmabufState::NotTried | UdmabufState::Failed => UdmabufState::Failed,
        };
    }
}

/// Blocks until the GPU is done reading the udmabuf of this shm buffer.
///
/// DrmCompositor already keeps these buffers alive until its frames are presented,
/// this covers rendering outside of it (e.g. screencasting).
pub(crate) fn wait_udmabuf_idle(buffer: &wl_buffer::WlBuffer) {
    let Some(user_data) = buffer.data::<ShmBufferUserData>() else {
        return;
    };
    let dmabuf = match &*user_data.udmabuf.lock().unwrap() {
        UdmabufState::Imported(dmabuf) | UdmabufState::Rejected(dmabuf) => dmabuf.clone(),
        UdmabufState::NotTried | UdmabufState::Failed => return,
    };
    if let Err(err) = dmabuf.sync_plane(0, DmabufSyncFlags::START | DmabufSyncFlags::WRITE) {
        tracing::warn!(?err, "Failed to wait for GPU reads of udmabuf");
    }
}

/// Whether the GPU reads this shm buffer's memory directly through udmabuf.
#[cfg(feature = "backend_gbm")]
pub(crate) fn is_udmabuf_backed(buffer: &wl_buffer::WlBuffer) -> bool {
    buffer.data::<ShmBufferUserData>().is_some_and(|data| {
        matches!(
            *data.udmabuf.lock().unwrap(),
            UdmabufState::Imported(_) | UdmabufState::Rejected(_)
        )
    })
}
