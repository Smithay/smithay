use std::collections::HashMap;
use std::io;
use std::os::unix::io::{AsFd, BorrowedFd};
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicBool, Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use calloop::{EventSource, Interest, Poll, PostAction, Readiness, Token, TokenFactory};
use drm::control::{connector, crtc, plane, Device as ControlDevice, Event, Mode, ResourceHandles};
use drm::{ClientCapability, Device as BasicDevice, DriverCapability};
use libc::dev_t;

pub(super) mod atomic;
mod fd;
pub use self::fd::DrmDeviceFd;
pub(super) mod legacy;
use crate::utils::{Buffer, DevPath, Size};

use super::error::AccessError;
use super::surface::{atomic::AtomicDrmSurface, legacy::LegacyDrmSurface, DrmSurface, DrmSurfaceInternal};
use super::{error::Error, planes, Planes};
use atomic::AtomicDrmDevice;
use legacy::LegacyDrmDevice;

use tracing::{debug_span, error, info, instrument, trace};

#[derive(Debug)]
struct PlaneClaimInner {
    plane: drm::control::plane::Handle,
    crtc: drm::control::crtc::Handle,
    storage: PlaneClaimStorage,
}

impl Drop for PlaneClaimInner {
    fn drop(&mut self) {
        self.storage.remove(self.plane);
    }
}

#[derive(Debug, Clone)]
struct PlaneClaimWeak(Weak<PlaneClaimInner>);

impl PlaneClaimWeak {
    fn upgrade(&self) -> Option<PlaneClaim> {
        self.0.upgrade().map(|claim| PlaneClaim { claim })
    }
}

/// A claim of a plane
#[derive(Debug, Clone)]
pub struct PlaneClaim {
    claim: Arc<PlaneClaimInner>,
}

impl PlaneClaim {
    /// The plane the claim was taken for
    pub fn plane(&self) -> drm::control::plane::Handle {
        self.claim.plane
    }

    /// The crtc the claim was taken for
    pub fn crtc(&self) -> drm::control::crtc::Handle {
        self.claim.crtc
    }
}

impl PlaneClaim {
    fn downgrade(&self) -> PlaneClaimWeak {
        PlaneClaimWeak(Arc::downgrade(&self.claim))
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlaneClaimStorage {
    claimed_planes: Arc<Mutex<HashMap<drm::control::plane::Handle, PlaneClaimWeak>>>,
}

impl PlaneClaimStorage {
    pub fn claim(
        &self,
        plane: drm::control::plane::Handle,
        crtc: drm::control::crtc::Handle,
    ) -> Option<PlaneClaim> {
        let mut guard = self.claimed_planes.lock().unwrap();
        if let Some(claim) = guard.get(&plane).and_then(|claim| claim.upgrade()) {
            if claim.crtc() == crtc {
                Some(claim)
            } else {
                None
            }
        } else {
            let claim = PlaneClaim {
                claim: Arc::new(PlaneClaimInner {
                    plane,
                    crtc,
                    storage: self.clone(),
                }),
            };
            guard.insert(plane, claim.downgrade());
            Some(claim)
        }
    }

    fn remove(&self, plane: drm::control::plane::Handle) {
        let mut guard = self.claimed_planes.lock().unwrap();
        guard.remove(&plane);
    }
}

/// An open drm device
#[derive(Debug)]
pub struct DrmDevice {
    pub(super) dev_id: dev_t,
    pub(crate) internal: Arc<DrmDeviceInternal>,
    has_universal_planes: bool,
    cursor_size: Size<u32, Buffer>,
    resources: ResourceHandles,
    plane_claim_storage: PlaneClaimStorage,
    surfaces: Vec<Weak<DrmSurfaceInternal>>,
}

impl AsFd for DrmDevice {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match &*self.internal {
            DrmDeviceInternal::Atomic(dev) => dev.fd.as_fd(),
            DrmDeviceInternal::Legacy(dev) => dev.fd.as_fd(),
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

impl DrmDeviceInternal {
    pub(crate) fn device_fd(&self) -> &DrmDeviceFd {
        match self {
            DrmDeviceInternal::Atomic(dev) => &dev.fd,
            DrmDeviceInternal::Legacy(dev) => &dev.fd,
        }
    }

    fn span(&self) -> &tracing::Span {
        match self {
            DrmDeviceInternal::Atomic(internal) => &internal.span,
            DrmDeviceInternal::Legacy(internal) => &internal.span,
        }
    }
}

impl AsFd for DrmDeviceInternal {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            DrmDeviceInternal::Atomic(dev) => dev.fd.as_fd(),
            DrmDeviceInternal::Legacy(dev) => dev.fd.as_fd(),
        }
    }
}

impl BasicDevice for DrmDeviceInternal {}
impl ControlDevice for DrmDeviceInternal {}

impl DrmDevice {
    /// Create a new [`DrmDevice`] from an open drm node
    ///
    /// # Arguments
    ///
    /// - `fd` - Open drm node
    /// - `disable_connectors` - Setting this to true will initialize all connectors \
    ///   as disabled on device creation. smithay enables connectors, when attached \
    ///   to a surface, and disables them, when detached. Setting this to `false` \
    ///   requires usage of `drm-rs` to disable unused connectors to prevent them \
    ///   showing garbage, but will also prevent flickering of already turned on \
    ///   connectors (assuming you won't change the resolution).
    ///
    /// # Return
    ///
    /// Returns an error if the file is no valid drm node or the device is not accessible.
    pub fn new(fd: DrmDeviceFd, disable_connectors: bool) -> Result<(Self, DrmDeviceNotifier), Error> {
        // setup parent span for internal device types
        let span = debug_span!(
            "drm_device",
            device = ?fd.dev_path()
        );
        let _guard = span.enter();

        info!("DrmDevice initializing");

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
        let resources = fd.resource_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading resource handles",
                dev: fd.dev_path(),
                source,
            })
        })?;

        let internal = Arc::new(DrmDevice::create_internal(fd, active, disable_connectors)?);

        Ok((
            DrmDevice {
                dev_id,
                internal: internal.clone(),
                has_universal_planes,
                cursor_size,
                resources,
                plane_claim_storage: Default::default(),
                surfaces: Default::default(),
            },
            DrmDeviceNotifier {
                internal,
                has_monotonic_timestamps,
                token: None,
            },
        ))
    }

    fn create_internal(
        fd: DrmDeviceFd,
        active: Arc<AtomicBool>,
        disable_connectors: bool,
    ) -> Result<DrmDeviceInternal, Error> {
        let force_legacy = std::env::var("SMITHAY_USE_LEGACY")
            .map(|x| {
                x == "1" || x.to_lowercase() == "true" || x.to_lowercase() == "yes" || x.to_lowercase() == "y"
            })
            .unwrap_or(false);

        if force_legacy {
            info!("SMITHAY_USE_LEGACY is set. Forcing LegacyDrmDevice.");
        };

        Ok(
            if !force_legacy && fd.set_client_capability(ClientCapability::Atomic, true).is_ok() {
                DrmDeviceInternal::Atomic(AtomicDrmDevice::new(fd, active, disable_connectors)?)
            } else {
                info!("Falling back to LegacyDrmDevice");
                DrmDeviceInternal::Legacy(LegacyDrmDevice::new(fd, active, disable_connectors)?)
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

    /// Claim a plane so that it won't be used by a different crtc
    ///  
    /// Returns `None` if the plane could not be claimed
    pub fn claim_plane(&self, plane: plane::Handle, crtc: crtc::Handle) -> Option<PlaneClaim> {
        self.plane_claim_storage.claim(plane, crtc)
    }

    /// Returns the size of the hardware cursor
    ///
    /// Note: In case of universal planes this is the
    /// maximum size of a buffer that can be used on
    /// the cursor plane.
    pub fn cursor_size(&self) -> Size<u32, Buffer> {
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
    #[instrument(skip(self), parent = self.internal.span(), err)]
    pub fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<DrmSurface, Error> {
        self.surfaces.retain(|surface| surface.strong_count() != 0);

        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(crtc));
        }

        if !self.is_active() {
            return Err(Error::DeviceInactive);
        }

        let planes = self.planes(&crtc)?;

        let selected_primary_plane = planes.primary.iter().find_map(|plane| {
            let claim = match self.plane_claim_storage.claim(plane.handle, crtc) {
                Some(claim) => claim,
                None => {
                    tracing::debug!(?crtc, ?plane, "skipping already claimed primary plane");
                    return None;
                }
            };
            let info = match self.get_plane(plane.handle) {
                Ok(info) => info,
                Err(err) => {
                    tracing::warn!(?crtc, ?err, ?plane, "failed to get primary plane info");
                    return None;
                }
            };

            let filter = info.possible_crtcs();
            if !self.resources.filter_crtcs(filter).contains(&crtc) {
                tracing::warn!(?crtc, ?plane, "primary plane not compatible with crtc");
                return None;
            }

            Some((plane.clone(), claim))
        });

        let Some((plane, claim)) = selected_primary_plane else {
            return Err(Error::NoPlane);
        };

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
                plane.handle,
                mapping,
                mode,
                connectors,
            )?)
        } else {
            DrmSurfaceInternal::Legacy(LegacyDrmSurface::new(
                self.internal.clone(),
                active,
                crtc,
                mode,
                connectors,
            )?)
        };
        let internal = Arc::new(internal);
        self.surfaces.push(Arc::downgrade(&internal));

        Ok(DrmSurface {
            dev_id: self.dev_id,
            crtc,
            planes,
            internal,
            plane_claim_storage: self.plane_claim_storage.clone(),
            primary_plane: (plane, claim),
        })
    }

    /// Returns the device_id of the underlying drm node
    pub fn device_id(&self) -> dev_t {
        self.dev_id
    }

    /// Returns the underlying file descriptor
    pub fn device_fd(&self) -> &DrmDeviceFd {
        self.internal.device_fd()
    }

    /// Pauses the device.
    ///
    /// This will cause the `DrmDevice` to avoid making calls to the file descriptor e.g. on drop.
    /// Note that calls directly utilizing the underlying file descriptor, like the traits of the `drm-rs` crate,
    /// will ignore this state. Use [`DrmDevice::is_active`] to guard these calls.
    pub fn pause(&mut self) {
        self.set_active(false);
        self.surfaces.retain(|surface| surface.strong_count() != 0);
        if self.device_fd().is_privileged() {
            if let Err(err) = self.release_master_lock() {
                error!("Failed to drop drm master state Error: {}", err);
            }
        }
    }

    /// Activates a previously paused device.
    ///
    /// Specifying `true` for `disable_connectors` will call [`DrmDevice::reset_state`] if
    /// the device was not active before. Otherwise you need to make sure there are no
    /// conflicting requirements when enabling or creating surfaces or you are prepared
    /// to handle errors caused by those.
    pub fn activate(&mut self, disable_connectors: bool) -> Result<(), Error> {
        if self.device_fd().is_privileged() {
            if let Err(err) = self.acquire_master_lock() {
                error!("Failed to acquire drm master again. Error: {}", err);
            }
        }
        if !self.set_active(true) && disable_connectors {
            self.reset_state()
        } else {
            self.surfaces.retain(|surface| surface.strong_count() != 0);
            Ok(())
        }
    }

    /// Returns if the device is currently paused or not.
    pub fn is_active(&self) -> bool {
        match &*self.internal {
            DrmDeviceInternal::Atomic(internal) => internal.active.load(Ordering::SeqCst),
            DrmDeviceInternal::Legacy(internal) => internal.active.load(Ordering::SeqCst),
        }
    }

    /// Reset the state of this device
    ///
    /// This will disable all connectors and reset all planes.
    /// Additional this will also reset the state on all known surfaces.
    pub fn reset_state(&mut self) -> Result<(), Error> {
        if !self.is_active() {
            return Err(Error::DeviceInactive);
        }

        match &*self.internal {
            DrmDeviceInternal::Atomic(internal) => internal.reset_state(),
            DrmDeviceInternal::Legacy(internal) => internal.reset_state(),
        }?;

        let mut i = 0;
        while i != self.surfaces.len() {
            if let Some(surface) = self.surfaces[i].upgrade() {
                match &*surface {
                    DrmSurfaceInternal::Atomic(surf) => surf.reset_state::<Self>(None),
                    DrmSurfaceInternal::Legacy(surf) => surf.reset_state::<Self>(None),
                }?;
                i += 1;
            } else {
                self.surfaces.remove(i);
            }
        }
        Ok(())
    }

    fn set_active(&self, active: bool) -> bool {
        match &*self.internal {
            DrmDeviceInternal::Atomic(internal) => internal.active.swap(active, Ordering::SeqCst),
            DrmDeviceInternal::Legacy(internal) => internal.active.swap(active, Ordering::SeqCst),
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
#[derive(Debug, Clone, Copy)]
pub struct EventMetadata {
    /// The time the frame flip happend
    pub time: Time,
    /// The sequence number of the frame
    pub sequence: u32,
}

/// Either a realtime or monotonic timestamp
#[derive(Debug, Clone, Copy)]
pub enum Time {
    /// Monotonic time stamp
    Monotonic(Duration),
    /// Realtime time stamp
    Realtime(SystemTime),
}

/// Even source of [`DrmDevice`]
#[derive(Debug)]
pub struct DrmDeviceNotifier {
    internal: Arc<DrmDeviceInternal>,
    has_monotonic_timestamps: bool,
    token: Option<Token>,
}

impl EventSource for DrmDeviceNotifier {
    type Event = DrmEvent;
    type Metadata = Option<EventMetadata>;
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(&mut self, _: Readiness, token: Token, mut callback: F) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let _guard = self.internal.span().enter();

        if Some(token) != self.token {
            return Ok(PostAction::Continue);
        }

        match self.internal.receive_events() {
            Ok(events) => {
                for event in events {
                    if let Event::PageFlip(event) = event {
                        trace!("Got a page-flip event for crtc ({:?})", event.crtc);
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
                            "Got a non-page-flip event of device '{:?}'.",
                            self.internal.dev_path()
                        );
                    }
                }
            }
            Err(source) => {
                callback(
                    DrmEvent::Error(Error::Access(AccessError {
                        errmsg: "Error processing drm events",
                        dev: self.internal.dev_path(),
                        source,
                    })),
                    &mut None,
                );
            }
        }
        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.token = Some(factory.token());
        // Safety: the FD cannot be closed without removing the DrmDeviceNotifier from the event loop
        unsafe {
            poll.register(
                self.internal.as_fd(),
                Interest::READ,
                calloop::Mode::Level,
                self.token.unwrap(),
            )
        }
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.token = Some(factory.token());
        poll.reregister(
            self.internal.as_fd(),
            Interest::READ,
            calloop::Mode::Level,
            self.token.unwrap(),
        )
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.token = None;
        poll.unregister(self.internal.as_fd())
    }
}
