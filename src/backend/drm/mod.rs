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
//! extern crate wayland_server;
//!
//! use drm::control::{Device as ControlDevice, ResourceInfo};
//! use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
//! use drm::control::encoder::{Info as EncoderInfo};
//! use std::fs::OpenOptions;
//! use smithay::backend::drm::{DrmDevice, DrmBackend};
//! use wayland_server::StateToken;
//!
//! # fn main() {
//!
//! let (_display, mut event_loop) = wayland_server::create_display();
//!
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
//! let backend: StateToken<DrmBackend> = device.create_backend(
//!     &mut event_loop,
//!     crtc,
//!     mode,
//!     vec![connector_info.handle()]
//! ).unwrap();
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
//! # use std::fs::OpenOptions;
//! # use std::time::Duration;
//! use smithay::backend::drm::{DrmDevice, DrmBackend, DrmHandler, drm_device_bind};
//! use smithay::backend::graphics::egl::EGLGraphicsBackend;
//! use wayland_server::{EventLoopHandle, StateToken};
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
//! # let backend: StateToken<DrmBackend> = device.create_backend(
//! #     &mut event_loop,
//! #     crtc,
//! #     mode,
//! #     vec![connector_info.handle()]
//! # ).unwrap();
//!
//! struct MyDrmHandler;
//!
//! impl DrmHandler<DrmBackend> for MyDrmHandler {
//!     fn ready(&mut self,
//!              evlh: &mut EventLoopHandle,
//!              _device: &mut DrmDevice<DrmBackend>,
//!              backend: &StateToken<DrmBackend>,
//!              _frame: u32,
//!              _duration: Duration)
//!     {
//!         // render surfaces and swap again
//!         evlh.state().get(backend).swap_buffers().unwrap();
//!     }
//!     fn error(&mut self,
//!              _: &mut EventLoopHandle,
//!              device: &mut DrmDevice<DrmBackend>,
//!              error: IoError)
//!     {
//!         panic!("DrmDevice errored: {}", error);
//!     }
//! }
//!
//! // render something (like clear_color)
//! event_loop.state().get(&backend).swap_buffers().unwrap();
//!
//! let _source = drm_device_bind(&mut event_loop, device, MyDrmHandler).unwrap();
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
use std::borrow::Borrow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Error as IoError, Result as IoResult};
use std::marker::PhantomData;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::time::Duration;
use wayland_server::{EventLoopHandle, StateToken};
use wayland_server::sources::{FdEventSource, FdEventSourceImpl, READ};

mod backend;
pub mod error;

pub use self::backend::DrmBackend;
use self::error::*;

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
pub struct DrmDevice<B: Borrow<DrmBackend> + 'static> {
    context: Rc<Context>,
    backends: HashMap<crtc::Handle, StateToken<B>>,
    logger: ::slog::Logger,
}

impl<B: From<DrmBackend> + Borrow<DrmBackend> + 'static> DrmDevice<B> {
    /// Create a new `DrmDevice` from a raw file descriptor
    ///
    /// Returns an error of opening the device failed or context creation was not
    /// successful.
    ///
    /// # Safety
    /// The file descriptor might not be valid and needs to be owned by smithay,
    /// make sure not to share it. Otherwise undefined behavior might occur.
    pub unsafe fn new_from_fd<L>(fd: RawFd, logger: L) -> Result<Self>
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
    pub unsafe fn new_from_fd_with_gl_attr<L>(fd: RawFd, attributes: GlAttributes, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new(DrmDev::new_from_fd(fd), attributes, logger)
    }

    /// Create a new `DrmDevice` from a `File` of an open drm node
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new_from_file<L>(file: File, logger: L) -> Result<Self>
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
    pub fn new_from_file_with_gl_attr<L>(file: File, attributes: GlAttributes, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new(DrmDev::new_from_file(file), attributes, logger)
    }

    fn new<L>(drm: DrmDev, attributes: GlAttributes, logger: L) -> Result<Self>
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
                    GbmDevice::new_from_drm::<DrmDevice<B>>(drm).chain_err(|| ErrorKind::GbmInitFailed)
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
                    ).map_err(Error::from)
                },
            )?),
            backends: HashMap::new(),
            logger: log,
        })
    }

    /// Create a new backend on a given crtc with a given `Mode` for a given amount
    /// of `connectors` (mirroring).
    ///
    /// Errors if initialization fails or the mode is not available on all given
    /// connectors.
    pub fn create_backend<I>(&mut self, evlh: &mut EventLoopHandle, crtc: crtc::Handle, mode: Mode,
                             connectors: I)
                             -> Result<StateToken<B>>
    where
        I: Into<Vec<connector::Handle>>,
    {
        if self.backends.contains_key(&crtc) {
            bail!(ErrorKind::CrtcAlreadyInUse(crtc));
        }

        // check if the given connectors and crtc match
        let connectors = connectors.into();

        // check if we have an encoder for every connector and the mode mode
        for connector in connectors.iter() {
            let con_info = connector::Info::load_from_device(self.context.head().head(), *connector)
                .chain_err(|| {
                    ErrorKind::DrmDev(format!("{:?}", self.context.head().head()))
                })?;

            // check the mode
            if !con_info.modes().contains(&mode) {
                bail!(ErrorKind::ModeNotSuitable(mode));
            }

            // check for every connector which encoders it does support
            let encoders = con_info
                .encoders()
                .iter()
                .map(|encoder| {
                    encoder::Info::load_from_device(self.context.head().head(), *encoder).chain_err(|| {
                        ErrorKind::DrmDev(format!("{:?}", self.context.head().head()))
                    })
                })
                .collect::<Result<Vec<encoder::Info>>>()?;

            // and if any encoder supports the selected crtc
            if !encoders.iter().any(|encoder| encoder.supports_crtc(crtc)) {
                bail!(ErrorKind::NoSuitableEncoder(con_info, crtc))
            }
        }

        // configuration is valid, the kernel will figure out the rest

        let logger = self.logger.new(o!("crtc" => format!("{:?}", crtc)));
        let backend = DrmBackend::new(self.context.clone(), crtc, mode, connectors, logger)?;
        let token = evlh.state().insert(backend.into());
        self.backends.insert(crtc, token.clone());

        Ok(token)
    }
}

// for users convinience and FdEventSource registering
impl<B: Borrow<DrmBackend> + 'static> AsRawFd for DrmDevice<B> {
    fn as_raw_fd(&self) -> RawFd {
        self.context.head().head().as_raw_fd()
    }
}
impl<B: Borrow<DrmBackend> + 'static> BasicDevice for DrmDevice<B> {}
impl<B: Borrow<DrmBackend> + 'static> ControlDevice for DrmDevice<B> {}

/// Handler for drm node events
///
/// See module-level documentation for its use
pub trait DrmHandler<B: Borrow<DrmBackend> + 'static> {
    /// A `DrmBackend` has finished swapping buffers and new frame can now
    /// (and should be immediately) be rendered.
    ///
    /// The `id` argument is the `Id` of the `DrmBackend` that finished rendering,
    /// check using `DrmBackend::is`.
    fn ready(&mut self, evlh: &mut EventLoopHandle, device: &mut DrmDevice<B>, backend: &StateToken<B>,
             frame: u32, duration: Duration);
    /// The `DrmDevice` has thrown an error.
    ///
    /// The related backends are most likely *not* usable anymore and
    /// the whole stack has to be recreated.
    fn error(&mut self, evlh: &mut EventLoopHandle, device: &mut DrmDevice<B>, error: IoError);
}

/// Bind a `DrmDevice` to an EventLoop,
///
/// This will cause it to recieve events and feed them into an `DrmHandler`
pub fn drm_device_bind<B, H>(evlh: &mut EventLoopHandle, device: DrmDevice<B>, handler: H)
                             -> IoResult<FdEventSource<(DrmDevice<B>, H)>>
where
    B: Borrow<DrmBackend> + 'static,
    H: DrmHandler<B> + 'static,
{
    evlh.add_fd_event_source(
        device.as_raw_fd(),
        fd_event_source_implementation(),
        (device, handler),
        READ,
    )
}

fn fd_event_source_implementation<B, H>() -> FdEventSourceImpl<(DrmDevice<B>, H)>
where
    B: Borrow<DrmBackend> + 'static,
    H: DrmHandler<B> + 'static,
{
    FdEventSourceImpl {
        ready: |evlh, id, _, _| {
            use std::any::Any;

            let &mut (ref mut dev, ref mut handler) = id;

            struct PageFlipHandler<
                'a,
                'b,
                B: Borrow<DrmBackend> + 'static,
                H: DrmHandler<B> + 'static,
            > {
                handler: &'a mut H,
                evlh: &'b mut EventLoopHandle,
                _marker: PhantomData<B>,
            };

            impl<'a, 'b, B, H> crtc::PageFlipHandler<DrmDevice<B>> for PageFlipHandler<'a, 'b, B, H>
            where
                B: Borrow<DrmBackend> + 'static,
                H: DrmHandler<B> + 'static,
            {
                fn handle_event(&mut self, device: &mut DrmDevice<B>, frame: u32, duration: Duration,
                                userdata: Box<Any>) {
                    let crtc_id: crtc::Handle = *userdata.downcast().unwrap();
                    let token = device.backends.get(&crtc_id).cloned();
                    if let Some(token) = token {
                        // we can now unlock the buffer
                        self.evlh.state().get(&token).borrow().unlock_buffer();
                        trace!(device.logger, "Handling event for backend {:?}", crtc_id);
                        // and then call the user to render the next frame
                        self.handler
                            .ready(self.evlh, device, &token, frame, duration);
                    }
                }
            }

            crtc::handle_event(
                dev,
                2,
                None::<&mut ()>,
                Some(&mut PageFlipHandler {
                    handler,
                    evlh,
                    _marker: PhantomData,
                }),
                None::<&mut ()>,
            ).unwrap();
        },
        error: |evlh, id, _, error| {
            warn!(id.0.logger, "DrmDevice errored: {}", error);
            id.1.error(evlh, &mut id.0, error);
        },
    }
}
