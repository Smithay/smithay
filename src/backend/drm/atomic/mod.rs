//!
//! [`RawDevice`](RawDevice) and [`RawSurface`](RawSurface)
//! implementations using the atomic mode-setting infrastructure.
//!
//! Usually this implementation will wrapped into a [`GbmDevice`](::backend::drm::gbm::GbmDevice).
//! Take a look at `anvil`s source code for an example of this.
//!
//! For an example how to use this standalone, take a look at the raw_atomic_drm example.
//!

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use drm::control::{atomic::AtomicModeReq, AtomicCommitFlags, Device as ControlDevice, Event};
use drm::control::{
    connector, crtc, encoder, framebuffer, plane, property, Mode, PropertyValueSet, ResourceHandle,
    ResourceHandles,
};
use drm::SystemError as DrmError;
use drm::{ClientCapability, Device as BasicDevice};
use failure::{Fail, ResultExt};
use nix::libc::dev_t;
use nix::sys::stat::fstat;

use super::{common::Error, DevPath, Device, DeviceHandler, RawDevice};

mod surface;
pub use self::surface::AtomicDrmSurface;
use self::surface::AtomicDrmSurfaceInternal;

#[cfg(feature = "backend_session")]
pub mod session;

/// Open raw drm device utilizing atomic mode-setting
pub struct AtomicDrmDevice<A: AsRawFd + 'static> {
    dev: Rc<Dev<A>>,
    dev_id: dev_t,
    active: Arc<AtomicBool>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<AtomicDrmSurfaceInternal<A>>>>>,
    handler: Option<RefCell<Box<dyn DeviceHandler<Device = AtomicDrmDevice<A>>>>>,
    logger: ::slog::Logger,
}

type OldState = (
    Vec<(connector::Handle, PropertyValueSet)>,
    Vec<(crtc::Handle, PropertyValueSet)>,
    Vec<(framebuffer::Handle, PropertyValueSet)>,
    Vec<(plane::Handle, PropertyValueSet)>,
);

type Mapping = (
    HashMap<connector::Handle, HashMap<String, property::Handle>>,
    HashMap<crtc::Handle, HashMap<String, property::Handle>>,
    HashMap<framebuffer::Handle, HashMap<String, property::Handle>>,
    HashMap<plane::Handle, HashMap<String, property::Handle>>,
);

struct Dev<A: AsRawFd + 'static> {
    fd: A,
    privileged: bool,
    active: Arc<AtomicBool>,
    old_state: OldState,
    prop_mapping: Mapping,
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
            // Here we restore the card/tty's to it's previous state.
            // In case e.g. getty was running on the tty sets the correct framebuffer again,
            // so that getty will be visible.

            let mut req = AtomicModeReq::new();

            fn add_multiple_props<T: ResourceHandle>(
                req: &mut AtomicModeReq,
                old_state: &[(T, PropertyValueSet)],
            ) {
                for (handle, set) in old_state {
                    let (prop_handles, values) = set.as_props_and_values();
                    for (&prop_handle, &val) in prop_handles.iter().zip(values.iter()) {
                        req.add_raw_property((*handle).into(), prop_handle, val);
                    }
                }
            };

            add_multiple_props(&mut req, &self.old_state.0);
            add_multiple_props(&mut req, &self.old_state.1);
            add_multiple_props(&mut req, &self.old_state.2);
            add_multiple_props(&mut req, &self.old_state.3);

            if let Err(err) = self.atomic_commit(&[AtomicCommitFlags::AllowModeset], req) {
                error!(self.logger, "Failed to restore previous state. Error: {}", err);
            }
        }
        if self.privileged {
            if let Err(err) = self.release_master_lock() {
                error!(self.logger, "Failed to drop drm master state. Error: {}", err);
            }
        }
    }
}

impl<A: AsRawFd + 'static> Dev<A> {
    // Add all properties of given handles to a given drm resource type to state.
    // You may use this to snapshot the current state of the drm device (fully or partially).
    fn add_props<T>(&self, handles: &[T], state: &mut Vec<(T, PropertyValueSet)>) -> Result<(), Error>
    where
        A: AsRawFd + 'static,
        T: ResourceHandle,
    {
        let iter = handles.iter().map(|x| (x, self.get_properties(*x)));
        if let Some(len) = iter.size_hint().1 {
            state.reserve_exact(len)
        }

        iter.map(|(x, y)| (*x, y))
            .try_for_each(|(x, y)| match y {
                Ok(y) => {
                    state.push((x, y));
                    Ok(())
                }
                Err(err) => Err(err),
            })
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error reading properties",
                dev: self.dev_path(),
                source,
            })
    }

    /// Create a mapping of property names and handles for given handles of a given drm resource type.
    /// You may use this to easily lookup properties by name instead of going through this procedure manually.
    fn map_props<T>(
        &self,
        handles: &[T],
        mapping: &mut HashMap<T, HashMap<String, property::Handle>>,
    ) -> Result<(), Error>
    where
        A: AsRawFd + 'static,
        T: ResourceHandle + Eq + std::hash::Hash,
    {
        handles
            .iter()
            .map(|x| (x, self.get_properties(*x)))
            .try_for_each(|(handle, props)| {
                let mut map = HashMap::new();
                match props {
                    Ok(props) => {
                        let (prop_handles, _) = props.as_props_and_values();
                        for prop in prop_handles {
                            if let Ok(info) = self.get_property(*prop) {
                                let name = info.name().to_string_lossy().into_owned();
                                map.insert(name, *prop);
                            }
                        }
                        mapping.insert(*handle, map);
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            })
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error reading properties on {:?}",
                dev: self.dev_path(),
                source,
            })
    }
}

impl<A: AsRawFd + 'static> AtomicDrmDevice<A> {
    /// Create a new [`AtomicDrmDevice`] from an open drm node
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
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm"));
        info!(log, "AtomicDrmDevice initializing");

        let dev_id = fstat(fd.as_raw_fd()).map_err(Error::UnableToGetDeviceId)?.st_rdev;

        let active = Arc::new(AtomicBool::new(true));
        let mut dev = Dev {
            fd,
            privileged: true,
            active: active.clone(),
            old_state: (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
            prop_mapping: (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new()),
            logger: log.clone(),
        };

        // we want to modeset, so we better be the master, if we run via a tty session
        if dev.acquire_master_lock().is_err() {
            warn!(log, "Unable to become drm master, assuming unprivileged mode");
            dev.privileged = false;
        };

        // enable the features we need
        dev.set_client_capability(ClientCapability::UniversalPlanes, true)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error enabling UniversalPlanes",
                dev: dev.dev_path(),
                source,
            })?;
        dev.set_client_capability(ClientCapability::Atomic, true)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error enabling AtomicModesetting",
                dev: dev.dev_path(),
                source,
            })?;

        // enumerate (and save) the current device state
        let res_handles = ControlDevice::resource_handles(&dev)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error loading drm resources",
                dev: dev.dev_path(),
                source,
            })?;

        let plane_handles = dev.plane_handles().compat().map_err(|source| Error::Access {
            errmsg: "Error loading planes",
            dev: dev.dev_path(),
            source,
        })?;
        let planes = plane_handles.planes();

        let mut old_state = dev.old_state.clone();
        let mut mapping = dev.prop_mapping.clone();

        dev.add_props(res_handles.connectors(), &mut old_state.0)?;
        dev.add_props(res_handles.crtcs(), &mut old_state.1)?;
        dev.add_props(res_handles.framebuffers(), &mut old_state.2)?;
        dev.add_props(planes, &mut old_state.3)?;

        dev.map_props(res_handles.connectors(), &mut mapping.0)?;
        dev.map_props(res_handles.crtcs(), &mut mapping.1)?;
        dev.map_props(res_handles.framebuffers(), &mut mapping.2)?;
        dev.map_props(planes, &mut mapping.3)?;

        dev.old_state = old_state;
        dev.prop_mapping = mapping;
        trace!(log, "Mapping: {:#?}", dev.prop_mapping);

        if disable_connectors {
            // Disable all connectors as initial state
            let mut req = AtomicModeReq::new();
            for conn in res_handles.connectors() {
                let prop = dev
                    .prop_mapping
                    .0
                    .get(&conn)
                    .expect("Unknown handle")
                    .get("CRTC_ID")
                    .expect("Unknown property CRTC_ID");
                req.add_property(*conn, *prop, property::Value::CRTC(None));
            }
            // A crtc without a connector has no mode, we also need to reset that.
            // Otherwise the commit will not be accepted.
            for crtc in res_handles.crtcs() {
                let active_prop = dev
                    .prop_mapping
                    .1
                    .get(&crtc)
                    .expect("Unknown handle")
                    .get("ACTIVE")
                    .expect("Unknown property ACTIVE");
                let mode_prop = dev
                    .prop_mapping
                    .1
                    .get(&crtc)
                    .expect("Unknown handle")
                    .get("MODE_ID")
                    .expect("Unknown property MODE_ID");
                req.add_property(*crtc, *mode_prop, property::Value::Unknown(0));
                req.add_property(*crtc, *active_prop, property::Value::Boolean(false));
            }
            dev.atomic_commit(&[AtomicCommitFlags::AllowModeset], req)
                .compat()
                .map_err(|source| Error::Access {
                    errmsg: "Failed to disable connectors",
                    dev: dev.dev_path(),
                    source,
                })?;
        }

        Ok(AtomicDrmDevice {
            dev: Rc::new(dev),
            dev_id,
            active,
            backends: Rc::new(RefCell::new(HashMap::new())),
            handler: None,
            logger: log.clone(),
        })
    }
}

impl<A: AsRawFd + 'static> AsRawFd for AtomicDrmDevice<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.dev.as_raw_fd()
    }
}

impl<A: AsRawFd + 'static> BasicDevice for AtomicDrmDevice<A> {}
impl<A: AsRawFd + 'static> ControlDevice for AtomicDrmDevice<A> {}

impl<A: AsRawFd + 'static> Device for AtomicDrmDevice<A> {
    type Surface = AtomicDrmSurface<A>;

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
    ) -> Result<AtomicDrmSurface<A>, Error> {
        if self.backends.borrow().contains_key(&crtc) {
            return Err(Error::CrtcAlreadyInUse(crtc));
        }

        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(crtc));
        }

        let backend = Rc::new(AtomicDrmSurfaceInternal::new(
            self.dev.clone(),
            crtc,
            mode,
            connectors,
            self.logger.new(o!("crtc" => format!("{:?}", crtc))),
        )?);

        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(AtomicDrmSurface(backend))
    }

    fn process_events(&mut self) {
        match self.receive_events() {
            Ok(events) => {
                for event in events {
                    if let Event::PageFlip(event) = event {
                        trace!(self.logger, "Got a page-flip event for crtc ({:?})", event.crtc);
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
                        trace!(
                            self.logger,
                            "Got a non-page-flip event of device '{:?}'.",
                            self.dev_path()
                        );
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

impl<A: AsRawFd + 'static> RawDevice for AtomicDrmDevice<A> {
    type Surface = AtomicDrmSurface<A>;
}

impl<A: AsRawFd + 'static> Drop for AtomicDrmDevice<A> {
    fn drop(&mut self) {
        self.clear_handler();
    }
}
