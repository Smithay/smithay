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
//!         *res_handles.filter_crtcs(encoder_info.possible_crtcs())
//!           .iter()
//!           .next()
//!           .unwrap());
//!
//! // Use first mode (usually the highest resolution)
//! let mode = connector_info.modes()[0];
//!
//! // Create the backend
//! let backend: StateToken<DrmBackend> = device.create_backend(
//!     event_loop.state(),
//!     crtc,
//!     mode,
//!     vec![connector_info.handle()]
//! ).unwrap().clone();
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
//! use drm::control::crtc::{Handle as CrtcHandle};
//! use drm::result::Error as DrmError;
//! # use std::fs::OpenOptions;
//! # use std::time::Duration;
//! use smithay::backend::drm::{DrmDevice, DrmBackend, DrmHandler, drm_device_bind};
//! use smithay::backend::graphics::egl::EGLGraphicsBackend;
//! use wayland_server::{StateToken, StateProxy};
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
//! #     event_loop.state(),
//! #     crtc,
//! #     mode,
//! #     vec![connector_info.handle()]
//! # ).unwrap().clone();
//!
//! struct MyDrmHandler;
//!
//! impl DrmHandler<DrmBackend> for MyDrmHandler {
//!     fn ready<'a, S: Into<StateProxy<'a>>>(
//!         &mut self,
//!         state: S,
//!         _device: &mut DrmDevice<DrmBackend>,
//!         backend: &StateToken<DrmBackend>,
//!         _crtc: CrtcHandle,
//!         _frame: u32,
//!         _duration: Duration)
//!     {
//!         // render surfaces and swap again
//!         state.into().get(backend).swap_buffers().unwrap();
//!     }
//!     fn error<'a, S: Into<StateProxy<'a>>>(
//!         &mut self,
//!         _state: S,
//!         device: &mut DrmDevice<DrmBackend>,
//!         error: DrmError)
//!     {
//!         panic!("DrmDevice errored: {}", error);
//!     }
//! }
//!
//! // render something (like clear_color)
//! event_loop.state().get(&backend).swap_buffers().unwrap();
//!
//! let device_token = event_loop.state().insert(device);
//! let _source = drm_device_bind(&mut event_loop, device_token, MyDrmHandler).unwrap();
//!
//! event_loop.run().unwrap();
//! # }
//! ```

#[cfg(feature = "backend_session")]
use backend::graphics::egl::EGLGraphicsBackend;
use backend::graphics::egl::context::{EGLContext, GlAttributes, PixelFormatRequirements};
use backend::graphics::egl::native::Gbm;
#[cfg(feature = "backend_session")]
use backend::session::SessionObserver;
use drm::Device as BasicDevice;
use drm::control::{connector, crtc, encoder, Mode, ResourceInfo};
use drm::control::Device as ControlDevice;
use drm::result::Error as DrmError;
use drm::control::framebuffer;
use gbm::Device as GbmDevice;
use nix;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Result as IoResult;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::{Once, ONCE_INIT};
use std::path::PathBuf;
use std::time::Duration;
use wayland_server::{EventLoopHandle, StateProxy, StateToken};
use wayland_server::sources::{FdEventSource, FdEventSourceImpl, FdInterest};

mod backend;
pub mod error;

pub use self::backend::DrmBackend;
use self::error::*;

static LOAD: Once = ONCE_INIT;

/// Representation of an open drm device node to create rendering backends
pub struct DrmDevice<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> {
    context: Rc<EGLContext<Gbm<framebuffer::Info>, GbmDevice<A>>>,
    backends: HashMap<crtc::Handle, StateToken<B>>,
    old_state: HashMap<crtc::Handle, (crtc::Info, Vec<connector::Handle>)>,
    active: bool,
    logger: ::slog::Logger,
}

impl<A: ControlDevice + 'static, B: From<DrmBackend<A>> + Borrow<DrmBackend<A>> + 'static> DrmDevice<A, B> {
    /// Create a new `DrmDevice` from an open drm node
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new<L>(dev: A, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        DrmDevice::new_with_gl_attr(
            dev,
            GlAttributes {
                version: None,
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            logger,
        )
    }

    /// Create a new `DrmDevice` from an open drm node and given `GlAttributes`
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new_with_gl_attr<L>(dev: A, attributes: GlAttributes, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm"));

        /* GBM will load a dri driver, but even though they need symbols from
         * libglapi, in some version of Mesa they are not linked to it. Since
         * only the gl-renderer module links to it, these symbols won't be globally available,
         * and loading the DRI driver fails.
         * Workaround this by dlopen()'ing libglapi with RTLD_GLOBAL.
         */
        LOAD.call_once(|| unsafe {
            nix::libc::dlopen(
                "libglapi.so.0".as_ptr() as *const _,
                nix::libc::RTLD_LAZY | nix::libc::RTLD_GLOBAL,
            );
        });

        let mut drm = DrmDevice {
            // Open the gbm device from the drm device and create a context based on that
            context: Rc::new(EGLContext::new(
                {
                    debug!(log, "Creating gbm device");
                    let gbm = GbmDevice::new(dev).chain_err(|| ErrorKind::GbmInitFailed)?;
                    debug!(log, "Creating egl context from gbm device");
                    gbm
                },
                attributes,
                PixelFormatRequirements {
                    hardware_accelerated: Some(true),
                    color_bits: Some(24),
                    alpha_bits: Some(8),
                    ..Default::default()
                },
                log.clone(),
            ).map_err(Error::from)?),
            backends: HashMap::new(),
            old_state: HashMap::new(),
            active: true,
            logger: log.clone(),
        };

        info!(log, "DrmDevice initializing");

        // we want to mode-set, so we better be the master
        drm.set_master().chain_err(|| ErrorKind::DrmMasterFailed)?;

        let res_handles = drm.resource_handles()
            .chain_err(|| ErrorKind::DrmDev(format!("Error loading drm resources on {:?}", drm.dev_path())))?;
        for &con in res_handles.connectors() {
            let con_info = connector::Info::load_from_device(&drm, con)
                .chain_err(|| ErrorKind::DrmDev(format!("Error loading connector info on {:?}", drm.dev_path())))?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = encoder::Info::load_from_device(&drm, enc)
                    .chain_err(|| ErrorKind::DrmDev(format!("Error loading encoder info on {:?}", drm.dev_path())))?;
                if let Some(crtc) = enc_info.current_crtc() {
                    let info = crtc::Info::load_from_device(&drm, crtc)
                        .chain_err(|| ErrorKind::DrmDev(format!("Error loading crtc info on {:?}", drm.dev_path())))?;
                    drm.old_state
                        .entry(crtc)
                        .or_insert((info, Vec::new()))
                        .1
                        .push(con);
                }
            }
        }

        Ok(drm)
    }

    /// Create a new backend on a given crtc with a given `Mode` for a given amount
    /// of `connectors` (mirroring).
    ///
    /// Errors if initialization fails or the mode is not available on all given
    /// connectors.
    pub fn create_backend<'a, I, S>(
        &mut self, state: S, crtc: crtc::Handle, mode: Mode, connectors: I
    ) -> Result<&StateToken<B>>
    where
        I: Into<Vec<connector::Handle>>,
        S: Into<StateProxy<'a>>,
    {
        if self.backends.contains_key(&crtc) {
            bail!(ErrorKind::CrtcAlreadyInUse(crtc));
        }

        if !self.active {
            bail!(ErrorKind::DeviceInactive);
        }

        // check if the given connectors and crtc match
        let connectors = connectors.into();

        // check if we have an encoder for every connector and the mode mode
        for connector in &connectors {
            let con_info = connector::Info::load_from_device(self, *connector)
                .chain_err(|| {
                    ErrorKind::DrmDev(format!("Error loading connector info on {:?}", self.dev_path()))
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
                    encoder::Info::load_from_device(self, *encoder).chain_err(|| {
                        ErrorKind::DrmDev(format!("Error loading encoder info on {:?}", self.dev_path()))
                    })
                })
                .collect::<Result<Vec<encoder::Info>>>()?;

            // and if any encoder supports the selected crtc
            let resource_handles = self.resource_handles().chain_err(|| {
                ErrorKind::DrmDev(format!("Error loading drm resources on {:?}", self.dev_path()))
            })?;
            if !encoders
                .iter()
                .map(|encoder| encoder.possible_crtcs())
                .any(|crtc_list| resource_handles.filter_crtcs(crtc_list).contains(&crtc))
            {
                bail!(ErrorKind::NoSuitableEncoder(con_info, crtc))
            }
        }

        // configuration is valid, the kernel will figure out the rest

        let logger = self.logger.new(o!("crtc" => format!("{:?}", crtc)));
        let backend = DrmBackend::new(self.context.clone(), crtc, mode, connectors, logger)?;
        self.backends
            .insert(crtc, state.into().insert(backend.into()));

        Ok(self.backends.get(&crtc).unwrap())
    }

    /// Get the current backend for a given crtc if any
    pub fn backend_for_crtc(&self, crtc: &crtc::Handle) -> Option<&StateToken<B>> {
        self.backends.get(crtc)
    }

    /// Get all belonging backends
    pub fn current_backends(&self) -> Vec<&StateToken<B>> {
        self.backends.values().collect()
    }

    /// Destroy the backend using a given crtc if any
    ///
    /// ## Panics
    /// Panics if the backend is already borrowed from the state
    pub fn destroy_backend<'a, S>(&mut self, state: S, crtc: &crtc::Handle)
    where
        S: Into<StateProxy<'a>>,
    {
        if let Some(token) = self.backends.remove(crtc) {
            state.into().remove(token);
        }
    }
}

pub trait DevPath {
    fn dev_path(&self) -> Option<PathBuf>;
}

impl<A: AsRawFd> DevPath for A {
    fn dev_path(&self) -> Option<PathBuf> {
        use std::fs;

        fs::read_link(format!("/proc/self/fd/{:?}", self.as_raw_fd())).ok()
    }
}

// for users convinience and FdEventSource registering
impl<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> AsRawFd for DrmDevice<A, B> {
    fn as_raw_fd(&self) -> RawFd {
        self.context.as_raw_fd()
    }
}

impl<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> BasicDevice for DrmDevice<A, B> {}
impl<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> ControlDevice for DrmDevice<A, B> {}

impl<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> Drop for DrmDevice<A, B> {
    fn drop(&mut self) {
        if Rc::strong_count(&self.context) > 1 {
            panic!("Pending DrmBackends. Please free all backends before the DrmDevice gets destroyed");
        }
        for (handle, (info, connectors)) in self.old_state.drain() {
            if let Err(err) = crtc::set(
                &*self.context,
                handle,
                info.fb(),
                &connectors,
                info.position(),
                info.mode(),
            ) {
                error!(
                    self.logger,
                    "Failed to reset crtc ({:?}). Error: {}", handle, err
                );
            }
        }
        if let Err(err) = self.drop_master() {
            error!(
                self.logger,
                "Failed to drop drm master state. Error: {}", err
            );
        }
    }
}

impl<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> Hash for DrmDevice<A, B> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_raw_fd().hash(state)
    }
}

/// Handler for drm node events
///
/// See module-level documentation for its use
pub trait DrmHandler<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> {
    /// A `DrmBackend` has finished swapping buffers and new frame can now
    /// (and should be immediately) be rendered.
    ///
    /// The `id` argument is the `Id` of the `DrmBackend` that finished rendering,
    /// check using `DrmBackend::is`.
    ///
    /// ## Panics
    /// The device is already borrowed from the given `state`. Borrowing it again will panic
    /// and is not necessary as it is already provided via the `device` parameter.
    fn ready<'a, S: Into<StateProxy<'a>>>(
        &mut self, state: S, device: &mut DrmDevice<A, B>, backend: &StateToken<B>, crtc: crtc::Handle,
        frame: u32, duration: Duration,
    );
    /// The `DrmDevice` has thrown an error.
    ///
    /// The related backends are most likely *not* usable anymore and
    /// the whole stack has to be recreated..
    ///
    /// ## Panics
    /// The device is already borrowed from the given `state`. Borrowing it again will panic
    /// and is not necessary as it is already provided via the `device` parameter.
    fn error<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, device: &mut DrmDevice<A, B>, error: DrmError);
}

/// Bind a `DrmDevice` to an `EventLoop`,
///
/// This will cause it to recieve events and feed them into an `DrmHandler`
pub fn drm_device_bind<A, B, H>(
    evlh: &mut EventLoopHandle, device: StateToken<DrmDevice<A, B>>, handler: H
) -> IoResult<FdEventSource<(StateToken<DrmDevice<A, B>>, H)>>
where
    A: ControlDevice + 'static,
    B: From<DrmBackend<A>> + Borrow<DrmBackend<A>> + 'static,
    H: DrmHandler<A, B> + 'static,
{
    let fd = evlh.state().get(&device).as_raw_fd();
    evlh.add_fd_event_source(
        fd,
        fd_event_source_implementation(),
        (device, handler),
        FdInterest::READ,
    )
}

fn fd_event_source_implementation<A, B, H>() -> FdEventSourceImpl<(StateToken<DrmDevice<A, B>>, H)>
where
    A: ControlDevice + 'static,
    B: From<DrmBackend<A>> + Borrow<DrmBackend<A>> + 'static,
    H: DrmHandler<A, B> + 'static,
{
    FdEventSourceImpl {
        ready: |evlh, &mut (ref mut dev_token, ref mut handler), _, _| {
            let (events, logger) = {
                let dev = evlh.state().get(dev_token);
                let events = crtc::receive_events(dev);
                let logger = dev.logger.clone();
                (events, logger)
            };

            match events {
                Ok(events) => for event in events {
                    if let crtc::Event::PageFlip(event) = event {
                        evlh.state().with_value(dev_token, |state, mut dev| {
                            if dev.active {
                                if let Some(backend_token) = dev.backend_for_crtc(&event.crtc).cloned() {
                                    // we can now unlock the buffer
                                    state.get(&backend_token).borrow().unlock_buffer();
                                    trace!(logger, "Handling event for backend {:?}", event.crtc);
                                    // and then call the user to render the next frame
                                    handler.ready(
                                        state,
                                        &mut dev,
                                        &backend_token,
                                        event.crtc,
                                        event.frame,
                                        event.duration,
                                    );
                                }
                            }
                        });
                    }
                },
                Err(err) => evlh.state().with_value(dev_token, |state, mut dev| {
                    handler.error(state, &mut dev, err)
                }),
            };
        },
        error: |evlh, &mut (ref mut dev_token, ref mut handler), _, error| {
            evlh.state().with_value(dev_token, |state, mut dev| {
                warn!(dev.logger, "DrmDevice errored: {}", error);
                handler.error(state, &mut dev, error.into());
            })
        },
    }
}

#[cfg(feature = "backend_session")]
impl<A: ControlDevice + 'static, B: Borrow<DrmBackend<A>> + 'static> SessionObserver for StateToken<DrmDevice<A, B>> {
    fn pause<'a>(&mut self, state: &mut StateProxy<'a>) {
        let device: &mut DrmDevice<A, B> = state.get_mut(self);
        device.active = false;
        if let Err(err) = device.drop_master() {
            error!(
                device.logger,
                "Failed to drop drm master state. Error: {}", err
            );
        }
    }

    fn activate<'a>(&mut self, state: &mut StateProxy<'a>) {
        state.with_value(self, |state, device| {
            device.active = true;
            if let Err(err) = device.set_master() {
                crit!(
                    device.logger,
                    "Failed to acquire drm master again. Error: {}",
                    err
                );
            }
            for token in device.backends.values() {
                let backend = state.get(token);
                if let Err(err) = backend.borrow().swap_buffers() {
                    // TODO handle this better?
                    error!(
                        device.logger,
                        "Failed to activate crtc ({:?}) again. Error: {}",
                        backend.borrow().crtc(),
                        err
                    );
                }
            }
        })
    }
}
