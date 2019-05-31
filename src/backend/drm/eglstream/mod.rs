//!
//! [`Device`](Device) and [`Surface`](Surface) implementations using
//! the proposed EGLStream api for efficient rendering.
//! Currently this api is only implemented by the proprietary `nvidia` driver.
//!
//! Usually this implementation will be wrapped into a [`EglDevice`](::backend::drm::egl::EglDevice).
//!
//! To use these types standalone, you will need to render via egl yourself as page flips
//! are driven via `eglSwapBuffers`.
//!
//! To use this api in place of GBM for nvidia cards take a look at
//! [`FallbackDevice`](::backend::drm::common::fallback::FallbackDevice).
//! Take a look at `anvil`s source code for an example of this.
//!

use super::{Device, DeviceHandler, RawDevice, Surface};

use drm::buffer::format::PixelFormat;
use drm::control::{
    connector, crtc, encoder, framebuffer, plane, Device as ControlDevice, Mode, ResourceHandles,
};
use drm::SystemError as DrmError;
use failure::ResultExt;
use nix::libc::dev_t;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::{fs, ptr};

mod surface;
pub use self::surface::EglStreamSurface;
use self::surface::EglStreamSurfaceInternal;

pub mod egl;
#[cfg(feature = "backend_session")]
pub mod session;

use crate::backend::egl::ffi::{self, egl::types::EGLDeviceEXT};
use crate::backend::egl::{wrap_egl_call, EGLError as RawEGLError, Error as EglError};
use crate::backend::graphics::SwapBuffersError;

/// Errors thrown by the [`EglStreamDevice`](::backend::drm::eglstream::EglStreamDevice)
/// and [`EglStreamSurface`](::backend::drm::eglstream::EglStreamSurface).
#[derive(thiserror::Error, Debug)]
pub enum Error<U: std::error::Error + std::fmt::Debug + std::fmt::Display + 'static> {
    /// `eglInitialize` returned an error
    #[error("Failed to initialize EGL: {0:}")]
    InitFailed(#[source] RawEGLError),
    /// Failed to enumerate EGL_EXT_drm_device
    #[error("Failed to enumerate EGL_EXT_drm_device: {0:}")]
    FailedToEnumerateDevices(#[source] RawEGLError),
    /// Device is not compatible with EGL_EXT_drm_device extension
    #[error("Device is not compatible with EGL_EXT_drm_device extension")]
    DeviceIsNoEGLStreamDevice,
    /// Device has not suitable output layer
    #[error("Device has no suitable output layer")]
    DeviceNoOutputLayer,
    /// Device was unable to create an EGLStream
    #[error("EGLStream creation failed")]
    DeviceStreamCreationFailed,
    /// Underlying backend  error
    #[error("Underlying error: {0}")]
    Underlying(#[source] U),
    /// Buffer creation failed
    #[error("Buffer creation failed")]
    BufferCreationFailed(#[source] failure::Compat<DrmError>),
    /// Buffer write failed
    #[error("Buffer write failed")]
    BufferWriteFailed(#[source] failure::Compat<DrmError>),
    /// Stream flip failed
    #[error("Stream flip failed ({0})")]
    StreamFlipFailed(#[source] RawEGLError),
}

/// Representation of an open egl stream device to create egl rendering surfaces
pub struct EglStreamDevice<D: RawDevice + ControlDevice + 'static> {
    pub(self) dev: EGLDeviceEXT,
    raw: D,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<EglStreamSurfaceInternal<D>>>>>,
    logger: ::slog::Logger,
}

impl<D: RawDevice + ControlDevice + 'static> EglStreamDevice<D> {
    /// Try to create a new [`EglStreamDevice`] from an open device.
    ///
    /// Returns an error if the underlying device would not support the required EGL extensions.
    pub fn new<L>(mut raw: D, logger: L) -> Result<Self, Error<<<D as Device>::Surface as Surface>::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_eglstream"));

        raw.clear_handler();

        debug!(log, "Creating egl stream device");

        ffi::make_sure_egl_is_loaded();

        fn has_extensions(exts: &[String], check: &'static [&'static str]) -> Result<(), EglError> {
            check
                .iter()
                .map(|ext| {
                    if exts.iter().any(|s| *s == *ext) {
                        Ok(())
                    } else {
                        Err(EglError::EglExtensionNotSupported(check))
                    }
                })
                .collect()
        }

        let device = unsafe {
            // the first step is to query the list of extensions without any display, if supported
            let dp_extensions = {
                let p = wrap_egl_call(|| {
                    ffi::egl::QueryString(ffi::egl::NO_DISPLAY, ffi::egl::EXTENSIONS as i32)
                })
                .map_err(Error::InitFailed)?;

                // this possibility is available only with EGL 1.5 or EGL_EXT_platform_base, otherwise
                // `eglQueryString` returns an error
                if p.is_null() {
                    vec![]
                } else {
                    let p = CStr::from_ptr(p);
                    let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
                    list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
                }
            };
            debug!(log, "EGL No-Display Extensions: {:?}", dp_extensions);

            // we need either EGL_EXT_device_base or EGL_EXT_device_enumeration &_query
            if let Err(_err) = has_extensions(&dp_extensions, &["EGL_EXT_device_base"]) {
                has_extensions(
                    &dp_extensions,
                    &["EGL_EXT_device_enumeration", "EGL_EXT_device_query"],
                )
                .map_err(|_| Error::DeviceIsNoEGLStreamDevice)?;
            }

            let mut num_devices = 0;
            wrap_egl_call(|| ffi::egl::QueryDevicesEXT(0, ptr::null_mut(), &mut num_devices))
                .map_err(Error::FailedToEnumerateDevices)?;
            if num_devices == 0 {
                return Err(Error::DeviceIsNoEGLStreamDevice);
            }

            let mut devices = Vec::with_capacity(num_devices as usize);
            wrap_egl_call(|| ffi::egl::QueryDevicesEXT(num_devices, devices.as_mut_ptr(), &mut num_devices))
                .map_err(Error::FailedToEnumerateDevices)?;
            devices.set_len(num_devices as usize);
            debug!(log, "Devices: {:#?}, Count: {}", devices, num_devices);

            devices
                .into_iter()
                .find(|device| {
                    *device != ffi::egl::NO_DEVICE_EXT
                        && {
                            let device_extensions = {
                                let p = ffi::egl::QueryDeviceStringEXT(*device, ffi::egl::EXTENSIONS as i32);
                                if p.is_null() {
                                    vec![]
                                } else {
                                    let p = CStr::from_ptr(p);
                                    let list = String::from_utf8(p.to_bytes().to_vec())
                                        .unwrap_or_else(|_| String::new());
                                    list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
                                }
                            };

                            device_extensions.iter().any(|s| *s == "EGL_EXT_device_drm")
                        }
                        && {
                            let path = {
                                let p = ffi::egl::QueryDeviceStringEXT(
                                    *device,
                                    ffi::egl::DRM_DEVICE_FILE_EXT as i32,
                                );
                                if p.is_null() {
                                    String::new()
                                } else {
                                    let p = CStr::from_ptr(p);
                                    String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new())
                                }
                            };

                            match fs::metadata(&path) {
                                Ok(metadata) => metadata.rdev() == raw.device_id(),
                                Err(_) => false,
                            }
                        }
                })
                .ok_or(Error::DeviceIsNoEGLStreamDevice)?
        };

        Ok(EglStreamDevice {
            dev: device,
            raw,
            backends: Rc::new(RefCell::new(HashMap::new())),
            logger: log,
        })
    }
}

struct InternalDeviceHandler<D: RawDevice + ControlDevice + 'static> {
    handler: Box<dyn DeviceHandler<Device = EglStreamDevice<D>> + 'static>,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<EglStreamSurfaceInternal<D>>>>>,
    logger: ::slog::Logger,
}

impl<D: RawDevice + ControlDevice + 'static> DeviceHandler for InternalDeviceHandler<D> {
    type Device = D;

    fn vblank(&mut self, crtc: crtc::Handle) {
        if let Some(backends) = self.backends.upgrade() {
            if let Some(surface) = backends.borrow().get(&crtc) {
                if surface.upgrade().is_some() {
                    self.handler.vblank(crtc);
                }
            } else {
                warn!(
                    self.logger,
                    "Surface ({:?}) not managed by egl stream, event not handled.", crtc
                );
            }
        }
    }
    fn error(&mut self, error: <<D as Device>::Surface as Surface>::Error) {
        self.handler.error(Error::Underlying(error))
    }
}

impl<D: RawDevice + ControlDevice + 'static> Device for EglStreamDevice<D> {
    type Surface = EglStreamSurface<D>;

    fn device_id(&self) -> dev_t {
        self.raw.device_id()
    }

    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static) {
        self.raw.set_handler(InternalDeviceHandler {
            handler: Box::new(handler),
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        });
    }

    fn clear_handler(&mut self) {
        self.raw.clear_handler();
    }

    fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<EglStreamSurface<D>, Error<<<D as Device>::Surface as Surface>::Error>> {
        info!(self.logger, "Initializing EglStreamSurface");

        let drm_surface =
            Device::create_surface(&mut self.raw, crtc, mode, connectors).map_err(Error::Underlying)?;

        // initialize a buffer for the cursor image
        let cursor = Cell::new(Some((
            self.raw
                .create_dumb_buffer((1, 1), PixelFormat::ARGB8888)
                .compat()
                .map_err(Error::BufferCreationFailed)?,
            (0, 0),
        )));

        let backend = Rc::new(EglStreamSurfaceInternal {
            crtc: drm_surface,
            cursor,
            stream: RefCell::new(None),
            commit_buffer: Cell::new(None),
            logger: self.logger.new(o!("crtc" => format!("{:?}", crtc))),
            locked: std::sync::atomic::AtomicBool::new(false),
        });
        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(EglStreamSurface(backend))
    }

    fn process_events(&mut self) {
        self.raw.process_events()
    }

    fn resource_handles(&self) -> Result<ResourceHandles, Error<<<D as Device>::Surface as Surface>::Error>> {
        Device::resource_handles(&self.raw).map_err(Error::Underlying)
    }

    fn get_connector_info(&self, conn: connector::Handle) -> Result<connector::Info, DrmError> {
        self.raw.get_connector_info(conn)
    }
    fn get_crtc_info(&self, crtc: crtc::Handle) -> Result<crtc::Info, DrmError> {
        self.raw.get_crtc_info(crtc)
    }
    fn get_encoder_info(&self, enc: encoder::Handle) -> Result<encoder::Info, DrmError> {
        self.raw.get_encoder_info(enc)
    }
    fn get_framebuffer_info(&self, fb: framebuffer::Handle) -> Result<framebuffer::Info, DrmError> {
        self.raw.get_framebuffer_info(fb)
    }
    fn get_plane_info(&self, plane: plane::Handle) -> Result<plane::Info, DrmError> {
        self.raw.get_plane_info(plane)
    }
}

impl<D: RawDevice + ControlDevice + 'static> AsRawFd for EglStreamDevice<D> {
    fn as_raw_fd(&self) -> RawFd {
        self.raw.as_raw_fd()
    }
}

impl<D: RawDevice + ControlDevice + 'static> Drop for EglStreamDevice<D> {
    fn drop(&mut self) {
        self.clear_handler();
    }
}

impl<E> Into<SwapBuffersError> for Error<E>
where
    E: std::error::Error + Into<SwapBuffersError> + 'static,
{
    fn into(self) -> SwapBuffersError {
        match self {
            Error::BufferCreationFailed(x)
                if match x.get_ref() {
                    drm::SystemError::Unknown {
                        errno: nix::errno::Errno::EBUSY,
                    } => true,
                    drm::SystemError::Unknown {
                        errno: nix::errno::Errno::EINTR,
                    } => true,
                    _ => false,
                } =>
            {
                SwapBuffersError::TemporaryFailure(Box::new(Error::<E>::BufferCreationFailed(x)))
            }
            Error::BufferWriteFailed(x)
                if match x.get_ref() {
                    drm::SystemError::Unknown {
                        errno: nix::errno::Errno::EBUSY,
                    } => true,
                    drm::SystemError::Unknown {
                        errno: nix::errno::Errno::EINTR,
                    } => true,
                    _ => false,
                } =>
            {
                SwapBuffersError::TemporaryFailure(Box::new(Error::<E>::BufferCreationFailed(x)))
            }
            Error::StreamFlipFailed(x @ RawEGLError::ResourceBusy) => {
                SwapBuffersError::TemporaryFailure(Box::new(Error::<E>::StreamFlipFailed(x)))
            }
            Error::Underlying(x) => x.into(),
            x => SwapBuffersError::ContextLost(Box::new(x)),
        }
    }
}
