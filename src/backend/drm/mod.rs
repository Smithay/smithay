

use backend::graphics::egl::{EGLContext, GlAttributes, PixelFormatRequirements};
use drm::Device as BasicDevice;
use drm::control::{connector, crtc, Mode};
use drm::control::Device as ControlDevice;

use gbm::Device as GbmDevice;

use nix;

use std::cell::RefCell;
use std::fs::File;
use std::io::Error as IoError;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::time::Duration;

use wayland_server::{EventLoopHandle};
use wayland_server::sources::{FdEventSourceHandler, FdInterest};

mod backend;
mod error;

pub use self::backend::{DrmBackend, Id};
use self::backend::DrmBackendInternal;
pub use self::error::{Error as DrmError, ModeError};

#[derive(Debug)]
pub(crate) struct DrmDev(File);

impl AsRawFd for DrmDev {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}
impl BasicDevice for DrmDev {}
impl ControlDevice for DrmDev {}

impl DrmDev {
    unsafe fn new_from_fd(fd: RawFd) -> Self {
        use std::os::unix::io::FromRawFd;
        DrmDev(File::from_raw_fd(fd))
    }

    fn new_from_file(file: File) -> Self {
        DrmDev(file)
    }
}

rental! {
    mod devices {
        use drm::control::framebuffer;
        use gbm::{Device as GbmDevice, Surface as GbmSurface};

        use ::backend::graphics::egl::EGLContext;
        use super::DrmDev;

        #[rental]
        pub(crate) struct Context {
            #[subrental(arity = 2)]
            devices: Box<Devices>,
            egl: EGLContext<'devices_1, GbmSurface<'devices_1, framebuffer::Info>>,
        }

        #[rental]
        pub(crate) struct Devices {
            drm: Box<DrmDev>,
            gbm: GbmDevice<'drm>,
        }
    }
}
use self::devices::{Context, Devices};


// odd naming, but makes sense for the user
pub struct DrmDevice<H: DrmHandler + 'static> {
    context: Rc<Context>,
    backends: Vec<Weak<RefCell<DrmBackendInternal>>>,
    handler: Option<H>,
    logger: ::slog::Logger,
}

impl<H: DrmHandler + 'static> DrmDevice<H> {
    pub unsafe fn new_from_fd<L>(fd: RawFd, logger: L) -> Result<Self, DrmError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new(
            DrmDev::new_from_fd(fd),
            GlAttributes {
                version: None,
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            logger,
        )
    }

    pub unsafe fn new_from_fd_with_gl_attr<L>(fd: RawFd, attributes: GlAttributes, logger: L)
        -> Result<Self, DrmError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new(DrmDev::new_from_fd(fd), attributes, logger)
    }

    pub fn new_from_file<L>(file: File, logger: L) -> Result<Self, DrmError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new(
            DrmDev::new_from_file(file),
            GlAttributes {
                version: None,
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            logger,
        )
    }

    pub fn new_from_file_with_gl_attr<L>(file: File, attributes: GlAttributes, logger: L)
                                         -> Result<Self, DrmError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new(DrmDev::new_from_file(file), attributes, logger)
    }

    fn new<L>(drm: DrmDev, attributes: GlAttributes, logger: L) -> Result<Self, DrmError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm", "drm" => "device"));

        /* GBM will load a dri driver, but even though they need symbols from
         * libglapi, in some version of Mesa they are not linked to it. Since
         * only the gl-renderer module links to it,  these symbols won't be globally available,
         * and loading the DRI driver fails.
         * Workaround this by dlopen()'ing libglapi with RTLD_GLOBAL.
         */
        unsafe {
            nix::libc::dlopen(
                "libglapi.so.0".as_ptr() as *const _,
                nix::libc::RTLD_LAZY | nix::libc::RTLD_GLOBAL,
            );
        }

        Ok(DrmDevice {
            context: Rc::new(Context::try_new(
                Box::new(Devices::try_new(Box::new(drm), |drm| {
                    GbmDevice::new_from_drm::<DrmDevice<H>>(drm).map_err(DrmError::from)
                })?),
                |devices| {
                    EGLContext::new_from_gbm(
                        devices.gbm,
                        attributes,
                        PixelFormatRequirements {
                            hardware_accelerated: Some(true),
                            color_bits: Some(24),
                            alpha_bits: Some(8),
                            ..Default::default()
                        },
                        log.clone(),
                    ).map_err(DrmError::from)
                },
            )?),
            backends: Vec::new(),
            handler: None,
            logger: log,
        })
    }

    pub fn create_backend<I>(&mut self, crtc: crtc::Handle, mode: Mode, connectors: I)
                             -> Result<DrmBackend, DrmError>
    where
        I: Into<Vec<connector::Handle>>,
    {
        let logger = self.logger
            .new(o!("drm" => "backend", "crtc" => format!("{:?}", crtc)));
        let own_id = self.backends.len();

        let backend = Rc::new(RefCell::new(DrmBackendInternal::new(
            self.context.clone(),
            crtc,
            mode,
            connectors,
            own_id,
            logger,
        )?));

        self.backends.push(Rc::downgrade(&backend));

        Ok(DrmBackend::new(backend))
    }

    pub fn set_handler(&mut self, handler: H) -> Option<H> {
        let res = self.handler.take();
        self.handler = Some(handler);
        res
    }

    pub fn clear_handler(&mut self) -> Option<H> {
        self.handler.take()
    }
}

// for users convinience
impl<H: DrmHandler + 'static> AsRawFd for DrmDevice<H> {
    fn as_raw_fd(&self) -> RawFd {
        self.context.head().head().as_raw_fd()
    }
}
impl<H: DrmHandler + 'static> BasicDevice for DrmDevice<H> {}
impl<H: DrmHandler + 'static> ControlDevice for DrmDevice<H> {}

pub trait DrmHandler {
    fn ready(&mut self, evlh: &mut EventLoopHandle, id: Id, frame: u32, duration: Duration);
    fn error(&mut self, evlh: &mut EventLoopHandle, error: IoError);
}

impl<H: DrmHandler + 'static> FdEventSourceHandler for DrmDevice<H> {
    fn ready(&mut self, evlh: &mut EventLoopHandle, fd: RawFd, _mask: FdInterest) {
        use std::any::Any;

        struct DrmDeviceRef(RawFd);
        impl AsRawFd for DrmDeviceRef {
            fn as_raw_fd(&self) -> RawFd {
                self.0
            }
        }
        impl BasicDevice for DrmDeviceRef {}
        impl ControlDevice for DrmDeviceRef {}

        struct PageFlipHandler<'a, 'b, H: DrmHandler + 'static>(
            &'a mut DrmDevice<H>,
            &'b mut EventLoopHandle,
        );

        impl<'a, 'b, H: DrmHandler + 'static> crtc::PageFlipHandler<DrmDeviceRef> for PageFlipHandler<'a, 'b, H> {
            fn handle_event(&mut self, _device: &DrmDeviceRef, frame: u32, duration: Duration,
                            userdata: Box<Any>) {
                let id: Id = *userdata.downcast().unwrap();
                if let Some(backend) = self.0.backends[id.raw()].upgrade() {
                    backend.borrow().unlock_buffer();
                    if let Some(handler) = self.0.handler.as_mut() {
                        handler.ready(self.1, id, frame, duration);
                    }
                }
            }
        }

        crtc::handle_event(
            &DrmDeviceRef(fd),
            2,
            None::<&mut ()>,
            Some(&mut PageFlipHandler(
                self,
                evlh,
            )),
            None::<&mut ()>,
        ).unwrap();
    }

    fn error(&mut self, evlh: &mut EventLoopHandle, _fd: RawFd, error: IoError) {
        if let Some(handler) = self.handler.as_mut() {
            handler.error(evlh, error)
        }
    }
}
