//! Drm/Kms types and backend implementations
//!
//! This module provide a `DrmDevice` which acts as a reprensentation for any drm
//! device and can be used to create the second provided structure a `DrmBackend`.
//!
//! Initialization happens through the types provided by [`drm-rs`](https://docs.rs/drm/).
//!
//! Three entities are relevant for the initialization procedure.
//!
//! "Crtc"s represent scanout engines of the device pointer to one framebuffer. There responsibility
//! is to read the data of the framebuffer and export it into an "Encoder". The number of crtc's
//! represent the number of independant output devices the hardware may handle.
//!
//! An "Encoder" encodes the data of connected crtcs into a video signal for a fixed set
//! of connectors. E.g. you might have an analog encoder based on a DAG for VGA ports, but another
//! one for digital ones. Also not every encoder might be connected to every crtc.
//!
//! The last entity the "Connector" represents a port on your computer, possibly with a connected
//! monitor, TV, capture card, etc.
//!
//! The `DrmBackend` created from a `DrmDevice` represents a crtc of the device you can render to
//! and that feeds a given set of connectors, that can be manipulated at runtime.
//!
//! From these circumstances it becomes clear, that one crtc might only send it's data to a connector,
//! that is attached to any encoder that is attached to the crtc itself. It is the responsibility of the
//! user to ensure that a given set of a crtc with it's connectors is valid or an error will be thrown.
//!
//! For more details refer to the [`drm-rs` documentation](https://docs.rs/drm).
//!
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize the `DrmDevice` you need either a `RawFd` or a `File` of
//! your drm node. The `File` is recommended as it represents the save api.
//!
//! Once you got your `DrmDevice` you can then use it to create `DrmBackend`s.
//! You will need to use the `drm` crate to provide the required types to create
//! a backend.
//!
//! ```rust,no_run
//! extern crate drm;
//! extern crate smithay;
//! # extern crate wayland_server;
//!
//! use drm::control::{Device as ControlDevice, ResourceInfo};
//! use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
//! use drm::control::encoder::{Info as EncoderInfo};
//! # use std::io::Error as IoError;
//! use std::fs::OpenOptions;
//! # use std::time::Duration;
//! use smithay::backend::drm::DrmDevice;
//! # use smithay::backend::drm::{DrmHandler, Id};
//! # use wayland_server::EventLoopHandle;
//!
//! # fn main() {
//! // Open the drm device
//! let mut options = OpenOptions::new();
//! options.read(true);
//! options.write(true);
//! let mut device = DrmDevice::new_from_file(
//!     options.open("/dev/dri/card0").unwrap(), // try to detect it properly
//!     None /*put a logger here*/
//! ).unwrap();
//!
//! // Get a set of all modesetting resource handles
//! let res_handles = device.resource_handles().unwrap();
//!
//! // Use first connected connector for this example
//! let connector_info = res_handles.connectors().iter()
//!     .map(|conn| ConnectorInfo::load_from_device(&device, *conn).unwrap())
//!     .find(|conn| conn.connection_state() == ConnectorState::Connected)
//!     .unwrap();
//!
//! // Use the first encoder
//! let encoder_info = EncoderInfo::load_from_device(&device, connector_info.encoders()[0]).unwrap();
//!
//! // use the connected crtc if any
//! let crtc = encoder_info.current_crtc()
//!     // or use the first one that is compatible with the encoder
//!     .unwrap_or_else(||
//!         *res_handles.crtcs()
//!         .iter()
//!         .find(|crtc| encoder_info.supports_crtc(**crtc))
//!         .unwrap());
//!
//! // Use first mode (usually the highest resolution)
//! let mode = connector_info.modes()[0];
//!
//! // Create the backend
//! let backend = device.create_backend(
//!         crtc,
//!         mode,
//!         vec![connector_info.handle()]
//!     ).unwrap();
//! # struct MyDrmHandler;
//! #
//! # impl DrmHandler for MyDrmHandler {
//! #    fn ready(&mut self, _: &mut EventLoopHandle, id: Id, _frame: u32, _duration: Duration) {}
//! #    fn error(&mut self, _: &mut EventLoopHandle, error: IoError) {}
//! # }
//! #
//! # device.set_handler(MyDrmHandler);
//! # }
//! ```
//!
//! ### Page Flips / Tear-free video
//! Calling the usual `EglGraphicsBackend::swap_buffers` function on a
//! `DrmBackend` works the same to finish the rendering, but will return
//! `SwapBuffersError::AlreadySwapped` for any new calls until the page flip of the
//! crtc has happened.
//!
//! You can monitor the page flips by registering the `DrmDevice` as and
//! `FdEventSourceHandler` and setting a `DrmHandler` on it. You will be notified
//! whenever a page flip has happend, so you can render the next frame immediately
//! and get a tear-free reprensentation on the display.
//!
//! You need to render at least once to successfully trigger the first event.
//!
//! ```rust,no_run
//! # extern crate drm;
//! # extern crate smithay;
//! # extern crate wayland_server;
//! #
//! # use drm::control::{Device as ControlDevice, ResourceInfo};
//! # use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
//! use std::io::Error as IoError;
//! use std::os::unix::io::AsRawFd;
//! # use std::fs::OpenOptions;
//! # use std::time::Duration;
//! use smithay::backend::drm::{DrmDevice, DrmBackend, DrmHandler, Id};
//! use smithay::backend::graphics::egl::EGLGraphicsBackend;
//! use wayland_server::sources::READ;
//! # use wayland_server::EventLoopHandle;
//! #
//! # fn main() {
//! #
//! # let (_display, mut event_loop) = wayland_server::create_display();
//! #
//! # let mut options = OpenOptions::new();
//! # options.read(true);
//! # options.write(true);
//! # let mut device = DrmDevice::new_from_file(
//! #     options.open("/dev/dri/card0").unwrap(), // try to detect it properly
//! #     None /*put a logger here*/
//! # ).unwrap();
//! #
//! # let res_handles = device.resource_handles().unwrap();
//! # let connector_info = res_handles.connectors().iter()
//! #     .map(|conn| ConnectorInfo::load_from_device(&device, *conn).unwrap())
//! #     .find(|conn| conn.connection_state() == ConnectorState::Connected)
//! #     .unwrap();
//! # let crtc = res_handles.crtcs()[0];
//! # let mode = connector_info.modes()[0];
//! # let backend = device.create_backend(
//! #         crtc,
//! #         mode,
//! #         vec![connector_info.handle()]
//! #     ).unwrap();
//!
//! struct MyDrmHandler(DrmBackend);
//!
//! impl DrmHandler for MyDrmHandler {
//!     fn ready(&mut self, _: &mut EventLoopHandle, id: Id, _frame: u32, _duration: Duration) {
//!         if self.0.is(id) { // check id in case you got multiple backends
//!             // ... render surfaces ...
//!             self.0.swap_buffers().unwrap(); // trigger the swap
//!         }
//!     }
//!     fn error(&mut self, _: &mut EventLoopHandle, error: IoError) {
//!         panic!("DrmDevice errored: {}", error);
//!     }
//! }
//!
//! // render something (like clear_color)
//! backend.swap_buffers().unwrap();
//!
//! device.set_handler(MyDrmHandler(backend));
//! let fd = device.as_raw_fd();
//! let drm_device_id = event_loop.add_handler(device);
//! let _drm_event_source = event_loop.add_fd_event_source::<DrmDevice<MyDrmHandler>>(fd, drm_device_id, READ);
//!
//! event_loop.run().unwrap();
//! # }
//! ```

use backend::graphics::egl::{EGLContext, GlAttributes, PixelFormatRequirements};
use drm::Device as BasicDevice;
use drm::control::{connector, crtc, encoder, Mode, ResourceInfo};
use drm::control::Device as ControlDevice;

use gbm::Device as GbmDevice;

use nix;

use std::cell::RefCell;
use std::fs::File;
use std::io::Error as IoError;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::time::Duration;

use wayland_server::EventLoopHandle;
use wayland_server::sources::{FdEventSourceHandler, FdInterest};

mod backend;
mod error;

pub use self::backend::{DrmBackend, Id};
use self::backend::DrmBackendInternal;
pub use self::error::{CrtcError, Error as DrmError, ModeError};

/// Internal struct as required by the drm crate
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

/// Representation of an open drm device node to create rendering backends
pub struct DrmDevice<H: DrmHandler + 'static> {
    context: Rc<Context>,
    backends: Vec<Weak<RefCell<DrmBackendInternal>>>,
    handler: Option<H>,
    logger: ::slog::Logger,
}

impl<H: DrmHandler + 'static> DrmDevice<H> {
    /// Create a new `DrmDevice` from a raw file descriptor
    ///
    /// Returns an error of opening the device failed or context creation was not
    /// successful.
    ///
    /// # Safety
    /// The file descriptor might not be valid and needs to be owned by smithay,
    /// make sure not to share it. Otherwise undefined behavior might occur.
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

    /// Create a new `DrmDevice` from a raw file descriptor and given `GlAttributes`
    ///
    /// Returns an error of opening the device failed or context creation was not
    /// successful.
    ///
    /// # Safety
    /// The file descriptor might not be valid and needs to be owned by smithay,
    /// make sure not to share it. Otherwise undefined behavior might occur.
    pub unsafe fn new_from_fd_with_gl_attr<L>(fd: RawFd, attributes: GlAttributes, logger: L)
                                              -> Result<Self, DrmError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new(DrmDev::new_from_fd(fd), attributes, logger)
    }

    /// Create a new `DrmDevice` from a `File` of an open drm node
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
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

    /// Create a new `DrmDevice` from a `File` of an open drm node and given `GlAttributes`
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
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
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm"));

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

        info!(log, "DrmDevice initializing");

        // Open the gbm device from the drm device and create a context based on that
        Ok(DrmDevice {
            context: Rc::new(Context::try_new(
                Box::new(Devices::try_new(Box::new(drm), |drm| {
                    debug!(log, "Creating gbm device");
                    GbmDevice::new_from_drm::<DrmDevice<H>>(drm).map_err(DrmError::from)
                })?),
                |devices| {
                    debug!(log, "Creating egl context from gbm device");
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

    /// Create a new backend on a given crtc with a given `Mode` for a given amount
    /// of `connectors` (mirroring).
    ///
    /// Errors if initialization fails or the mode is not available on all given
    /// connectors.
    pub fn create_backend<I>(&mut self, crtc: crtc::Handle, mode: Mode, connectors: I)
                             -> Result<DrmBackend, DrmError>
    where
        I: Into<Vec<connector::Handle>>,
    {
        use std::collections::hash_set::HashSet;

        for backend in self.backends.iter() {
            if let Some(backend) = backend.upgrade() {
                if backend.borrow().is_crtc(crtc) {
                    return Err(DrmError::Crtc(CrtcError::AlreadyInUse));
                }
            }
        }

        // check if the given connectors and crtc match
        let connectors = connectors.into();
        // get all encoders supported by this device
        let mut set = self.context
            .head()
            .head()
            .resource_handles()?
            .encoders()
            .iter()
            .cloned()
            .collect::<HashSet<encoder::Handle>>();
        for connector in connectors.iter() {
            let info = connector::Info::load_from_device(self.context.head().head(), *connector)?;
            // then check for every connector which encoders it does support
            let conn_set = info.encoders()
                .iter()
                .cloned()
                .collect::<HashSet<encoder::Handle>>();
            // and update the list of supported encoders for this combination
            set = set.intersection(&conn_set)
                .cloned()
                .collect::<HashSet<encoder::Handle>>();
        }

        // check if there is any encoder left that can be connected to the crtc
        let encoders: Vec<encoder::Info> = set.iter()
            .map(|handle| {
                encoder::Info::load_from_device(self.context.head().head(), *handle).map_err(DrmError::from)
            })
            .collect::<Result<Vec<encoder::Info>, DrmError>>()?;
        if !encoders.iter().any(|enc| enc.supports_crtc(crtc)) {
            return Err(DrmError::Crtc(CrtcError::NoSuitableEncoder));
        }

        // configuration is valid, the kernel will figure out the rest

        let own_id = self.backends.len();
        let logger = self.logger
            .new(o!("id" => format!("{}", own_id), "crtc" => format!("{:?}", crtc)));

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

    /// Set a handler for handling finished rendering
    pub fn set_handler(&mut self, handler: H) -> Option<H> {
        let res = self.handler.take();
        self.handler = Some(handler);
        res
    }

    /// Clear the currently set handler
    pub fn clear_handler(&mut self) -> Option<H> {
        self.handler.take()
    }
}

// for users convinience and FdEventSource registering
impl<H: DrmHandler + 'static> AsRawFd for DrmDevice<H> {
    fn as_raw_fd(&self) -> RawFd {
        self.context.head().head().as_raw_fd()
    }
}
impl<H: DrmHandler + 'static> BasicDevice for DrmDevice<H> {}
impl<H: DrmHandler + 'static> ControlDevice for DrmDevice<H> {}

/// Handler for drm node events
///
/// See module-level documentation for its use
pub trait DrmHandler {
    /// A `DrmBackend` has finished swapping buffers and new frame can now
    /// (and should be immediately) be rendered.
    ///
    /// The `id` argument is the `Id` of the `DrmBackend` that finished rendering,
    /// check using `DrmBackend::is`.
    fn ready(&mut self, evlh: &mut EventLoopHandle, id: Id, frame: u32, duration: Duration);
    /// The `DrmDevice` has thrown an error.
    ///
    /// The related backends are most likely *not* usable anymore and
    /// the whole stack has to be recreated.
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

        struct PageFlipHandler<'a, 'b, H: DrmHandler + 'static>(&'a mut DrmDevice<H>, &'b mut EventLoopHandle);

        impl<'a, 'b, H: DrmHandler + 'static> crtc::PageFlipHandler<DrmDeviceRef> for PageFlipHandler<'a, 'b, H> {
            fn handle_event(&mut self, _device: &DrmDeviceRef, frame: u32, duration: Duration,
                            userdata: Box<Any>) {
                let id: Id = *userdata.downcast().unwrap();
                if let Some(backend) = self.0.backends[id.raw()].upgrade() {
                    // we can now unlock the buffer
                    trace!(self.0.logger, "Handling event for backend {:?}", id.raw());
                    backend.borrow().unlock_buffer();
                    if let Some(handler) = self.0.handler.as_mut() {
                        // and then call the user to render the next frame
                        handler.ready(self.1, id, frame, duration);
                    }
                }
            }
        }

        crtc::handle_event(
            &DrmDeviceRef(fd),
            2,
            None::<&mut ()>,
            Some(&mut PageFlipHandler(self, evlh)),
            None::<&mut ()>,
        ).unwrap();
    }

    fn error(&mut self, evlh: &mut EventLoopHandle, _fd: RawFd, error: IoError) {
        if let Some(handler) = self.handler.as_mut() {
            warn!(self.logger, "DrmDevice errored: {}", error);
            handler.error(evlh, error)
        }
    }
}
