//! Module for [dmabuf](https://docs.kernel.org/driver-api/dma-buf.html) buffers.
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
use rustix::ioctl::Setter;

use super::{Allocator, Buffer, Format, Fourcc, Modifier};
#[cfg(feature = "backend_drm")]
use crate::backend::drm::DrmNode;
use crate::utils::{Buffer as BufferCoords, Size};
#[cfg(feature = "wayland_frontend")]
use crate::wayland::compositor::{Blocker, BlockerState};
use std::hash::{Hash, Hasher};
use std::os::unix::io::{AsFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "backend_drm")]
use std::sync::Mutex;
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
    /// Format modifier
    pub modifier: Modifier,
    /// The flags applied to it
    ///
    /// This is a bitflag, to be compared with the `Flags` enum re-exported by this module.
    pub flags: DmabufFlags,
    /// Presumably compatible device for buffer import
    ///
    /// This is inferred from client apis, however there is no kernel api or guarantee this is correct
    #[cfg(feature = "backend_drm")]
    node: Mutex<Option<DrmNode>>,
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
}

impl From<Plane> for OwnedFd {
    #[inline]
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

// A reference to a particular dmabuf plane fd, so it can be used as a calloop source.
#[derive(Debug)]
struct PlaneRef {
    dmabuf: Dmabuf,
    idx: usize,
}

impl AsFd for PlaneRef {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.dmabuf.0.planes[self.idx].fd.as_fd()
    }
}

impl PartialEq for Dmabuf {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Dmabuf {}

impl PartialEq for WeakDmabuf {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for WeakDmabuf {}

impl Hash for Dmabuf {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state)
    }
}
impl Hash for WeakDmabuf {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_ptr().hash(state)
    }
}

impl Buffer for Dmabuf {
    #[inline]
    fn size(&self) -> Size<i32, BufferCoords> {
        self.0.size
    }

    #[inline]
    fn format(&self) -> Format {
        Format {
            code: self.0.format,
            modifier: self.0.modifier,
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
    pub fn add_plane(&mut self, fd: OwnedFd, idx: u32, offset: u32, stride: u32) -> bool {
        if self.internal.planes.len() == MAX_PLANES {
            return false;
        }
        self.internal.planes.push(Plane {
            fd,
            plane_idx: idx,
            offset,
            stride,
        });

        true
    }

    /// Sets a known good node to import the dmabuf with.
    ///
    /// While this is only a strong hint and no guarantee, implementations
    /// should avoid setting this at all, if they can't be reasonably certain.
    #[cfg(feature = "backend_drm")]
    pub fn set_node(&mut self, node: DrmNode) {
        self.internal.node = Mutex::new(Some(node));
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
        Self::builder(src.size(), src.format().code, src.format().modifier, flags)
    }

    /// Create a new Dmabuf builder
    pub fn builder(
        size: impl Into<Size<i32, BufferCoords>>,
        format: Fourcc,
        modifier: Modifier,
        flags: DmabufFlags,
    ) -> DmabufBuilder {
        DmabufBuilder {
            internal: DmabufInternal {
                planes: Vec::with_capacity(MAX_PLANES),
                size: size.into(),
                format,
                modifier,
                flags,
                #[cfg(feature = "backend_drm")]
                node: Mutex::new(None),
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
        self.0.modifier != Modifier::Invalid && self.0.modifier != Modifier::Linear
    }

    /// Returns if the buffer is stored inverted on the y-axis
    pub fn y_inverted(&self) -> bool {
        self.0.flags.contains(DmabufFlags::Y_INVERT)
    }

    /// Create a weak reference to this dmabuf
    pub fn weak(&self) -> WeakDmabuf {
        WeakDmabuf(Arc::downgrade(&self.0))
    }

    /// Presumably compatible device for buffer import
    ///
    /// This is inferred from client apis, as there is no kernel api or other guarantee this is correct,
    /// so it should only be treated as a hint.
    #[cfg(feature = "backend_drm")]
    pub fn node(&self) -> Option<DrmNode> {
        *self.0.node.lock().unwrap()
    }

    /// Sets or unsets any node set for this dmabuf (see [`Dmabuf::node`]).
    ///
    /// May alter behavior of other parts of smithay using this as a hint.
    #[cfg(feature = "backend_drm")]
    pub fn set_node(&self, node: impl Into<Option<DrmNode>>) {
        *self.0.node.lock().unwrap() = node.into();
    }

    /// Create an [`calloop::EventSource`] and [`Blocker`] for this [`Dmabuf`].
    ///
    /// Usually used to block applying surface state on the readiness of an attached dmabuf.
    #[cfg(feature = "wayland_frontend")]
    #[profiling::function]
    pub fn generate_blocker(
        &self,
        interest: Interest,
    ) -> Result<(DmabufBlocker, DmabufSource), AlreadyReady> {
        let source = DmabufSource::new(self.clone(), interest)?;
        let blocker = DmabufBlocker(source.signal.clone());
        Ok((blocker, source))
    }

    /// Map the plane at specified index with the specified mode
    ///
    /// Returns `Err` if the plane with the specified index does not exist or
    /// mmap failed
    pub fn map_plane(
        &self,
        idx: usize,
        mode: DmabufMappingMode,
    ) -> Result<DmabufMapping, DmabufMappingFailed> {
        let plane = self
            .0
            .planes
            .get(idx)
            .ok_or(DmabufMappingFailed::PlaneIndexOutOfBound)?;

        let size = rustix::fs::seek(&plane.fd, rustix::fs::SeekFrom::End(0)).map_err(std::io::Error::from)?;
        rustix::fs::seek(&plane.fd, rustix::fs::SeekFrom::Start(0)).map_err(std::io::Error::from)?;

        let len = (size - plane.offset as u64) as usize;
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                len,
                mode.into(),
                rustix::mm::MapFlags::SHARED,
                &plane.fd,
                plane.offset as u64,
            )
        }
        .map_err(std::io::Error::from)?;
        Ok(DmabufMapping { len, ptr })
    }

    /// Synchronize access for the plane at the specified index
    ///
    /// Returns `Err` if the plane with the specified index does not exist or
    /// the dmabuf_sync ioctl failed
    pub fn sync_plane(&self, idx: usize, flags: DmabufSyncFlags) -> Result<(), DmabufSyncFailed> {
        let plane = self
            .0
            .planes
            .get(idx)
            .ok_or(DmabufSyncFailed::PlaneIndexOutOfBound)?;
        unsafe { rustix::ioctl::ioctl(&plane.fd, Setter::<DMA_BUF_SYNC, _>::new(dma_buf_sync { flags })) }
            .map_err(std::io::Error::from)?;
        Ok(())
    }
}

bitflags::bitflags! {
    /// Modes of mapping a dmabuf plane
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct DmabufMappingMode: u32 {
        /// Map the dmabuf readable
        const READ = 0b00000001;
        /// Map the dmabuf writable
        const WRITE = 0b00000010;
    }
}

impl From<DmabufMappingMode> for rustix::mm::ProtFlags {
    #[inline]
    fn from(mode: DmabufMappingMode) -> Self {
        let mut flags = rustix::mm::ProtFlags::empty();

        if mode.contains(DmabufMappingMode::READ) {
            flags |= rustix::mm::ProtFlags::READ;
        }

        if mode.contains(DmabufMappingMode::WRITE) {
            flags |= rustix::mm::ProtFlags::WRITE;
        }

        flags
    }
}

/// Dmabuf mapping errors
#[derive(Debug, thiserror::Error)]
#[error("Mapping the dmabuf failed")]
pub enum DmabufMappingFailed {
    /// The supplied index for the plane is out of bounds
    #[error("The supplied index for the plane is out of bounds")]
    PlaneIndexOutOfBound,
    /// Io error during map operation
    Io(#[from] std::io::Error),
}

/// Dmabuf sync errors
#[derive(Debug, thiserror::Error)]
#[error("Sync of the dmabuf failed")]
pub enum DmabufSyncFailed {
    /// The supplied index for the plane is out of bounds
    #[error("The supplied index for the plane is out of bounds")]
    PlaneIndexOutOfBound,
    /// Io error during sync operation
    Io(#[from] std::io::Error),
}

bitflags::bitflags! {
    /// Flags for the [`Dmabuf::sync_plane`](Dmabuf::sync_plane) operation
    #[derive(Copy, Clone)]
    pub struct DmabufSyncFlags: std::ffi::c_ulonglong {
        /// Read from the dmabuf
        const READ = 1 << 0;
        /// Write to the dmabuf
        #[allow(clippy::identity_op)]
        const WRITE = 2 << 0;
        /// Start of read/write
        const START = 0 << 2;
        /// End of read/write
        const END = 1 << 2;
    }
}

#[repr(C)]
#[allow(non_camel_case_types)]
struct dma_buf_sync {
    flags: DmabufSyncFlags,
}

const DMA_BUF_SYNC: rustix::ioctl::Opcode = rustix::ioctl::opcode::write::<dma_buf_sync>(b'b', 0);

/// A mapping into a [`Dmabuf`]
#[derive(Debug)]
pub struct DmabufMapping {
    ptr: *mut std::ffi::c_void,
    len: usize,
}

impl DmabufMapping {
    /// Access the raw pointer of the mapping
    pub fn ptr(&self) -> *mut std::ffi::c_void {
        self.ptr
    }

    /// Access the length of the mapping
    pub fn length(&self) -> usize {
        self.len
    }
}

// SAFETY: The caller is responsible for accessing the data without assuming
// another process isn't mutating it, regardless of how many threads this is
// referenced in.
unsafe impl Send for DmabufMapping {}
unsafe impl Sync for DmabufMapping {}

impl Drop for DmabufMapping {
    fn drop(&mut self) {
        let _ = unsafe { rustix::mm::munmap(self.ptr, self.len) };
    }
}

impl WeakDmabuf {
    /// Try to upgrade to a strong reference of this buffer.
    ///
    /// Fails if no strong references exist anymore and the handle was already closed.
    #[inline]
    pub fn upgrade(&self) -> Option<Dmabuf> {
        self.0.upgrade().map(Dmabuf)
    }

    /// Returns true if there are not any strong references remaining
    #[inline]
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

    #[inline]
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
    #[inline]
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

    #[profiling::function]
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

/// [`Blocker`] implementation for an accompaning [`DmabufSource`]
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
    Active(Generic<PlaneRef, std::io::Error>),
    Done(Generic<PlaneRef, std::io::Error>),
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
    #[profiling::function]
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
            if matches!(
                rustix::event::poll(
                    &mut [rustix::event::PollFd::new(
                        &handle,
                        if interest.writable {
                            rustix::event::PollFlags::OUT
                        } else {
                            rustix::event::PollFlags::IN
                        },
                    )],
                    Some(&rustix::time::Timespec {
                        tv_sec: 0,
                        tv_nsec: 0
                    })
                ),
                Ok(1)
            ) {
                continue;
            }
            let fd = PlaneRef {
                dmabuf: dmabuf.clone(),
                idx,
            };
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

    #[profiling::function]
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
