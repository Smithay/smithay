//! Module for [dmabuf](https://01.org/linuxgraphics/gfx-docs/drm/driver-api/dma-buf.html) buffers.
//!
//! `Dmabuf`s act alike to smart pointers and can be freely cloned and passed around.
//! Once the last `Dmabuf` reference is dropped, its file descriptor is closed and
//! underlying resources are freed.
//!
//! If you want to hold on to a potentially alive dmabuf without blocking the free up
//! of the underlying resources, you may `downgrade` a `Dmabuf` reference to a `WeakDmabuf`.
//!
//! This can be especially useful in resources where other parts of the stack should decide upon
//! the lifetime of the buffer. E.g. when you are only caching associated resources for a dmabuf.

use super::{Buffer, Format, Fourcc, Modifier};
use crate::utils::{Buffer as BufferCoords, Size};
use std::hash::{Hash, Hasher};
use std::os::unix::io::{IntoRawFd, RawFd};
use std::sync::{Arc, Weak};

/// Maximum amount of planes this implementation supports
pub const MAX_PLANES: usize = 4;

#[derive(Debug)]
pub(crate) struct DmabufInternal {
    /// The submitted planes
    pub planes: Vec<Plane>,
    /// The size of this buffer
    pub size: Size<i32, BufferCoords>,
    /// The format in use
    pub format: Fourcc,
    /// The flags applied to it
    ///
    /// This is a bitflag, to be compared with the `Flags` enum re-exported by this module.
    pub flags: DmabufFlags,
}

#[derive(Debug)]
pub(crate) struct Plane {
    pub fd: Option<RawFd>,
    /// The plane index
    pub plane_idx: u32,
    /// Offset from the start of the Fd
    pub offset: u32,
    /// Stride for this plane
    pub stride: u32,
    /// Modifier for this plane
    pub modifier: Modifier,
}

impl IntoRawFd for Plane {
    fn into_raw_fd(mut self) -> RawFd {
        self.fd.take().unwrap()
    }
}

impl Drop for Plane {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            let _ = nix::unistd::close(fd);
        }
    }
}

bitflags::bitflags! {
    /// Possible flags for a DMA buffer
    pub struct DmabufFlags: u32 {
        /// The buffer content is Y-inverted
        const Y_INVERT = 1;
        /// The buffer content is interlaced
        const INTERLACED = 2;
        /// The buffer content if interlaced is bottom-field first
        const BOTTOM_FIRST = 4;
    }
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

impl PartialEq for WeakDmabuf {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for WeakDmabuf {}

impl Hash for Dmabuf {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state)
    }
}
impl Hash for WeakDmabuf {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_ptr().hash(state)
    }
}

impl Buffer for Dmabuf {
    fn size(&self) -> Size<i32, BufferCoords> {
        self.0.size
    }

    fn format(&self) -> Format {
        Format {
            code: self.0.format,
            modifier: self.0.planes[0].modifier,
        }
    }
}

/// Builder for Dmabufs
#[derive(Debug)]
pub struct DmabufBuilder {
    internal: DmabufInternal,
}

impl DmabufBuilder {
    /// Add a plane to the constructed Dmabuf
    ///
    /// *Note*: Each Dmabuf needs at least one plane.
    /// MAX_PLANES notes the maximum amount of planes any format may use with this implementation.
    pub fn add_plane(&mut self, fd: RawFd, idx: u32, offset: u32, stride: u32, modifier: Modifier) -> bool {
        if self.internal.planes.len() == MAX_PLANES {
            return false;
        }
        self.internal.planes.push(Plane {
            fd: Some(fd),
            plane_idx: idx,
            offset,
            stride,
            modifier,
        });

        true
    }

    /// Build a `Dmabuf` out of the provided parameters and planes
    ///
    /// Returns `None` if the builder has no planes attached.
    pub fn build(mut self) -> Option<Dmabuf> {
        if self.internal.planes.is_empty() {
            return None;
        }

        self.internal.planes.sort_by_key(|plane| plane.plane_idx);
        Some(Dmabuf(Arc::new(self.internal)))
    }
}

impl Dmabuf {
    /// Create a new Dmabuf by initializing with values from an existing buffer
    ///
    // Note: the `src` Buffer is only used a reference for size and format.
    // The contents are determined by the provided file descriptors, which
    // do not need to refer to the same buffer `src` does.
    pub fn builder_from_buffer(src: &impl Buffer, flags: DmabufFlags) -> DmabufBuilder {
        DmabufBuilder {
            internal: DmabufInternal {
                planes: Vec::with_capacity(MAX_PLANES),
                size: src.size(),
                format: src.format().code,
                flags,
            },
        }
    }

    /// Create a new Dmabuf builder
    pub fn builder(
        size: impl Into<Size<i32, BufferCoords>>,
        format: Fourcc,
        flags: DmabufFlags,
    ) -> DmabufBuilder {
        DmabufBuilder {
            internal: DmabufInternal {
                planes: Vec::with_capacity(MAX_PLANES),
                size: size.into(),
                format,
                flags,
            },
        }
    }

    /// The amount of planes this Dmabuf has
    pub fn num_planes(&self) -> usize {
        self.0.planes.len()
    }

    /// Returns raw handles of the planes of this buffer
    pub fn handles(&self) -> impl Iterator<Item = RawFd> + '_ {
        self.0.planes.iter().map(|p| *p.fd.as_ref().unwrap())
    }

    /// Returns offsets for the planes of this buffer
    pub fn offsets(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.planes.iter().map(|p| p.offset)
    }

    /// Returns strides for the planes of this buffer
    pub fn strides(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.planes.iter().map(|p| p.stride)
    }

    /// Returns if this buffer format has any vendor-specific modifiers set or is implicit/linear
    pub fn has_modifier(&self) -> bool {
        self.0.planes[0].modifier != Modifier::Invalid && self.0.planes[0].modifier != Modifier::Linear
    }

    /// Returns if the buffer is stored inverted on the y-axis
    pub fn y_inverted(&self) -> bool {
        self.0.flags.contains(DmabufFlags::Y_INVERT)
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

    /// Returns true if there are not any strong references remaining
    pub fn is_gone(&self) -> bool {
        self.0.strong_count() == 0
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
    type Error = std::convert::Infallible;

    fn export(&self) -> Result<Dmabuf, std::convert::Infallible> {
        Ok(self.clone())
    }
}
