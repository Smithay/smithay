use std::cell::RefCell;
use std::collections::HashSet;
use std::convert::TryFrom;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{atomic::AtomicBool, Arc};

use calloop::{generic::Generic, InsertError, LoopHandle, Source};
use drm::control::{
    connector, crtc, plane, property, Device as ControlDevice, Event, Mode, PlaneResourceHandles, PlaneType,
    ResourceHandles,
};
use drm::{ClientCapability, Device as BasicDevice, DriverCapability};
use nix::libc::dev_t;
use nix::sys::stat::fstat;

pub(super) mod atomic;
pub(super) mod legacy;
use super::error::Error;
use super::surface::{atomic::AtomicDrmSurface, legacy::LegacyDrmSurface, DrmSurface, DrmSurfaceInternal};
use crate::backend::allocator::{Format, Fourcc, Modifier};
use atomic::AtomicDrmDevice;
use legacy::LegacyDrmDevice;

/// An open drm device
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
        let internal = Arc::new(DrmDevice::create_internal(
            dev,
            active,
            disable_connectors,
            log.clone(),
        )?);

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
            if dev.set_client_capability(ClientCapability::Atomic, true).is_ok() && !force_legacy {
                DrmDeviceInternal::Atomic(AtomicDrmDevice::new(dev, active, disable_connectors, log)?)
            } else {
                info!(log, "Falling back to LegacyDrmDevice");
                DrmDeviceInternal::Legacy(LegacyDrmDevice::new(dev, active, disable_connectors, log)?)
            },
        )
    }

    /// Processes any open events of the underlying file descriptor.
    ///
    /// You should not call this function manually, but rather use
    /// [`device_bind`] to register the device
    /// to an [`EventLoop`](calloop::EventLoop)
    /// and call this function when the device becomes readable
    /// to synchronize your rendering to the vblank events of the open crtc's
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

    /// Returns if the underlying implementation uses atomic-modesetting or not.
    pub fn is_atomic(&self) -> bool {
        match *self.internal {
            DrmDeviceInternal::Atomic(_) => true,
            DrmDeviceInternal::Legacy(_) => false,
        }
    }

    /// Assigns a [`DeviceHandler`] called during event processing.
    ///
    /// See [`device_bind`] and [`DeviceHandler`]
    pub fn set_handler(&mut self, handler: impl DeviceHandler + 'static) {
        let handler = Some(Box::new(handler) as Box<dyn DeviceHandler + 'static>);
        *self.handler.borrow_mut() = handler;
    }

    /// Clear a set [`DeviceHandler`](trait.DeviceHandler.html), if any
    pub fn clear_handler(&mut self) {
        self.handler.borrow_mut().take();
    }

    /// Returns a list of crtcs for this device
    pub fn crtcs(&self) -> &[crtc::Handle] {
        self.resources.crtcs()
    }

    /// Returns a set of available planes for a given crtc
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
                    PlaneType::Primary => {
                        primary = Some(*plane);
                    }
                    PlaneType::Cursor => {
                        cursor = Some(*plane);
                    }
                    PlaneType::Overlay => {
                        overlay.push(*plane);
                    }
                };
            }
        }

        Ok(Planes {
            primary: primary.expect("Crtc has no primary plane"),
            cursor,
            overlay: if self.has_universal_planes {
                Some(overlay)
            } else {
                None
            },
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

    /// Creates a new rendering surface.
    ///
    /// # Arguments
    ///
    /// Initialization of surfaces happens through the types provided by
    /// [`drm-rs`](drm).
    ///
    /// - [`crtc`](drm::control::crtc)s represent scanout engines of the device pointing to one framebuffer. \
    ///     Their responsibility is to read the data of the framebuffer and export it into an "Encoder". \
    ///     The number of crtc's represent the number of independant output devices the hardware may handle.
    /// - [`plane`](drm::control::plane)s represent a single plane on a crtc, which is composite together with
    ///     other planes on the same crtc to present the final image.
    /// - [`mode`](drm::control::Mode) describes the resolution and rate of images produced by the crtc and \
    ///     has to be compatible with the provided `connectors`.
    /// - [`connectors`] - List of connectors driven by the crtc. At least one(!) connector needs to be \
    ///     attached to a crtc in smithay.
    pub fn create_surface(
        &self,
        crtc: crtc::Handle,
        plane: plane::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<DrmSurface<A>, Error> {
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(crtc));
        }

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
            if self.plane_type(plane)? != PlaneType::Primary {
                return Err(Error::NonPrimaryPlane(plane));
            }

            DrmSurfaceInternal::Legacy(LegacyDrmSurface::new(
                self.internal.clone(),
                active,
                crtc,
                mode,
                connectors,
                self.logger.clone(),
            )?)
        };

        // get plane formats
        let plane_info = self.get_plane(plane).map_err(|source| Error::Access {
            errmsg: "Error loading plane info",
            dev: self.dev_path(),
            source,
        })?;
        let mut formats = HashSet::new();
        for code in plane_info
            .formats()
            .iter()
            .flat_map(|x| Fourcc::try_from(*x).ok())
        {
            formats.insert(Format {
                code,
                modifier: Modifier::Invalid,
            });
        }

        if let Ok(1) = 
            self.get_driver_capability(DriverCapability::AddFB2Modifiers)
        {
            let set = self.get_properties(plane).map_err(|source| Error::Access {
                errmsg: "Failed to query properties",
                dev: self.dev_path(),
                source,
            })?;
            let (handles, _) = set.as_props_and_values();
            // for every handle ...
            let prop = handles.iter().find(|handle| {
                // get information of that property
                if let Some(info) = self.get_property(**handle).ok() {
                    // to find out, if we got the handle of the "IN_FORMATS" property ...
                    if info.name().to_str().map(|x| x == "IN_FORMATS").unwrap_or(false) {
                        // so we can use that to get formats
                        return true;
                    }
                }
                false
            }).copied();
            if let Some(prop) = prop {
                let prop_info = self.get_property(prop).map_err(|source| Error::Access {
                    errmsg: "Failed to query property",
                    dev: self.dev_path(),
                    source,
                })?;
                let (handles, raw_values) = set.as_props_and_values();
                let raw_value = raw_values[handles
                    .iter()
                    .enumerate()
                    .find_map(|(i, handle)| if *handle == prop { Some(i) } else { None })
                    .unwrap()];
                if let property::Value::Blob(blob) = prop_info.value_type().convert_value(raw_value) {
                    let data = self.get_property_blob(blob).map_err(|source| Error::Access {
                        errmsg: "Failed to query property blob data",
                        dev: self.dev_path(),
                        source,
                    })?;
                    // be careful here, we have no idea about the alignment inside the blob, so always copy using `read_unaligned`,
                    // although slice::from_raw_parts would be so much nicer to iterate and to read.
                    unsafe {
                        let fmt_mod_blob_ptr = data.as_ptr() as *const drm_ffi::drm_format_modifier_blob;
                        let fmt_mod_blob = &*fmt_mod_blob_ptr;

                        let formats_ptr: *const u32 = fmt_mod_blob_ptr
                            .cast::<u8>()
                            .offset(fmt_mod_blob.formats_offset as isize)
                            as *const _;
                        let modifiers_ptr: *const drm_ffi::drm_format_modifier = fmt_mod_blob_ptr
                            .cast::<u8>()
                            .offset(fmt_mod_blob.modifiers_offset as isize)
                            as *const _;
                        let formats_ptr = formats_ptr as *const u32;
                        let modifiers_ptr = modifiers_ptr as *const drm_ffi::drm_format_modifier;

                        for i in 0..fmt_mod_blob.count_modifiers {
                            let mod_info = modifiers_ptr.offset(i as isize).read_unaligned();
                            for j in 0..64 {
                                if mod_info.formats & (1u64 << j) != 0 {
                                    let code = Fourcc::try_from(
                                        formats_ptr
                                            .offset((j + mod_info.offset) as isize)
                                            .read_unaligned(),
                                    )
                                    .ok();
                                    let modifier = Modifier::from(mod_info.modifier);
                                    if let Some(code) = code {
                                        formats.insert(Format { code, modifier });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if self.plane_type(plane)? == PlaneType::Cursor {
            // Force a LINEAR layout for the cursor if the driver doesn't support modifiers
            for format in formats.clone() {
                formats.insert(Format {
                    code: format.code,
                    modifier: Modifier::Linear,
                });
            }
        }

        if formats.is_empty() {
            formats.insert(Format {
                code: Fourcc::Argb8888,
                modifier: Modifier::Invalid,
            });
        }

        trace!(
            self.logger,
            "Supported scan-out formats for plane ({:?}): {:?}", plane, formats
        );

        Ok(DrmSurface {
            dev_id: self.dev_id,
            crtc,
            plane,
            internal: Arc::new(internal),
            formats,
            #[cfg(feature = "backend_session")]
            links: RefCell::new(Vec::new()),
        })
    }

    /// Returns the device_id of the underlying drm node
    pub fn device_id(&self) -> dev_t {
        self.dev_id
    }
}

/// A set of planes as supported by a crtc
pub struct Planes {
    /// The primary plane of the crtc
    pub primary: plane::Handle,
    /// The cursor plane of the crtc, if available
    pub cursor: Option<plane::Handle>,
    /// Overlay planes supported by the crtc, if available
    pub overlay: Option<Vec<plane::Handle>>,
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
