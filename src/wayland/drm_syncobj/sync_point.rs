use calloop::generic::Generic;
use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};
use drm::control::Device;
use std::{
    io,
    os::unix::io::{AsFd, BorrowedFd, OwnedFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use crate::backend::drm::DrmDeviceFd;
use crate::backend::renderer::sync::{Fence, Interrupted};
use crate::wayland::compositor::{Blocker, BlockerState};

#[derive(Debug, PartialEq)]
struct DrmTimelineInner {
    device: DrmDeviceFd,
    syncobj: drm::control::syncobj::Handle,
}

impl Drop for DrmTimelineInner {
    fn drop(&mut self) {
        let _ = self.device.destroy_syncobj(self.syncobj);
    }
}

/// DRM timeline syncobj
#[derive(Clone, Debug, PartialEq)]
pub struct DrmTimeline(Arc<DrmTimelineInner>);

impl DrmTimeline {
    /// Import DRM timeline from file descriptor
    pub fn new(device: &DrmDeviceFd, fd: BorrowedFd<'_>) -> io::Result<Self> {
        Ok(Self(Arc::new(DrmTimelineInner {
            device: device.clone(),
            syncobj: device.fd_to_syncobj(fd, false)?,
        })))
    }

    /// Query the last signalled timeline point
    pub fn query_signalled_point(&self) -> io::Result<u64> {
        let mut points = [0];
        self.0
            .device
            .syncobj_timeline_query(&[self.0.syncobj], &mut points, false)?;
        Ok(points[0])
    }
}

/// Point on a DRM timeline syncobj
#[derive(Clone, Debug, PartialEq)]
pub struct DrmSyncPoint {
    pub(super) timeline: DrmTimeline,
    pub(super) point: u64,
}

impl DrmSyncPoint {
    /// Create an eventfd that will be signaled by the syncpoint
    pub fn eventfd(&self) -> io::Result<OwnedFd> {
        let fd = rustix::event::eventfd(
            0,
            rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK,
        )?;
        self.timeline
            .0
            .device
            .syncobj_eventfd(self.timeline.0.syncobj, self.point, fd.as_fd(), false)?;
        Ok(fd)
    }

    /// Signal the sync point.
    pub fn signal(&self) -> io::Result<()> {
        self.timeline
            .0
            .device
            .syncobj_timeline_signal(&[self.timeline.0.syncobj], &[self.point])
    }

    /// Wait for sync point.
    #[allow(clippy::same_name_method)]
    pub fn wait(&self, timeout_nsec: i64) -> io::Result<()> {
        self.timeline.0.device.syncobj_timeline_wait(
            &[self.timeline.0.syncobj],
            &[self.point],
            timeout_nsec,
            false,
            false,
            false,
        )?;
        Ok(())
    }

    /// Export DRM sync file for sync point.
    pub fn export_sync_file(&self) -> io::Result<OwnedFd> {
        let syncobj = self.timeline.0.device.create_syncobj(false)?;
        // Wrap in `DrmTimelineInner` to destroy on drop
        let syncobj = DrmTimelineInner {
            device: self.timeline.0.device.clone(),
            syncobj,
        };
        syncobj
            .device
            .syncobj_timeline_transfer(self.timeline.0.syncobj, syncobj.syncobj, self.point, 0)?;
        syncobj.device.syncobj_to_fd(syncobj.syncobj, true)
    }

    /// Create an [`calloop::EventSource`] and [`Blocker`] for this sync point.
    ///
    /// This will fail if `drmSyncobjEventfd` isn't supported by the device. See
    /// [`supports_syncobj_eventfd`](super::supports_syncobj_eventfd).
    pub fn generate_blocker(&self) -> io::Result<(DrmSyncPointBlocker, DrmSyncPointSource)> {
        let fd = self.eventfd()?;
        let signal = Arc::new(AtomicBool::new(false));
        let blocker = DrmSyncPointBlocker {
            signal: signal.clone(),
        };
        let source = DrmSyncPointSource {
            source: Generic::new(fd, Interest::READ, Mode::Level),
            signal,
        };
        Ok((blocker, source))
    }
}

impl Fence for DrmSyncPoint {
    fn is_signaled(&self) -> bool {
        self.timeline
            .query_signalled_point()
            .ok()
            .map_or(false, |point| point >= self.point)
    }

    fn wait(&self) -> Result<(), Interrupted> {
        self.wait(i64::MAX).map_err(|_| Interrupted)
    }

    fn is_exportable(&self) -> bool {
        true
    }

    fn export(&self) -> Option<OwnedFd> {
        self.export_sync_file().ok()
    }
}

/// Event source generating an event when a [`DrmSyncPoint`] is signalled..
#[derive(Debug)]
pub struct DrmSyncPointSource {
    source: Generic<OwnedFd>,
    signal: Arc<AtomicBool>,
}

impl EventSource for DrmSyncPointSource {
    type Event = ();
    type Metadata = ();
    type Ret = Result<(), std::io::Error>;
    type Error = io::Error;

    fn process_events<C>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: C,
    ) -> Result<PostAction, Self::Error>
    where
        C: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.signal.store(true, Ordering::SeqCst);
        self.source
            .process_events(readiness, token, |_, _| Ok(PostAction::Remove))?;
        callback((), &mut ())?;
        Ok(PostAction::Remove)
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.source.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.source.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.source.unregister(poll)
    }
}

/// [`Blocker`] implementation for an accompaning [`DrmSyncPointSource`]
#[derive(Debug)]
pub struct DrmSyncPointBlocker {
    signal: Arc<AtomicBool>,
}

impl Blocker for DrmSyncPointBlocker {
    fn state(&self) -> BlockerState {
        if self.signal.load(Ordering::SeqCst) {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}
