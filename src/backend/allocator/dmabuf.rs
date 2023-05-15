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

use calloop::generic::Generic;
use calloop::{EventSource, Interest, Mode, PostAction};
use nix::poll;

use super::{Allocator, Buffer, Format, Fourcc, Modifier};
use crate::utils::{Buffer as BufferCoords, Size};
#[cfg(feature = "wayland_frontend")]
use crate::wayland::compositor::{Blocker, BlockerState};
use std::hash::{Hash, Hasher};
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::{error, fmt};

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
    pub fd: OwnedFd,
    /// The plane index
    pub plane_idx: u32,
    /// Offset from the start of the Fd
    pub offset: u32,
    /// Stride for this plane
    pub stride: u32,
    /// Modifier for this plane
    pub modifier: Modifier,
}

impl From<Plane> for OwnedFd {
    fn from(plane: Plane) -> OwnedFd {
        plane.fd
    }
}

bitflags::bitflags! {
    /// Possible flags for a DMA buffer
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    pub fn add_plane(&mut self, fd: OwnedFd, idx: u32, offset: u32, stride: u32, modifier: Modifier) -> bool {
        if self.internal.planes.len() == MAX_PLANES {
            return false;
        }
        self.internal.planes.push(Plane {
            fd,
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
    pub fn handles(&self) -> impl Iterator<Item = BorrowedFd<'_>> + '_ {
        self.0.planes.iter().map(|p| p.fd.as_fd())
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

    /// Create an [`calloop::EventSource`] and [`crate::wayland::compositor::Blocker`] for this [`Dmabuf`].
    ///
    /// Usually used to block applying surface state on the readiness of an attached dmabuf.
    #[cfg(feature = "wayland_frontend")]
    pub fn generate_blocker(
        &self,
        interest: Interest,
    ) -> Result<(DmabufBlocker, DmabufSource), AlreadyReady> {
        let source = DmabufSource::new(self.clone(), interest)?;
        let blocker = DmabufBlocker(source.signal.clone());
        Ok((blocker, source))
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
    type Error: std::error::Error;

    /// Export this buffer as a new Dmabuf
    fn export(&self) -> Result<Dmabuf, Self::Error>;
}

impl AsDmabuf for Dmabuf {
    type Error = std::convert::Infallible;

    fn export(&self) -> Result<Dmabuf, std::convert::Infallible> {
        Ok(self.clone())
    }
}

/// Type erased error
#[derive(Debug)]
pub struct AnyError(Box<dyn error::Error + Send + Sync>);

impl fmt::Display for AnyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl error::Error for AnyError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&*self.0)
    }
}

/// Wrapper for Allocators, whos buffer types implement [`AsDmabuf`].
///
/// Implements `Allocator<Buffer=Dmabuf, Error=AnyError>`
#[derive(Debug)]
pub struct DmabufAllocator<A>(pub A)
where
    A: Allocator,
    <A as Allocator>::Buffer: AsDmabuf + 'static,
    <A as Allocator>::Error: 'static;

impl<A> Allocator for DmabufAllocator<A>
where
    A: Allocator,
    <A as Allocator>::Buffer: AsDmabuf + 'static,
    <A as Allocator>::Error: Send + Sync + 'static,
    <<A as Allocator>::Buffer as AsDmabuf>::Error: Send + Sync + 'static,
{
    type Buffer = Dmabuf;
    type Error = AnyError;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<Self::Buffer, Self::Error> {
        self.0
            .create_buffer(width, height, fourcc, modifiers)
            .map_err(|err| AnyError(err.into()))
            .and_then(|b| AsDmabuf::export(&b).map_err(|err| AnyError(err.into())))
    }
}

/// [`crate::wayland::compositor::Blocker`] implementation for an accompaning [`DmabufSource`]
#[cfg(feature = "wayland_frontend")]
#[derive(Debug)]
pub struct DmabufBlocker(Arc<AtomicBool>);

#[cfg(feature = "wayland_frontend")]
impl Blocker for DmabufBlocker {
    fn state(&self) -> BlockerState {
        if self.0.load(Ordering::SeqCst) {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}

#[derive(Debug)]
enum Subsource {
    Active(Generic<RawFd, std::io::Error>),
    Done(Generic<RawFd, std::io::Error>),
    Empty,
}

impl Subsource {
    fn done(&mut self) {
        let mut this = Subsource::Empty;
        std::mem::swap(self, &mut this);
        match this {
            Subsource::Done(source) | Subsource::Active(source) => {
                *self = Subsource::Done(source);
            }
            _ => {}
        }
    }
}

/// [`Dmabuf`]-based event source. Can be used to monitor implicit fences of a dmabuf.
#[derive(Debug)]
pub struct DmabufSource {
    dmabuf: Dmabuf,
    signal: Arc<AtomicBool>,
    sources: [Subsource; 4],
}

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[error("Dmabuf is already ready for the given interest")]
/// Dmabuf is already ready for the given interest
pub struct AlreadyReady;

impl DmabufSource {
    /// Creates a new [`DmabufSource`] from a [`Dmabuf`] and interest.
    ///
    /// This event source will monitor the implicit fences of the given dmabuf.
    /// Monitoring for READ-access will monitor the state of the most recent write or exclusive fence.
    /// Monitoring for WRITE-access, will monitor state of all attached fences, shared and exclusive ones.
    ///
    /// The event source is a one shot event source and will remove itself from the event loop after being triggered once.
    /// To monitor for new fences added at a later time a new DmabufSource needs to be created.
    ///
    /// Returns `AlreadyReady` if all corresponding fences are already signalled or if `interest` is empty.
    pub fn new(dmabuf: Dmabuf, interest: Interest) -> Result<Self, AlreadyReady> {
        if !interest.readable && !interest.writable {
            return Err(AlreadyReady);
        }

        let mut sources = [
            Subsource::Empty,
            Subsource::Empty,
            Subsource::Empty,
            Subsource::Empty,
        ];
        for (idx, handle) in dmabuf.handles().enumerate() {
            // SAFETY: This is stored together with the Dmabuf holding the owned file descriptors
            let fd = handle.as_raw_fd();
            if matches!(
                poll::poll(
                    &mut [poll::PollFd::new(
                        fd,
                        if interest.writable {
                            poll::PollFlags::POLLOUT
                        } else {
                            poll::PollFlags::POLLIN
                        },
                    )],
                    0
                ),
                Ok(1)
            ) {
                continue;
            }
            sources[idx] = Subsource::Active(Generic::new(fd, interest, Mode::OneShot));
        }
        if sources
            .iter()
            .all(|x| matches!(x, Subsource::Done(_) | Subsource::Empty))
        {
            Err(AlreadyReady)
        } else {
            Ok(DmabufSource {
                dmabuf,
                sources,
                signal: Arc::new(AtomicBool::new(false)),
            })
        }
    }
}

impl EventSource for DmabufSource {
    type Event = ();
    type Metadata = Dmabuf;
    type Ret = Result<(), std::io::Error>;

    type Error = std::io::Error;

    fn process_events<F>(
        &mut self,
        readiness: calloop::Readiness,
        token: calloop::Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        for i in 0..4 {
            if let Ok(PostAction::Remove) = if let Subsource::Active(ref mut source) = &mut self.sources[i] {
                // luckily Generic skips events for other tokens
                source.process_events(readiness, token, |_, _| Ok(PostAction::Remove))
            } else {
                Ok(PostAction::Continue)
            } {
                self.sources[i].done();
            }
        }

        if self
            .sources
            .iter()
            .all(|x| matches!(x, Subsource::Done(_) | Subsource::Empty))
        {
            self.signal.store(true, Ordering::SeqCst);
            callback((), &mut self.dmabuf)?;
            Ok(PostAction::Remove)
        } else {
            Ok(PostAction::Reregister)
        }
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        for source in self.sources.iter_mut().filter_map(|source| match source {
            Subsource::Active(ref mut source) => Some(source),
            _ => None,
        }) {
            source.register(poll, token_factory)?;
        }
        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        for source in self.sources.iter_mut() {
            match source {
                Subsource::Active(ref mut source) => source.reregister(poll, token_factory)?,
                Subsource::Done(ref mut source) => {
                    let _ = source.unregister(poll);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        for source in self.sources.iter_mut() {
            match source {
                Subsource::Active(ref mut source) => source.unregister(poll)?,
                Subsource::Done(ref mut source) => {
                    let _ = source.unregister(poll);
                }
                _ => {}
            }
        }
        Ok(())
    }
}
