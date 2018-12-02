use super::{DevPath, Device, DeviceHandler, RawDevice, Surface};

use drm::control::{connector, crtc, encoder, Device as ControlDevice, Mode, ResourceHandles, ResourceInfo};
use drm::Device as BasicDevice;
use nix::libc::dev_t;
use nix::sys::stat::fstat;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::iter::FromIterator;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

mod surface;
pub use self::surface::LegacyDrmSurface;
use self::surface::{LegacyDrmSurfaceInternal, State};

pub mod error;
use self::error::*;

#[cfg(feature = "backend_session")]
pub mod session;

pub struct LegacyDrmDevice<A: AsRawFd + 'static> {
    dev: Rc<Dev<A>>,
    dev_id: dev_t,
    active: Arc<AtomicBool>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<LegacyDrmSurfaceInternal<A>>>>>,
    handler: Option<RefCell<Box<DeviceHandler<Device = LegacyDrmDevice<A>>>>>,
    logger: ::slog::Logger,
}

pub(in crate::backend::drm) struct Dev<A: AsRawFd + 'static> {
    fd: A,
    priviledged: bool,
    active: Arc<AtomicBool>,
    old_state: HashMap<crtc::Handle, (crtc::Info, Vec<connector::Handle>)>,
    logger: ::slog::Logger,
}
impl<A: AsRawFd + 'static> AsRawFd for Dev<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl<A: AsRawFd + 'static> BasicDevice for Dev<A> {}
impl<A: AsRawFd + 'static> ControlDevice for Dev<A> {}
impl<A: AsRawFd + 'static> Drop for Dev<A> {
    fn drop(&mut self) {
        info!(self.logger, "Dropping device: {:?}", self.dev_path());
        if self.active.load(Ordering::SeqCst) {
            let old_state = self.old_state.clone();
            for (handle, (info, connectors)) in old_state {
                if let Err(err) = crtc::set(
                    &*self,
                    handle,
                    info.fb(),
                    &connectors,
                    info.position(),
                    info.mode(),
                ) {
                    error!(self.logger, "Failed to reset crtc ({:?}). Error: {}", handle, err);
                }
            }
        }
        if self.priviledged {
            if let Err(err) = self.drop_master() {
                error!(self.logger, "Failed to drop drm master state. Error: {}", err);
            }
        }
    }
}

impl<A: AsRawFd + 'static> LegacyDrmDevice<A> {
    /// Create a new `LegacyDrmDevice` from an open drm node
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new<L>(dev: A, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm"));
        info!(log, "DrmDevice initializing");

        let dev_id = fstat(dev.as_raw_fd())
            .chain_err(|| ErrorKind::UnableToGetDeviceId)?
            .st_rdev;

        let active = Arc::new(AtomicBool::new(true));
        let mut dev = Dev {
            fd: dev,
            priviledged: true,
            old_state: HashMap::new(),
            active: active.clone(),
            logger: log.clone(),
        };

        // we want to modeset, so we better be the master, if we run via a tty session
        if dev.set_master().is_err() {
            warn!(log, "Unable to become drm master, assuming unpriviledged mode");
            dev.priviledged = false;
        };

        let res_handles = ControlDevice::resource_handles(&dev).chain_err(|| {
            ErrorKind::DrmDev(format!("Error loading drm resources on {:?}", dev.dev_path()))
        })?;
        for &con in res_handles.connectors() {
            let con_info = connector::Info::load_from_device(&dev, con).chain_err(|| {
                ErrorKind::DrmDev(format!("Error loading connector info on {:?}", dev.dev_path()))
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = encoder::Info::load_from_device(&dev, enc).chain_err(|| {
                    ErrorKind::DrmDev(format!("Error loading encoder info on {:?}", dev.dev_path()))
                })?;
                if let Some(crtc) = enc_info.current_crtc() {
                    let info = crtc::Info::load_from_device(&dev, crtc).chain_err(|| {
                        ErrorKind::DrmDev(format!("Error loading crtc info on {:?}", dev.dev_path()))
                    })?;
                    dev.old_state
                        .entry(crtc)
                        .or_insert((info, Vec::new()))
                        .1
                        .push(con);
                }
            }
        }

        Ok(LegacyDrmDevice {
            // Open the drm device and create a context based on that
            dev: Rc::new(dev),
            dev_id,
            active,
            backends: Rc::new(RefCell::new(HashMap::new())),
            handler: None,
            logger: log.clone(),
        })
    }
}

impl<A: AsRawFd + 'static> AsRawFd for LegacyDrmDevice<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.dev.as_raw_fd()
    }
}

impl<A: AsRawFd + 'static> BasicDevice for LegacyDrmDevice<A> {}
impl<A: AsRawFd + 'static> ControlDevice for LegacyDrmDevice<A> {}

impl<A: AsRawFd + 'static> Device for LegacyDrmDevice<A> {
    type Surface = LegacyDrmSurface<A>;

    fn device_id(&self) -> dev_t {
        self.dev_id
    }

    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static) {
        self.handler = Some(RefCell::new(Box::new(handler)));
    }

    fn clear_handler(&mut self) {
        let _ = self.handler.take();
    }

    fn create_surface(&mut self, crtc: crtc::Handle) -> Result<LegacyDrmSurface<A>> {
        if self.backends.borrow().contains_key(&crtc) {
            bail!(ErrorKind::CrtcAlreadyInUse(crtc));
        }

        if !self.active.load(Ordering::SeqCst) {
            bail!(ErrorKind::DeviceInactive);
        }

        let crtc_info = crtc::Info::load_from_device(self, crtc)
            .chain_err(|| ErrorKind::DrmDev(format!("Error loading crtc info on {:?}", self.dev_path())))?;

        let mode = crtc_info.mode();

        let mut connectors = HashSet::new();
        let res_handles = ControlDevice::resource_handles(self).chain_err(|| {
            ErrorKind::DrmDev(format!("Error loading drm resources on {:?}", self.dev_path()))
        })?;
        for &con in res_handles.connectors() {
            let con_info = connector::Info::load_from_device(self, con).chain_err(|| {
                ErrorKind::DrmDev(format!("Error loading connector info on {:?}", self.dev_path()))
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = encoder::Info::load_from_device(self, enc).chain_err(|| {
                    ErrorKind::DrmDev(format!("Error loading encoder info on {:?}", self.dev_path()))
                })?;
                if let Some(current_crtc) = enc_info.current_crtc() {
                    if crtc == current_crtc {
                        connectors.insert(con);
                    }
                }
            }
        }

        let state = State { mode, connectors };
        let backend = Rc::new(LegacyDrmSurfaceInternal {
            dev: self.dev.clone(),
            crtc,
            state: RwLock::new(state.clone()),
            pending: RwLock::new(state),
            logger: self.logger.new(o!("crtc" => format!("{:?}", crtc))),
        });

        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(LegacyDrmSurface(backend))
    }

    fn process_events(&mut self) {
        match crtc::receive_events(self) {
            Ok(events) => for event in events {
                if let crtc::Event::PageFlip(event) = event {
                    if self.active.load(Ordering::SeqCst) {
                        if self
                            .backends
                            .borrow()
                            .get(&event.crtc)
                            .iter()
                            .flat_map(|x| x.upgrade())
                            .next()
                            .is_some()
                        {
                            trace!(self.logger, "Handling event for backend {:?}", event.crtc);
                            if let Some(handler) = self.handler.as_ref() {
                                handler.borrow_mut().vblank(event.crtc);
                            }
                        } else {
                            self.backends.borrow_mut().remove(&event.crtc);
                        }
                    }
                }
            },
            Err(err) => if let Some(handler) = self.handler.as_ref() {
                handler.borrow_mut().error(
                    ResultExt::<()>::chain_err(Err(err), || {
                        ErrorKind::DrmDev(format!("Error processing drm events on {:?}", self.dev_path()))
                    }).unwrap_err(),
                );
            },
        }
    }

    fn resource_info<T: ResourceInfo>(&self, handle: T::Handle) -> Result<T> {
        T::load_from_device(self, handle)
            .chain_err(|| ErrorKind::DrmDev(format!("Error loading resource info on {:?}", self.dev_path())))
    }

    fn resource_handles(&self) -> Result<ResourceHandles> {
        ControlDevice::resource_handles(self)
            .chain_err(|| ErrorKind::DrmDev(format!("Error loading resource info on {:?}", self.dev_path())))
    }
}

impl<A: AsRawFd + 'static> RawDevice for LegacyDrmDevice<A> {
    type Surface = LegacyDrmSurface<A>;
}

impl<A: AsRawFd + 'static> Drop for LegacyDrmDevice<A> {
    fn drop(&mut self) {
        self.clear_handler();
    }
}
