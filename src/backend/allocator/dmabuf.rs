use super::{Buffer, Format, Modifier};
use std::sync::{Arc, Weak};
use std::os::unix::io::RawFd;

const MAX_PLANES: usize = 4;

pub(crate) struct DmabufInternal {
    pub num_planes: usize,
    pub offsets: [u32; MAX_PLANES],
    pub strides: [u32; MAX_PLANES],
    pub fds: [RawFd; MAX_PLANES],
    pub width: u32,
    pub height: u32,
    pub format: Format,
}

#[derive(Clone)]
pub struct Dmabuf(Arc<DmabufInternal>);

#[derive(Clone)]
pub struct WeakDmabuf(Weak<DmabufInternal>);

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
    pub fn new(
        src: impl Buffer + 'static,

        planes: usize,
        offsets: &[u32],
        strides: &[u32],
        fds: &[RawFd],
    ) -> Option<Dmabuf> {
        if offsets.len() < planes
        || strides.len() < planes
        || fds.len() < planes
        || planes == 0 || planes > MAX_PLANES {
            return None;
        }

        let end = [0u32, 0, 0];
        let end_fds = [0i32, 0, 0];
        let mut offsets = offsets.iter().take(planes).chain(end.iter());
        let mut strides = strides.iter().take(planes).chain(end.iter());
        let mut fds = fds.iter().take(planes).chain(end_fds.iter());

        Some(Dmabuf(Arc::new(DmabufInternal {
            num_planes: planes,
            offsets: [*offsets.next().unwrap(), *offsets.next().unwrap(), *offsets.next().unwrap(), *offsets.next().unwrap()],
            strides: [*strides.next().unwrap(), *strides.next().unwrap(), *strides.next().unwrap(), *strides.next().unwrap()],
            fds: [*fds.next().unwrap(), *fds.next().unwrap(), *fds.next().unwrap(), *fds.next().unwrap()],

            width: src.width(),
            height: src.height(),
            format: src.format(),
        })))
    }

    pub fn handles(&self) -> &[RawFd] {
        self.0.fds.split_at(self.0.num_planes).0
    }

    pub fn offsets(&self) -> &[u32] {
        self.0.offsets.split_at(self.0.num_planes).0
    }

    pub fn strides(&self) -> &[u32] {
        self.0.strides.split_at(self.0.num_planes).0
    }

    pub fn has_modifier(&self) -> bool {
        self.0.format.modifier != Modifier::Invalid &&
        self.0.format.modifier != Modifier::Linear 
    }

    pub fn weak(&self) -> WeakDmabuf {
        WeakDmabuf(Arc::downgrade(&self.0))
    }
}

impl WeakDmabuf {
    pub fn upgrade(&self) -> Option<Dmabuf> {
        self.0.upgrade().map(|internal| Dmabuf(internal))
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