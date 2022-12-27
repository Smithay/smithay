use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, SystemTime};

use calloop::{EventSource, Interest, Poll, PostAction, Readiness, Token, TokenFactory};
use drm::control::{connector, crtc, Device as ControlDevice, Event, Mode, ResourceHandles};
use drm::{ClientCapability, Device as BasicDevice, DriverCapability};
use nix::libc::dev_t;

pub(super) mod atomic;
mod fd;
pub use self::fd::DrmDeviceFd;
pub(super) mod legacy;
use crate::utils::{DevPath, Physical, Size};

use super::surface::{atomic::AtomicDrmSurface, legacy::LegacyDrmSurface, DrmSurface, DrmSurfaceInternal};
use super::{error::Error, planes, Planes};
use atomic::AtomicDrmDevice;
use legacy::LegacyDrmDevice;

use slog::{info, o, trace};

/// An open drm device
#[derive(Debug)]
pub struct DrmDevice {
    pub(super) dev_id: dev_t,
    pub(crate) internal: Arc<DrmDeviceInternal>,
    has_universal_planes: bool,
    has_monotonic_timestamps: bool,
    cursor_size: Size<u32, Physical>,
    resources: ResourceHandles,
    pub(super) logger: ::slog::Logger,
    token: Option<Token>,
}

// TODO: Drop once not necessary for drm-rs anymore
impl AsRawFd for DrmDevice {
    fn as_raw_fd(&self) -> RawFd {
        match &*self.internal {
            DrmDeviceInternal::Atomic(dev) => dev.fd.as_raw_fd(),
            DrmDeviceInternal::Legacy(dev) => dev.fd.as_raw_fd(),
        }
    }
}
impl BasicDevice for DrmDevice {}
impl ControlDevice for DrmDevice {}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DrmDeviceInternal {
    Atomic(AtomicDrmDevice),
    Legacy(LegacyDrmDevice),
}

// TODO: Drop once not necessary for drm-rs anymore
impl AsRawFd for DrmDeviceInternal {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            DrmDeviceInternal::Atomic(dev) => dev.fd.as_raw_fd(),
            DrmDeviceInternal::Legacy(dev) => dev.fd.as_raw_fd(),
        }
    }
}
impl BasicDevice for DrmDeviceInternal {}
impl ControlDevice for DrmDeviceInternal {}

impl DevPath for DrmDeviceInternal {
    fn dev_path(&self) -> Option<PathBuf> {
        match self {
            DrmDeviceInternal::Atomic(dev) => dev.fd.dev_path(),
            DrmDeviceInternal::Legacy(dev) => dev.fd.dev_path(),
        }
    }
}

impl DrmDevice {
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

    pub fn new(
        fd: DrmDeviceFd,
        disable_connectors: bool,
        logger: impl Into<Option<slog::Logger>>,
    ) -> Result<Self, Error> {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_drm"));
        info!(log, "DrmDevice initializing");

        let dev_id = fd.dev_id().map_err(Error::UnableToGetDeviceId)?;
        let active = Arc::new(AtomicBool::new(true));

        let has_universal_planes = fd
            .set_client_capability(ClientCapability::UniversalPlanes, true)
            .is_ok();
        let has_monotonic_timestamps = fd
            .get_driver_capability(DriverCapability::MonotonicTimestamp)
            .unwrap_or(0)
            == 1;
        let cursor_width = fd
            .get_driver_capability(DriverCapability::CursorWidth)
            .unwrap_or(64);
        let cursor_height = fd
            .get_driver_capability(DriverCapability::CursorHeight)
            .unwrap_or(64);
        let cursor_size = Size::from((cursor_width as u32, cursor_height as u32));
        let resources = fd.resource_handles().map_err(|source| Error::Access {
            errmsg: "Error loading resource handles",
            dev: fd.dev_path(),
            source,
        })?;
        let internal = Arc::new(DrmDevice::create_internal(
            fd,
            active,
            disable_connectors,
            log.clone(),
        )?);

        Ok(DrmDevice {
            dev_id,
            internal,
            has_universal_planes,
            has_monotonic_timestamps,
            cursor_size,
            resources,
            logger: log,
            token: None,
        })
    }

    fn create_internal(
        fd: DrmDeviceFd,
        active: Arc<AtomicBool>,
        disable_connectors: bool,
        log: ::slog::Logger,
    ) -> Result<DrmDeviceInternal, Error> {
        let force_legacy = std::env::var("SMITHAY_USE_LEGACY")
            .map(|x| {
                x == "1" || x.to_lowercase() == "true" || x.to_lowercase() == "yes" || x.to_lowercase() == "y"
            })
            .unwrap_or(false);

        if force_legacy {
            info!(log, "SMITHAY_USE_LEGACY is set. Forcing LegacyDrmDevice.");
        };

        Ok(
            if !force_legacy && fd.set_client_capability(ClientCapability::Atomic, true).is_ok() {
                DrmDeviceInternal::Atomic(AtomicDrmDevice::new(fd, active, disable_connectors, log)?)
            } else {
                info!(log, "Falling back to LegacyDrmDevice");
                DrmDeviceInternal::Legacy(LegacyDrmDevice::new(fd, active, disable_connectors, log)?)
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

    /// Returns the size of the hardware cursor
    ///
    /// Note: In case of universal planes this is the
    /// maximum size of a buffer that can be used on
    /// the cursor plane.
    pub fn cursor_size(&self) -> Size<u32, Physical> {
        self.cursor_size
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
    ) -> Result<DrmSurface, Error> {
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(crtc));
        }

        if !self.is_active() {
            return Err(Error::DeviceInactive);
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
        })
    }

    /// Returns the device_id of the underlying drm node
    pub fn device_id(&self) -> dev_t {
        self.dev_id
    }

    /// Returns the underlying file descriptor
    pub fn device_fd(&self) -> DrmDeviceFd {
        match &*self.internal {
            DrmDeviceInternal::Atomic(internal) => internal.fd.clone(),
            DrmDeviceInternal::Legacy(internal) => internal.fd.clone(),
        }
    }

    /// Pauses the device.
    ///
    /// This will cause the `DrmDevice` to avoid making calls to the file descriptor e.g. on drop.
    /// Note that calls directly utilizing the underlying file descriptor, like the traits of the `drm-rs` crate,
    /// will ignore this state. Use [`DrmDevice::is_active`] to guard these calls.
    pub fn pause(&self) {
        self.set_active(false);
        if self.device_fd().is_privileged() {
            if let Err(err) = self.release_master_lock() {
                slog::error!(self.logger, "Failed to drop drm master state Error: {}", err);
            }
        }
    }

    /// Actives a previously paused device.
    pub fn activate(&self) {
        if self.device_fd().is_privileged() {
            if let Err(err) = self.acquire_master_lock() {
                slog::crit!(self.logger, "Failed to acquire drm master again. Error: {}", err);
            }
        }
        self.set_active(true);
    }

    /// Returns if the device is currently paused or not.
    pub fn is_active(&self) -> bool {
        match &*self.internal {
            DrmDeviceInternal::Atomic(internal) => internal.active.load(Ordering::SeqCst),
            DrmDeviceInternal::Legacy(internal) => internal.active.load(Ordering::SeqCst),
        }
    }

    fn set_active(&self, active: bool) {
        match &*self.internal {
            DrmDeviceInternal::Atomic(internal) => internal.active.store(active, Ordering::SeqCst),
            DrmDeviceInternal::Legacy(internal) => internal.active.store(active, Ordering::SeqCst),
        }
    }
}

impl DevPath for DrmDevice {
    fn dev_path(&self) -> Option<std::path::PathBuf> {
        match &*self.internal {
            DrmDeviceInternal::Atomic(internal) => internal.fd.dev_path(),
            DrmDeviceInternal::Legacy(internal) => internal.fd.dev_path(),
        }
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
    Monotonic(Duration),
    /// Realtime time stamp
    Realtime(SystemTime),
}

impl EventSource for DrmDevice {
    type Event = DrmEvent;
    type Metadata = Option<EventMetadata>;
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(&mut self, _: Readiness, token: Token, mut callback: F) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        if Some(token) != self.token {
            return Ok(PostAction::Continue);
        }
        match self.receive_events() {
            Ok(events) => {
                for event in events {
                    if let Event::PageFlip(event) = event {
                        trace!(self.logger, "Got a page-flip event for crtc ({:?})", event.crtc);
                        let metadata = EventMetadata {
                            time: if self.has_monotonic_timestamps {
                                Time::Monotonic(event.duration)
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

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.token = Some(factory.token());
        poll.register(
            self.as_raw_fd(),
            Interest::READ,
            calloop::Mode::Level,
            self.token.unwrap(),
        )
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.token = Some(factory.token());
        poll.reregister(
            self.as_raw_fd(),
            Interest::READ,
            calloop::Mode::Level,
            self.token.unwrap(),
        )
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.token = None;
        poll.unregister(self.as_raw_fd())
    }
}
