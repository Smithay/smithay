//!
//! [`RawDevice`](RawDevice) and [`RawSurface`](RawSurface)
//! implementations using the legacy mode-setting infrastructure.
//!
//! Usually this implementation will be wrapped into a [`GbmDevice`](::backend::drm::gbm::GbmDevice).
//! Take a look at `anvil`s source code for an example of this.
//!
//! For an example how to use this standalone, take a look at the `raw_drm` example.
//!

use super::{DevPath, Device, DeviceHandler, RawDevice};

use drm::control::{connector, crtc, encoder, framebuffer, plane, Device as ControlDevice, Event, ResourceHandles};
use drm::{Device as BasicDevice, SystemError as DrmError};
use failure::ResultExt as FailureResultExt;
use nix::libc::dev_t;
use nix::sys::stat::fstat;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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

/// Open raw drm device utilizing legacy mode-setting
pub struct LegacyDrmDevice<A: AsRawFd + 'static> {
    dev: Rc<Dev<A>>,
    dev_id: dev_t,
    active: Arc<AtomicBool>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<LegacyDrmSurfaceInternal<A>>>>>,
    handler: Option<RefCell<Box<dyn DeviceHandler<Device = LegacyDrmDevice<A>>>>>,
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
            // Here we restore the tty to it's previous state.
            // In case e.g. getty was running on the tty sets the correct framebuffer again,
            // so that getty will be visible.
            let old_state = self.old_state.clone();
            for (handle, (info, connectors)) in old_state {
                if let Err(err) = self.set_crtc(
                    handle,
                    info.framebuffer(),
                    info.position(),
                    &connectors,
                    info.mode(),
                ) {
                    error!(self.logger, "Failed to reset crtc ({:?}). Error: {}", handle, err);
                }
            }
        }
        if self.priviledged {
            if let Err(err) = self.release_master_lock() {
                error!(self.logger, "Failed to drop drm master state. Error: {}", err);
            }
        }
    }
}

impl<A: AsRawFd + 'static> LegacyDrmDevice<A> {
    /// Create a new [`LegacyDrmDevice`] from an open drm node
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new<L>(dev: A, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm"));
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
        if dev.acquire_master_lock().is_err() {
            warn!(log, "Unable to become drm master, assuming unpriviledged mode");
            dev.priviledged = false;
        };

        // enumerate (and save) the current device state
        let res_handles = ControlDevice::resource_handles(&dev).compat().chain_err(|| {
            ErrorKind::DrmDev(format!("Error loading drm resources on {:?}", dev.dev_path()))
        })?;
        for &con in res_handles.connectors() {
            let con_info = dev.get_connector(con).compat().chain_err(|| {
                ErrorKind::DrmDev(format!("Error loading connector info on {:?}", dev.dev_path()))
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = dev.get_encoder(enc).compat().chain_err(|| {
                    ErrorKind::DrmDev(format!("Error loading encoder info on {:?}", dev.dev_path()))
                })?;
                if let Some(crtc) = enc_info.crtc() {
                    let info = dev.get_crtc(crtc).compat().chain_err(|| {
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

        // Try to enumarate the current state to set the initial state variable correctly

        let crtc_info = self.get_crtc(crtc).compat()
            .chain_err(|| ErrorKind::DrmDev(format!("Error loading crtc info on {:?}", self.dev_path())))?;

        let mode = crtc_info.mode();

        let mut connectors = HashSet::new();
        let res_handles = ControlDevice::resource_handles(self).compat().chain_err(|| {
            ErrorKind::DrmDev(format!("Error loading drm resources on {:?}", self.dev_path()))
        })?;
        for &con in res_handles.connectors() {
            let con_info = self.get_connector(con).compat().chain_err(|| {
                ErrorKind::DrmDev(format!("Error loading connector info on {:?}", self.dev_path()))
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = self.get_encoder(enc).compat().chain_err(|| {
                    ErrorKind::DrmDev(format!("Error loading encoder info on {:?}", self.dev_path()))
                })?;
                if let Some(current_crtc) = enc_info.crtc() {
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
        match self.receive_events() {
            Ok(events) => {
                for event in events {
                    if let Event::PageFlip(event) = event {
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
                }
            }
            Err(err) => {
                if let Some(handler) = self.handler.as_ref() {
                    handler.borrow_mut().error(
                        ResultExt::<()>::chain_err(Err(err).compat(), || {
                            ErrorKind::DrmDev(format!("Error processing drm events on {:?}", self.dev_path()))
                        })
                        .unwrap_err(),
                    );
                }
            }
        }
    }

    fn resource_handles(&self) -> Result<ResourceHandles> {
        ControlDevice::resource_handles(self).compat()
            .chain_err(|| ErrorKind::DrmDev(format!("Error loading resource info on {:?}", self.dev_path())))
    }

    fn get_connector_info(&self, conn: connector::Handle) -> std::result::Result<connector::Info, DrmError> {
        self.get_connector(conn)
    }
    fn get_crtc_info(&self, crtc: crtc::Handle) -> std::result::Result<crtc::Info, DrmError> {
        self.get_crtc(crtc)
    }
    fn get_encoder_info(&self, enc: encoder::Handle) -> std::result::Result<encoder::Info, DrmError> {
        self.get_encoder(enc)
    }
    fn get_framebuffer_info(&self, fb: framebuffer::Handle) -> std::result::Result<framebuffer::Info, DrmError> {
        self.get_framebuffer(fb)
    }
    fn get_plane_info(&self, plane: plane::Handle) -> std::result::Result<plane::Info, DrmError> {
        self.get_plane(plane)
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
