use std::rc::Rc;
use std::cell::RefCell;
use std::sync::{Arc, atomic::AtomicBool};
use std::path::PathBuf;
use std::os::unix::io::{AsRawFd, RawFd};

use calloop::{generic::Generic, InsertError, LoopHandle, Source};
use drm::{Device as BasicDevice, ClientCapability};
use drm::control::{ResourceHandles, PlaneResourceHandles, Device as ControlDevice, Event, Mode, PlaneType, crtc, plane, connector};
use nix::libc::dev_t;
use nix::sys::stat::fstat;

pub(super) mod atomic;
pub(super) mod legacy;
use atomic::AtomicDrmDevice;
use legacy::LegacyDrmDevice;
use super::surface::{DrmSurface, DrmSurfaceInternal, atomic::AtomicDrmSurface, legacy::LegacyDrmSurface};
use super::error::Error;

pub struct DrmDevice<A: AsRawFd + 'static> {
    pub(super) dev_id: dev_t,
    pub(crate) internal: Arc<DrmDeviceInternal<A>>,
    handler: Rc<RefCell<Option<Box<dyn DeviceHandler>>>>,
    #[cfg(feature = "backend_session")]
    pub(super) links: RefCell<Vec<crate::signaling::SignalToken>>,
    has_universal_planes: bool,
    resources: ResourceHandles,
    planes: PlaneResourceHandles,
    pub(super) logger: ::slog::Logger,
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
    pub fn new<L>(fd: A, disable_connectors: bool, logger: L) -> Result<Self, Error>
    where
        A: AsRawFd + Clone + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_drm"));
        info!(log, "DrmDevice initializing");
        
        let dev_id = fstat(fd.as_raw_fd())
            .map_err(Error::UnableToGetDeviceId)?
            .st_rdev;
        let active = Arc::new(AtomicBool::new(true));
        let dev = Arc::new({
            let mut dev = FdWrapper {
                fd: fd.clone(),
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

        let has_universal_planes = dev.set_client_capability(ClientCapability::UniversalPlanes, true).is_ok();
        let resources = dev.resource_handles().map_err(|source| Error::Access {
            errmsg: "Error loading resource handles",
            dev: dev.dev_path(),
            source,
        })?;
        let planes = dev.plane_handles().map_err(|source| Error::Access {
            errmsg: "Error loading plane handles",
            dev: dev.dev_path(),
            source,
        })?;
        let internal = Arc::new(DrmDevice::create_internal(dev, active, disable_connectors, log.clone())?);

        Ok(DrmDevice {
            dev_id,
            internal,
            handler: Rc::new(RefCell::new(None)),
            #[cfg(feature = "backend_session")]
            links: RefCell::new(Vec::new()),
            has_universal_planes,
            resources,
            planes,
            logger: log,
        })
    }

    fn create_internal(dev: Arc<FdWrapper<A>>, active: Arc<AtomicBool>, disable_connectors: bool, log: ::slog::Logger) -> Result<DrmDeviceInternal<A>, Error> {
        let force_legacy = std::env::var("SMITHAY_USE_LEGACY")
            .map(|x| {
                x == "1" || x.to_lowercase() == "true" || x.to_lowercase() == "yes" || x.to_lowercase() == "y"
            })
            .unwrap_or(false);

        if force_legacy {
            info!(log, "SMITHAY_USE_LEGACY is set. Forcing LegacyDrmDevice.");
        };
        
        Ok(if dev.set_client_capability(ClientCapability::Atomic, true).is_ok() && !force_legacy {
            DrmDeviceInternal::Atomic(AtomicDrmDevice::new(dev, active, disable_connectors, log)?)
        } else {
            info!(log, "Falling back to LegacyDrmDevice");
            DrmDeviceInternal::Legacy(LegacyDrmDevice::new(dev, active, disable_connectors, log)?)
        })
    }

    pub fn process_events(&mut self) {
        match self.receive_events() {
            Ok(events) => {
                for event in events {
                    if let Event::PageFlip(event) = event {
                        trace!(self.logger, "Got a page-flip event for crtc ({:?})", event.crtc);
                        if let Some(handler) = self.handler.borrow_mut().as_mut() {
                            handler.vblank(event.crtc);
                        }
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
                if let Some(handler) = self.handler.borrow_mut().as_mut() {
                    handler.error(Error::Access {
                        errmsg: "Error processing drm events",
                        dev: self.dev_path(),
                        source,
                    });
                }
            }
        }
    }

    pub fn is_atomic(&self) -> bool {
        match *self.internal {
            DrmDeviceInternal::Atomic(_) => true,
            DrmDeviceInternal::Legacy(_) => false,
        }
    }
    
    pub fn set_handler(&mut self, handler: impl DeviceHandler + 'static) {
        let handler = Some(Box::new(handler) as Box<dyn DeviceHandler + 'static>);
        *self.handler.borrow_mut() = handler;
    }

    pub fn clear_handler(&mut self) {
        self.handler.borrow_mut().take();
    }

    pub fn crtcs(&self) -> &[crtc::Handle] {
        self.resources.crtcs()
    }

    pub fn planes(&self, crtc: &crtc::Handle) -> Result<Planes, Error> {
        let mut primary = None;
        let mut cursor = None;
        let mut overlay = Vec::new();

        for plane in self.planes.planes() {
            let info = self.get_plane(*plane).map_err(|source| Error::Access {
                errmsg: "Failed to get plane information",
                dev: self.dev_path(),
                source,  
            })?;
            let filter = info.possible_crtcs();
            if self.resources.filter_crtcs(filter).contains(crtc) {
                match self.plane_type(*plane)? {
                    PlaneType::Primary => { primary = Some(*plane); },
                    PlaneType::Cursor => { cursor = Some(*plane); },
                    PlaneType::Overlay => { overlay.push(*plane); },
                };
            }
        }

        Ok(Planes {
            primary: primary.expect("Crtc has no primary plane"),
            cursor,
            overlay: if self.has_universal_planes { Some(overlay) } else { None },
        })
    }

    fn plane_type(&self, plane: plane::Handle) -> Result<PlaneType, Error> {
        let props = self.get_properties(plane).map_err(|source| Error::Access {
            errmsg: "Failed to get properties of plane",
            dev: self.dev_path(),
            source,  
        })?;
        let (ids, vals) = props.as_props_and_values();
        for (&id, &val) in ids.iter().zip(vals.iter()) {
            let info = self.get_property(id).map_err(|source| Error::Access {
                errmsg: "Failed to get property info",
                dev: self.dev_path(),
                source,  
            })?;
            if info.name().to_str().map(|x| x == "type").unwrap_or(false) {
                return Ok(match val {
                    x if x == (PlaneType::Primary as u64) => PlaneType::Primary,
                    x if x == (PlaneType::Cursor as u64) => PlaneType::Cursor,
                    _ => PlaneType::Overlay,
                });
            }
        }
        unreachable!()
    }

    pub fn create_surface(&self, crtc: crtc::Handle, plane: plane::Handle, mode: Mode, connectors: &[connector::Handle]) -> Result<DrmSurface<A>, Error> {
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(crtc));
        }
        
        let info = self.get_plane(plane).map_err(|source| Error::Access {
            errmsg: "Failed to get plane info",
            dev: self.dev_path(),
            source
        })?;
        let filter = info.possible_crtcs();
        if !self.resources.filter_crtcs(filter).contains(&crtc) {
            return Err(Error::PlaneNotCompatible(crtc, plane));
        }
        
        let active = match &*self.internal {
            DrmDeviceInternal::Atomic(dev) => dev.active.clone(),
            DrmDeviceInternal::Legacy(dev) => dev.active.clone(),
        };

        let internal = Arc::new(if self.is_atomic() {
            let mapping = match &*self.internal {
                DrmDeviceInternal::Atomic(dev) => dev.prop_mapping.clone(),
                _ => unreachable!(),
            };

            DrmSurfaceInternal::Atomic(AtomicDrmSurface::new(self.internal.clone(), active, crtc, plane, mapping, mode, connectors, self.logger.clone())?)
        } else {
            if self.plane_type(plane)? != PlaneType::Primary {
                return Err(Error::NonPrimaryPlane(plane));
            }

            DrmSurfaceInternal::Legacy(LegacyDrmSurface::new(self.internal.clone(), active, crtc, mode, connectors, self.logger.clone())?)
        });

        Ok(DrmSurface {
            crtc,
            plane,
            internal,
        })
    }
}

pub struct Planes {
    pub primary: plane::Handle,
    pub cursor: Option<plane::Handle>,
    pub overlay: Option<Vec<plane::Handle>>,
}

impl<A: AsRawFd + 'static> DrmDeviceInternal<A> {
    pub(super) fn reset_state(&self) -> Result<(), Error> {
        match self {
            DrmDeviceInternal::Atomic(dev) => dev.reset_state(),
            DrmDeviceInternal::Legacy(dev) => dev.reset_state(),
        }
    }
}

/// Trait to receive events of a bound [`DrmDevice`]
///
/// See [`device_bind`]
pub trait DeviceHandler {
    /// A vblank blank event on the provided crtc has happend
    fn vblank(&mut self, crtc: crtc::Handle);
    /// An error happend while processing events
    fn error(&mut self, error: Error);
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

/// calloop source associated with a Device
pub type DrmSource<A> = Generic<DrmDevice<A>>;

/// Bind a `Device` to an [`EventLoop`](calloop::EventLoop),
///
/// This will cause it to recieve events and feed them into a previously
/// set [`DeviceHandler`](DeviceHandler).
pub fn device_bind<A, Data>(
    handle: &LoopHandle<Data>,
    device: DrmDevice<A>,
) -> ::std::result::Result<Source<DrmSource<A>>, InsertError<DrmSource<A>>>
where
    A: AsRawFd + 'static,
    Data: 'static,
{
    let source = Generic::new(device, calloop::Interest::Readable, calloop::Mode::Level);

    handle.insert_source(source, |_, source, _| {
        source.process_events();
        Ok(())
    })
}
