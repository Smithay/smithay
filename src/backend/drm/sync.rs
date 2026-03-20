//! DRM syncobj and timeline helpers

use calloop::generic::Generic;
use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};
use drm::control::Device;
use std::sync::{Mutex, Weak};
use std::{
    io,
    os::unix::io::{AsFd, BorrowedFd, OwnedFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use super::{DrmDeviceFd, WeakDrmDeviceFd};
#[cfg(feature = "wayland_frontend")]
use crate::wayland::compositor::{Blocker, BlockerState};

/// Test if DRM device supports `syncobj_eventfd`.
// Similar to test used in Mutter
pub fn supports_syncobj_eventfd(device: &DrmDeviceFd) -> bool {
    // Pass device as placeholder for eventfd as well, since `drm_ffi` requires
    // a valid fd.
    match drm_ffi::syncobj::eventfd(device.as_fd(), 0, 0, device.as_fd(), false) {
        Ok(_) => unreachable!(),
        Err(err) => err.kind() == std::io::ErrorKind::NotFound,
    }
}

#[derive(Debug)]
pub(crate) struct DrmTimelineInner {
    timeline_fd: OwnedFd,
    dev_ctx: Mutex<DrmTimelineDeviceSpecific>,
}

#[derive(Debug)]
struct DrmTimelineDeviceSpecific {
    device: WeakDrmDeviceFd,
    syncobj: drm::control::syncobj::Handle,
    event_fds: Vec<(u64, Weak<OwnedFd>)>,
}

impl DrmTimelineDeviceSpecific {
    fn import(fd: BorrowedFd<'_>, device: &DrmDeviceFd) -> io::Result<Self> {
        let syncobj = device.fd_to_syncobj(fd, false)?;
        Ok(DrmTimelineDeviceSpecific {
            device: device.downgrade(),
            syncobj,
            event_fds: Vec::new(),
        })
    }

    fn invalidate(&mut self) {
        if let Some(device) = self.device.upgrade() {
            let _ = device.destroy_syncobj(self.syncobj);
        }
        self.device = WeakDrmDeviceFd::new();
        // trigger event fds
        for eventfd in self.event_fds.drain(..).filter_map(|(_, x)| Weak::upgrade(&x)) {
            let _ = rustix::io::write(&eventfd, &[1]);
        }
    }
}

/// DRM timeline syncobj
#[derive(Clone, Debug)]
pub struct DrmTimeline(Arc<DrmTimelineInner>);

/// Weak reference to a DRM timeline syncobj
#[derive(Clone, Debug)]
pub struct WeakDrmTimeline(Weak<DrmTimelineInner>);

impl PartialEq for DrmTimeline {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl DrmTimeline {
    /// Import DRM timeline from file descriptor
    pub fn new(device: &DrmDeviceFd, fd: OwnedFd) -> io::Result<Self> {
        let dev_ctx = Mutex::new(DrmTimelineDeviceSpecific::import(fd.as_fd(), device)?);
        Ok(Self(Arc::new(DrmTimelineInner {
            timeline_fd: fd,
            dev_ctx,
        })))
    }

    /// Query the last signalled timeline point
    pub fn query_signalled_point(&self) -> io::Result<u64> {
        let ctx = self.0.dev_ctx.lock().unwrap();
        let device = ctx
            .device
            .upgrade()
            .ok_or::<io::Error>(io::ErrorKind::InvalidInput.into())?;

        let mut points = [0];
        device.syncobj_timeline_query(&[ctx.syncobj], &mut points, false)?;
        Ok(points[0])
    }

    /// Creates a weak reference to the timeline,
    /// which doesn't keep the resource alive.
    pub fn downgrade(&self) -> WeakDrmTimeline {
        WeakDrmTimeline(Arc::downgrade(&self.0))
    }

    pub(crate) fn update_device(&self, device: &DrmDeviceFd) -> io::Result<()> {
        let mut ctx = self.0.dev_ctx.lock().unwrap();
        let mut new = DrmTimelineDeviceSpecific::import(self.0.timeline_fd.as_fd(), device)?;
        for (point, eventfd) in ctx
            .event_fds
            .iter()
            .flat_map(|(p, fd)| fd.upgrade().map(|fd| (p, fd)))
        {
            device.syncobj_eventfd(new.syncobj, *point, eventfd.as_fd(), false)?;
            new.event_fds.push((*point, Arc::downgrade(&eventfd)));
        }
        *ctx = new;
        Ok(())
    }

    pub(crate) fn invalidate(&self) {
        self.0.dev_ctx.lock().unwrap().invalidate()
    }
}

impl WeakDrmTimeline {
    /// Tries to recieve a strong reference to the underlying timeline
    pub fn upgrade(&self) -> Option<DrmTimeline> {
        self.0.upgrade().map(DrmTimeline)
    }
}

/// Point on a DRM timeline syncobj
#[derive(Clone, Debug)]
pub struct DrmSyncPoint {
    pub(crate) timeline: DrmTimeline,
    pub(crate) point: u64,
}

impl DrmSyncPoint {
    /// Create an eventfd that will be signaled by the syncpoint
    pub fn eventfd(&self) -> io::Result<Arc<OwnedFd>> {
        let fd = rustix::event::eventfd(
            0,
            rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK,
        )?;
        let mut ctx = self.timeline.0.dev_ctx.lock().unwrap();
        ctx.device
            .upgrade()
            .ok_or::<io::Error>(io::ErrorKind::InvalidInput.into())?
            .syncobj_eventfd(ctx.syncobj, self.point, fd.as_fd(), false)?;

        let fd = Arc::new(fd);
        ctx.event_fds.retain(|(_, fd)| fd.upgrade().is_some());
        ctx.event_fds.push((self.point, Arc::downgrade(&fd)));
        Ok(fd)
    }

    /// Signal the sync point.
    pub fn signal(&self) -> io::Result<()> {
        let ctx = self.timeline.0.dev_ctx.lock().unwrap();
        ctx.device
            .upgrade()
            .ok_or::<io::Error>(io::ErrorKind::InvalidInput.into())?
            .syncobj_timeline_signal(&[ctx.syncobj], &[self.point])
    }

    /// Wait for sync point.
    pub fn wait(&self, timeout_nsec: i64) -> io::Result<()> {
        let ctx = self.timeline.0.dev_ctx.lock().unwrap();
        ctx.device
            .upgrade()
            .ok_or::<io::Error>(io::ErrorKind::InvalidInput.into())?
            .syncobj_timeline_wait(&[ctx.syncobj], &[self.point], timeout_nsec, false, false, false)?;
        Ok(())
    }

    /// Export DRM sync file for sync point.
    pub fn export_sync_file(&self) -> io::Result<OwnedFd> {
        let ctx = self.timeline.0.dev_ctx.lock().unwrap();
        let Some(device) = ctx.device.upgrade() else {
            return Err(io::ErrorKind::InvalidInput.into());
        };

        let syncobj = device.create_syncobj(false)?;
        if let Err(err) = device.syncobj_timeline_transfer(ctx.syncobj, syncobj, self.point, 0) {
            let _ = device.destroy_syncobj(syncobj);
            return Err(err);
        };

        let res = device.syncobj_to_fd(syncobj, true);
        let _ = device.destroy_syncobj(syncobj);
        res
    }

    /// Create an [`calloop::EventSource`] and [`Blocker`] for this sync point.
    ///
    /// This will fail if `drmSyncobjEventfd` isn't supported by the device. See
    /// [`supports_syncobj_eventfd`].
    #[cfg(feature = "wayland_frontend")]
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

/// Event source generating an event when a [`DrmSyncPoint`] is signalled..
#[derive(Debug)]
pub struct DrmSyncPointSource {
    source: Generic<Arc<OwnedFd>>,
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
        self.source.register(poll, token_factory)?;
        Ok(())
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.source.reregister(poll, token_factory)?;
        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.source.unregister(poll)?;
        Ok(())
    }
}

#[cfg(feature = "wayland_frontend")]
/// [`Blocker`] implementation for an accompaning [`DrmSyncPointSource`]
#[derive(Debug)]
pub struct DrmSyncPointBlocker {
    signal: Arc<AtomicBool>,
}

#[cfg(feature = "wayland_frontend")]
impl Blocker for DrmSyncPointBlocker {
    fn state(&self) -> BlockerState {
        if self.signal.load(Ordering::SeqCst) {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}
