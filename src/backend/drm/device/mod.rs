#[cfg(feature = "backend_session")]
use std::cell::RefCell;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Instant, SystemTime};

use calloop::{EventSource, Interest, Poll, PostAction, Readiness, Token, TokenFactory};
use drm::control::{connector, crtc, Device as ControlDevice, Event, Mode, ResourceHandles};
use drm::{ClientCapability, Device as BasicDevice, DriverCapability};
use nix::libc::dev_t;
use nix::sys::stat::fstat;

pub(super) mod atomic;
pub(super) mod legacy;
use super::surface::{atomic::AtomicDrmSurface, legacy::LegacyDrmSurface, DrmSurface, DrmSurfaceInternal};
use super::{error::Error, planes, Planes};
use atomic::AtomicDrmDevice;
use legacy::LegacyDrmDevice;

use slog::{error, info, o, trace, warn};

/// An open drm device
#[derive(Debug)]
pub struct DrmDevice<A: AsRawFd + 'static> {
    pub(super) dev_id: dev_t,
    pub(crate) internal: Arc<DrmDeviceInternal<A>>,
    #[cfg(feature = "backend_session")]
    pub(super) links: RefCell<Vec<crate::utils::signaling::SignalToken>>,
    has_universal_planes: bool,
    has_monotonic_timestamps: bool,
    resources: ResourceHandles,
    pub(super) logger: ::slog::Logger,
    token: Token,
}

impl<A: AsRawFd + 'static> AsRawFd for DrmDevice<A> {
    fn as_raw_fd(&self) -> RawFd {
        match &*self.internal {
            DrmDeviceInternal::Atomic(dev) => dev.fd.as_raw_fd(),
            DrmDeviceInternal::Legacy(dev) => dev.fd.as_raw_fd(),
        }
    }
}
impl<A: AsRawFd + 'static> BasicDevice for DrmDevice<A> {}
impl<A: AsRawFd + 'static> ControlDevice for DrmDevice<A> {}

#[derive(Debug)]
pub struct FdWrapper<A: AsRawFd + 'static> {
    fd: A,
    pub(super) privileged: bool,
    logger: ::slog::Logger,
}

impl<A: AsRawFd + 'static> AsRawFd for FdWrapper<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl<A: AsRawFd + 'static> BasicDevice for FdWrapper<A> {}
impl<A: AsRawFd + 'static> ControlDevice for FdWrapper<A> {}

impl<A: AsRawFd + 'static> Drop for FdWrapper<A> {
    fn drop(&mut self) {
        info!(self.logger, "Dropping device: {:?}", self.dev_path());
        if self.privileged {
            if let Err(err) = self.release_master_lock() {
                error!(self.logger, "Failed to drop drm master state. Error: {}", err);
            }
        }
    }
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DrmDeviceInternal<A: AsRawFd + 'static> {
    Atomic(AtomicDrmDevice<A>),
    Legacy(LegacyDrmDevice<A>),
}
impl<A: AsRawFd + 'static> AsRawFd for DrmDeviceInternal<A> {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            DrmDeviceInternal::Atomic(dev) => dev.fd.as_raw_fd(),
            DrmDeviceInternal::Legacy(dev) => dev.fd.as_raw_fd(),
        }
    }
}
impl<A: AsRawFd + 'static> BasicDevice for DrmDeviceInternal<A> {}
impl<A: AsRawFd + 'static> ControlDevice for DrmDeviceInternal<A> {}

impl<A: AsRawFd + 'static> DrmDevice<A> {
    /// Create a new [`DrmDevice`] from an open drm node
    ///
    /// # Arguments
    ///
    /// - `fd` - Open drm node
    /// - `disable_connectors` - Setting this to true will initialize all connectors \
    ///     as disabled on device creation. smithay enables connectors, when attached \
    ///     to a surface, and disables them, when detached. Setting this to `false` \
    ///     requires usage of `drm-rs` to disable unused connectors to prevent them \
    ///     showing garbage, but will also prevent flickering of already turned on \
    ///     connectors (assuming you won't change the resolution).
    /// - `logger` - Optional [`slog::Logger`] to be used by this device.
    ///
    /// # Return
    ///
    /// Returns an error if the file is no valid drm node or the device is not accessible.

    pub fn new<L>(fd: A, disable_connectors: bool, logger: L) -> Result<Self, Error>
    where
        A: AsRawFd + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_drm"));
        info!(log, "DrmDevice initializing");

        let dev_id = fstat(fd.as_raw_fd()).map_err(Error::UnableToGetDeviceId)?.st_rdev;
        let active = Arc::new(AtomicBool::new(true));
        let dev = Arc::new({
            let mut dev = FdWrapper {
                fd,
                privileged: false,
                logger: log.clone(),
            };

            // We want to modeset, so we better be the master, if we run via a tty session.
            // This is only needed on older kernels. Newer kernels grant this permission,
            // if no other process is already the *master*. So we skip over this error.
            if dev.acquire_master_lock().is_err() {
                warn!(log, "Unable to become drm master, assuming unprivileged mode");
            } else {
                dev.privileged = true;
            }

            dev
        });

        let has_universal_planes = dev
            .set_client_capability(ClientCapability::UniversalPlanes, true)
            .is_ok();
        let has_monotonic_timestamps = dev
            .get_driver_capability(DriverCapability::MonotonicTimestamp)
            .unwrap_or(0)
            == 1;
        let resources = dev.resource_handles().map_err(|source| Error::Access {
            errmsg: "Error loading resource handles",
            dev: dev.dev_path(),
            source,
        })?;
        let internal = Arc::new(DrmDevice::create_internal(
            dev,
            active,
            disable_connectors,
            log.clone(),
        )?);

        Ok(DrmDevice {
            dev_id,
            internal,
            #[cfg(feature = "backend_session")]
            links: RefCell::new(Vec::new()),
            has_universal_planes,
            has_monotonic_timestamps,
            resources,
            logger: log,
            token: Token::invalid(),
        })
    }

    fn create_internal(
        dev: Arc<FdWrapper<A>>,
        active: Arc<AtomicBool>,
        disable_connectors: bool,
        log: ::slog::Logger,
    ) -> Result<DrmDeviceInternal<A>, Error> {
        let force_legacy = std::env::var("SMITHAY_USE_LEGACY")
            .map(|x| {
                x == "1" || x.to_lowercase() == "true" || x.to_lowercase() == "yes" || x.to_lowercase() == "y"
            })
            .unwrap_or(false);

        if force_legacy {
            info!(log, "SMITHAY_USE_LEGACY is set. Forcing LegacyDrmDevice.");
        };

        Ok(
            if !force_legacy && dev.set_client_capability(ClientCapability::Atomic, true).is_ok() {
                DrmDeviceInternal::Atomic(AtomicDrmDevice::new(dev, active, disable_connectors, log)?)
            } else {
                info!(log, "Falling back to LegacyDrmDevice");
                DrmDeviceInternal::Legacy(LegacyDrmDevice::new(dev, active, disable_connectors, log)?)
            },
        )
    }

    /// Returns if the underlying implementation uses atomic-modesetting or not.
    pub fn is_atomic(&self) -> bool {
        match *self.internal {
            DrmDeviceInternal::Atomic(_) => true,
            DrmDeviceInternal::Legacy(_) => false,
        }
    }

    /// Returns a list of crtcs for this device
    pub fn crtcs(&self) -> &[crtc::Handle] {
        self.resources.crtcs()
    }

    /// Returns a set of available planes for a given crtc
    pub fn planes(&self, crtc: &crtc::Handle) -> Result<Planes, Error> {
        planes(self, crtc, self.has_universal_planes)
    }

    /// Creates a new rendering surface.
    ///
    /// # Arguments
    ///
    /// Initialization of surfaces happens through the types provided by
    /// [`drm-rs`](drm).
    ///
    /// - [`crtcs`](drm::control::crtc) represent scanout engines of the device pointing to one framebuffer. \
    ///     Their responsibility is to read the data of the framebuffer and export it into an "Encoder". \
    ///     The number of crtc's represent the number of independent output devices the hardware may handle.
    /// - [`mode`](drm::control::Mode) describes the resolution and rate of images produced by the crtc and \
    ///     has to be compatible with the provided `connectors`.
    /// - [`connectors`](drm::control::connector) - List of connectors driven by the crtc. At least one(!) connector needs to be \
    ///     attached to a crtc in smithay.
    pub fn create_surface(
        &self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<DrmSurface<A>, Error> {
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(crtc));
        }

        let plane = planes(self, &crtc, self.has_universal_planes)?.primary;
        let info = self.get_plane(plane).map_err(|source| Error::Access {
            errmsg: "Failed to get plane info",
            dev: self.dev_path(),
            source,
        })?;
        let filter = info.possible_crtcs();
        if !self.resources.filter_crtcs(filter).contains(&crtc) {
            return Err(Error::PlaneNotCompatible(crtc, plane));
        }

        let active = match &*self.internal {
            DrmDeviceInternal::Atomic(dev) => dev.active.clone(),
            DrmDeviceInternal::Legacy(dev) => dev.active.clone(),
        };

        let internal = if self.is_atomic() {
            let mapping = match &*self.internal {
                DrmDeviceInternal::Atomic(dev) => dev.prop_mapping.clone(),
                _ => unreachable!(),
            };

            DrmSurfaceInternal::Atomic(AtomicDrmSurface::new(
                self.internal.clone(),
                active,
                crtc,
                plane,
                mapping,
                mode,
                connectors,
                self.logger.clone(),
            )?)
        } else {
            DrmSurfaceInternal::Legacy(LegacyDrmSurface::new(
                self.internal.clone(),
                active,
                crtc,
                mode,
                connectors,
                self.logger.clone(),
            )?)
        };

        Ok(DrmSurface {
            dev_id: self.dev_id,
            crtc,
            primary: plane,
            internal: Arc::new(internal),
            has_universal_planes: self.has_universal_planes,
            #[cfg(feature = "backend_session")]
            links: RefCell::new(Vec::new()),
        })
    }

    /// Returns the device_id of the underlying drm node
    pub fn device_id(&self) -> dev_t {
        self.dev_id
    }
}

/// Trait representing open devices that *may* return a `Path`
pub trait DevPath {
    /// Returns the path of the open device if possible
    fn dev_path(&self) -> Option<PathBuf>;
}

impl<A: AsRawFd> DevPath for A {
    fn dev_path(&self) -> Option<PathBuf> {
        use std::fs;

        fs::read_link(format!("/proc/self/fd/{:?}", self.as_raw_fd())).ok()
    }
}

/// Events that can be generated by a DrmDevice
#[derive(Debug)]
pub enum DrmEvent {
    /// A vblank blank event on the provided crtc has happened
    VBlank(crtc::Handle),
    /// An error happened while processing events
    Error(Error),
}

/// Timing metadata for page-flip events
#[derive(Debug)]
pub struct EventMetadata {
    /// The time the frame flip happend
    pub time: Time,
    /// The sequence number of the frame
    pub sequence: u32,
}

/// Either a realtime or monotonic timestamp
#[derive(Debug)]
pub enum Time {
    /// Monotonic time stamp
    Monotonic(Instant),
    /// Realtime time stamp
    Realtime(SystemTime),
}

impl<A> EventSource for DrmDevice<A>
where
    A: AsRawFd + 'static,
{
    type Event = DrmEvent;
    type Metadata = Option<EventMetadata>;
    type Ret = ();

    fn process_events<F>(
        &mut self,
        _: Readiness,
        token: Token,
        mut callback: F,
    ) -> std::io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        if token != self.token {
            return Ok(PostAction::Continue);
        }
        match self.receive_events() {
            Ok(events) => {
                for event in events {
                    if let Event::PageFlip(event) = event {
                        trace!(self.logger, "Got a page-flip event for crtc ({:?})", event.crtc);
                        let metadata = EventMetadata {
                            time: if self.has_monotonic_timestamps {
                                // There is no way to create an Instant, although the underlying type on unix systems
                                // is just libc::timespec, which is literally what drm-rs is getting from the kernel and just converting
                                // into a Duration. So we cheat and initialize a Zero-Instant (because although Instant::ZERO
                                // exists, its private, so you cannot create abitrary Instants). What we really need is a unix-Ext
                                // trait for both SystemTime and Instant to convert from a libc::timespec.
                                //
                                // But this works for now, although it is quite the hack.
                                Time::Monotonic(unsafe { std::mem::zeroed::<Instant>() } + event.duration)
                            } else {
                                Time::Realtime(SystemTime::UNIX_EPOCH + event.duration)
                            },
                            sequence: event.frame,
                        };
                        callback(DrmEvent::VBlank(event.crtc), &mut Some(metadata));
                    } else {
                        trace!(
                            self.logger,
                            "Got a non-page-flip event of device '{:?}'.",
                            self.dev_path()
                        );
                    }
                }
            }
            Err(source) => {
                callback(
                    DrmEvent::Error(Error::Access {
                        errmsg: "Error processing drm events",
                        dev: self.dev_path(),
                        source,
                    }),
                    &mut None,
                );
            }
        }
        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.token = factory.token();
        poll.register(self.as_raw_fd(), Interest::READ, calloop::Mode::Level, self.token)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.token = factory.token();
        poll.reregister(self.as_raw_fd(), Interest::READ, calloop::Mode::Level, self.token)
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        self.token = Token::invalid();
        poll.unregister(self.as_raw_fd())
    }
}
