//! Module for [dmabuf](https://01.org/linuxgraphics/gfx-docs/drm/driver-api/dma-buf.html) buffers.

use super::{Buffer, Format, Modifier};
use std::os::unix::io::RawFd;
use std::sync::{Arc, Weak};

const MAX_PLANES: usize = 4;

#[derive(Debug)]
pub(crate) struct DmabufInternal {
    pub num_planes: usize,
    pub offsets: [u32; MAX_PLANES],
    pub strides: [u32; MAX_PLANES],
    pub fds: [RawFd; MAX_PLANES],
    pub width: u32,
    pub height: u32,
    pub format: Format,
}

#[derive(Debug, Clone)]
/// Strong reference to a dmabuf handle
pub struct Dmabuf(pub(crate) Arc<DmabufInternal>);

#[derive(Debug, Clone)]
/// Weak reference to a dmabuf handle
pub struct WeakDmabuf(pub(crate) Weak<DmabufInternal>);

impl PartialEq for Dmabuf {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Dmabuf {}

impl PartialEq<WeakDmabuf> for Dmabuf {
    fn eq(&self, other: &WeakDmabuf) -> bool {
        if let Some(dmabuf) = other.upgrade() {
            return Arc::ptr_eq(&self.0, &dmabuf.0);
        }
        false
    }
}

impl PartialEq for WeakDmabuf {
    fn eq(&self, other: &Self) -> bool {
        if let Some(dmabuf) = self.upgrade() {
            return &dmabuf == other;
        }
        false
    }
}

impl Buffer for Dmabuf {
    fn width(&self) -> u32 {
        self.0.width
    }

    fn height(&self) -> u32 {
        self.0.height
    }

    fn format(&self) -> Format {
        self.0.format
    }
}

impl Dmabuf {
    pub(crate) fn new(
        src: &impl Buffer,
        planes: usize,
        offsets: &[u32],
        strides: &[u32],
        fds: &[RawFd],
    ) -> Option<Dmabuf> {
        if offsets.len() < planes
            || strides.len() < planes
            || fds.len() < planes
            || planes == 0
            || planes > MAX_PLANES
        {
            return None;
        }

        let end = [0u32, 0, 0];
        let end_fds = [0i32, 0, 0];
        let mut offsets = offsets.iter().take(planes).chain(end.iter());
        let mut strides = strides.iter().take(planes).chain(end.iter());
        let mut fds = fds.iter().take(planes).chain(end_fds.iter());

        Some(Dmabuf(Arc::new(DmabufInternal {
            num_planes: planes,
            offsets: [
                *offsets.next().unwrap(),
                *offsets.next().unwrap(),
                *offsets.next().unwrap(),
                *offsets.next().unwrap(),
            ],
            strides: [
                *strides.next().unwrap(),
                *strides.next().unwrap(),
                *strides.next().unwrap(),
                *strides.next().unwrap(),
            ],
            fds: [
                *fds.next().unwrap(),
                *fds.next().unwrap(),
                *fds.next().unwrap(),
                *fds.next().unwrap(),
            ],

            width: src.width(),
            height: src.height(),
            format: src.format(),
        })))
    }

    /// Return raw handles of the planes of this buffer
    pub fn handles(&self) -> &[RawFd] {
        self.0.fds.split_at(self.0.num_planes).0
    }

    /// Return offsets for the planes of this buffer
    pub fn offsets(&self) -> &[u32] {
        self.0.offsets.split_at(self.0.num_planes).0
    }

    /// Return strides for the planes of this buffer
    pub fn strides(&self) -> &[u32] {
        self.0.strides.split_at(self.0.num_planes).0
    }

    /// Check if this buffer format has any vendor-specific modifiers set or is implicit/linear
    pub fn has_modifier(&self) -> bool {
        self.0.format.modifier != Modifier::Invalid && self.0.format.modifier != Modifier::Linear
    }

    /// Create a weak reference to this dmabuf
    pub fn weak(&self) -> WeakDmabuf {
        WeakDmabuf(Arc::downgrade(&self.0))
    }
}

impl WeakDmabuf {
    /// Try to upgrade to a strong reference of this buffer.
    ///
    /// Fails if no strong references exist anymore and the handle was already closed.
    pub fn upgrade(&self) -> Option<Dmabuf> {
        self.0.upgrade().map(Dmabuf)
    }
}

impl Drop for DmabufInternal {
    fn drop(&mut self) {
        for fd in self.fds.iter() {
            if *fd != 0 {
                let _ = nix::unistd::close(*fd);
            }
        }
    }
}

/// Buffer that can be exported as Dmabufs
pub trait AsDmabuf {
    /// Error type returned, if exporting fails
    type Error;

    /// Export this buffer as a new Dmabuf
    fn export(&self) -> Result<Dmabuf, Self::Error>;
}

impl AsDmabuf for Dmabuf {
    type Error = ();

    fn export(&self) -> Result<Dmabuf, ()> {
        Ok(self.clone())
    }
}
