//!
//! [`RawDevice`](RawDevice) and [`RawSurface`](RawSurface)
//! implementations using the legacy mode-setting infrastructure.
//!
//! Usually this implementation will be wrapped into a [`GbmDevice`](::backend::drm::gbm::GbmDevice).
//! Take a look at `anvil`s source code for an example of this.
//!
//! For an example how to use this standalone, take a look at the `raw_legacy_drm` example.
//!

use super::{common::Error, DevPath, Device, DeviceHandler, RawDevice};

use drm::control::{
    connector, crtc, encoder, framebuffer, plane, Device as ControlDevice, Event, Mode, ResourceHandles,
};
use drm::{Device as BasicDevice, SystemError as DrmError};
use nix::libc::dev_t;
use nix::sys::stat::fstat;

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use failure::{Fail, ResultExt};

mod surface;
pub use self::surface::LegacyDrmSurface;
use self::surface::LegacyDrmSurfaceInternal;

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
    privileged: bool,
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
        if self.privileged {
            if let Err(err) = self.release_master_lock() {
                error!(self.logger, "Failed to drop drm master state. Error: {}", err);
            }
        }
    }
}

impl<A: AsRawFd + 'static> LegacyDrmDevice<A> {
    /// Create a new [`LegacyDrmDevice`] from an open drm node
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
    pub fn new<L>(dev: A, disable_connectors: bool, logger: L) -> Result<Self, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm"));
        info!(log, "LegacyDrmDevice initializing");

        let dev_id = fstat(dev.as_raw_fd())
            .map_err(Error::UnableToGetDeviceId)?
            .st_rdev;

        let active = Arc::new(AtomicBool::new(true));
        let mut dev = Dev {
            fd: dev,
            privileged: true,
            old_state: HashMap::new(),
            active: active.clone(),
            logger: log.clone(),
        };

        // we want to modeset, so we better be the master, if we run via a tty session
        if dev.acquire_master_lock().is_err() {
            warn!(log, "Unable to become drm master, assuming unprivileged mode");
            dev.privileged = false;
        };

        // enumerate (and save) the current device state
        let res_handles = ControlDevice::resource_handles(&dev)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error loading drm resources",
                dev: dev.dev_path(),
                source,
            })?;
        for &con in res_handles.connectors() {
            let con_info = dev.get_connector(con).compat().map_err(|source| Error::Access {
                errmsg: "Error loading connector info",
                dev: dev.dev_path(),
                source,
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = dev.get_encoder(enc).compat().map_err(|source| Error::Access {
                    errmsg: "Error loading encoder info",
                    dev: dev.dev_path(),
                    source,
                })?;
                if let Some(crtc) = enc_info.crtc() {
                    let info = dev.get_crtc(crtc).compat().map_err(|source| Error::Access {
                        errmsg: "Error loading crtc info",
                        dev: dev.dev_path(),
                        source,
                    })?;
                    dev.old_state
                        .entry(crtc)
                        .or_insert((info, Vec::new()))
                        .1
                        .push(con);
                }
            }
        }

        if disable_connectors {
            dev.set_connector_state(res_handles.connectors().iter().copied(), false)?;

            for crtc in res_handles.crtcs() {
                // null commit
                dev.set_crtc(*crtc, None, (0, 0), &[], None)
                    .compat()
                    .map_err(|source| Error::Access {
                        errmsg: "Error setting crtc",
                        dev: dev.dev_path(),
                        source,
                    })?;
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

impl<A: AsRawFd + 'static> Dev<A> {
    pub(in crate::backend::drm::legacy) fn set_connector_state(
        &self,
        connectors: impl Iterator<Item = connector::Handle>,
        enabled: bool,
    ) -> Result<(), Error> {
        for conn in connectors {
            let info = self
                .get_connector(conn)
                .compat()
                .map_err(|source| Error::Access {
                    errmsg: "Failed to get connector infos",
                    dev: self.dev_path(),
                    source,
                })?;
            if info.state() == connector::State::Connected {
                let props = self
                    .get_properties(conn)
                    .compat()
                    .map_err(|source| Error::Access {
                        errmsg: "Failed to get properties for connector",
                        dev: self.dev_path(),
                        source,
                    })?;
                let (handles, _) = props.as_props_and_values();
                for handle in handles {
                    let info = self
                        .get_property(*handle)
                        .compat()
                        .map_err(|source| Error::Access {
                            errmsg: "Failed to get property of connector",
                            dev: self.dev_path(),
                            source,
                        })?;
                    if info.name().to_str().map(|x| x == "DPMS").unwrap_or(false) {
                        self.set_property(
                            conn,
                            *handle,
                            if enabled {
                                0 /*DRM_MODE_DPMS_ON*/
                            } else {
                                3 /*DRM_MODE_DPMS_OFF*/
                            },
                        )
                        .compat()
                        .map_err(|source| Error::Access {
                            errmsg: "Failed to set property of connector",
                            dev: self.dev_path(),
                            source,
                        })?;
                    }
                }
            }
        }
        Ok(())
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

    fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<LegacyDrmSurface<A>, Error> {
        if self.backends.borrow().contains_key(&crtc) {
            return Err(Error::CrtcAlreadyInUse(crtc));
        }

        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(crtc));
        }

        let backend = Rc::new(LegacyDrmSurfaceInternal::new(
            self.dev.clone(),
            crtc,
            mode,
            connectors,
            self.logger.new(o!("crtc" => format!("{:?}", crtc))),
        )?);

        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(LegacyDrmSurface(backend))
    }

    fn process_events(&mut self) {
        match self.receive_events() {
            Ok(events) => {
                for event in events {
                    if let Event::PageFlip(event) = event {
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
                    } else {
                        trace!(self.logger, "Unrelated event");
                    }
                }
            }
            Err(source) => {
                if let Some(handler) = self.handler.as_ref() {
                    handler.borrow_mut().error(Error::Access {
                        errmsg: "Error processing drm events",
                        dev: self.dev_path(),
                        source: source.compat(),
                    });
                }
            }
        }
    }

    fn resource_handles(&self) -> Result<ResourceHandles, Error> {
        ControlDevice::resource_handles(self)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error loading resource info",
                dev: self.dev_path(),
                source,
            })
    }

    fn get_connector_info(&self, conn: connector::Handle) -> Result<connector::Info, DrmError> {
        self.get_connector(conn)
    }
    fn get_crtc_info(&self, crtc: crtc::Handle) -> Result<crtc::Info, DrmError> {
        self.get_crtc(crtc)
    }
    fn get_encoder_info(&self, enc: encoder::Handle) -> Result<encoder::Info, DrmError> {
        self.get_encoder(enc)
    }
    fn get_framebuffer_info(&self, fb: framebuffer::Handle) -> Result<framebuffer::Info, DrmError> {
        self.get_framebuffer(fb)
    }
    fn get_plane_info(&self, plane: plane::Handle) -> Result<plane::Info, DrmError> {
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
