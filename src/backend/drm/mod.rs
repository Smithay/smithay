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
//! use drm::Device as BasicDevice;
//! use drm::control::{Device as ControlDevice, ResourceInfo};
//! use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
//! use drm::control::encoder::{Info as EncoderInfo};
//! use std::fs::{File, OpenOptions};
//! use std::os::unix::io::RawFd;
//! use std::os::unix::io::AsRawFd;
//! use smithay::backend::drm::{DrmDevice, DrmBackend};
//!
//! #[derive(Debug)]
//! pub struct Card(File);
//!
//! impl AsRawFd for Card {
//!     fn as_raw_fd(&self) -> RawFd {
//!         self.0.as_raw_fd()
//!     }
//! }
//!
//! impl BasicDevice for Card {}
//! impl ControlDevice for Card {}
//!
//! # fn main() {
//! // Open the drm device
//! let mut options = OpenOptions::new();
//! options.read(true);
//! options.write(true);
//! let mut device = DrmDevice::new(
//!     Card(options.open("/dev/dri/card0").unwrap()), // try to detect it properly
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
//! let backend = device.create_backend(
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
//! # use drm::Device as BasicDevice;
//! # use drm::control::{Device as ControlDevice, ResourceInfo};
//! # use drm::control::connector::{Info as ConnectorInfo, State as ConnectorState};
//! use drm::control::crtc::{Handle as CrtcHandle};
//! use drm::result::Error as DrmError;
//! # use std::fs::{File, OpenOptions};
//! # use std::os::unix::io::RawFd;
//! # use std::os::unix::io::AsRawFd;
//! # use std::time::Duration;
//! use smithay::backend::drm::{DrmDevice, DrmBackend, DrmHandler, drm_device_bind};
//! use smithay::backend::graphics::egl::EGLGraphicsBackend;
//! #
//! # #[derive(Debug)]
//! # pub struct Card(File);
//! # impl AsRawFd for Card {
//! #     fn as_raw_fd(&self) -> RawFd {
//! #         self.0.as_raw_fd()
//! #     }
//! # }
//! # impl BasicDevice for Card {}
//! # impl ControlDevice for Card {}
//! #
//! # fn main() {
//! #
//! # let (_display, mut event_loop) = wayland_server::Display::new();
//! #
//! # let mut options = OpenOptions::new();
//! # options.read(true);
//! # options.write(true);
//! # let mut device = DrmDevice::new(
//! #     Card(options.open("/dev/dri/card0").unwrap()), // try to detect it properly
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
//! #     crtc,
//! #     mode,
//! #     vec![connector_info.handle()]
//! # ).unwrap();
//!
//! struct MyDrmHandler(DrmBackend<Card>);
//!
//! impl DrmHandler<Card> for MyDrmHandler {
//!     fn ready(
//!         &mut self,
//!         _device: &mut DrmDevice<Card>,
//!         _crtc: CrtcHandle,
//!         _frame: u32,
//!         _duration: Duration)
//!     {
//!         // render surfaces and swap again
//!         self.0.swap_buffers().unwrap();
//!     }
//!     fn error(
//!         &mut self,
//!         device: &mut DrmDevice<Card>,
//!         error: DrmError)
//!     {
//!         panic!("DrmDevice errored: {}", error);
//!     }
//! }
//!
//! // render something (like clear_color)
//! backend.swap_buffers().unwrap();
//!
//! let (_source, _device_rc) = drm_device_bind(
//!     &event_loop.token(),
//!     device,
//!     MyDrmHandler(backend)
//! ).map_err(|(err, _)| err).unwrap();
//!
//! event_loop.run().unwrap();
//! # }
//! ```

use backend::graphics::egl::context::{EGLContext, GlAttributes};
use backend::graphics::egl::error::Result as EGLResult;
use backend::graphics::egl::native::Gbm;
use backend::graphics::egl::wayland::{EGLDisplay, EGLWaylandExtensions};
#[cfg(feature = "backend_session")]
use backend::session::{AsSessionObserver, SessionObserver};
use drm::Device as BasicDevice;
use drm::control::{connector, crtc, encoder, Mode, ResourceInfo};
use drm::control::Device as ControlDevice;
use drm::control::framebuffer;
use drm::result::Error as DrmError;
use gbm::{BufferObject, Device as GbmDevice};
use nix;
use nix::sys::stat::{self, dev_t, fstat};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Error as IoError;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Once, ONCE_INIT};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use wayland_server::{Display, LoopToken};
use wayland_server::commons::Implementation;
use wayland_server::sources::{FdEvent, FdInterest, Source};

mod backend;
pub mod error;

pub use self::backend::DrmBackend;
use self::backend::DrmBackendInternal;
use self::error::*;

static LOAD: Once = ONCE_INIT;

/// Representation of an open drm device node to create rendering backends
pub struct DrmDevice<A: ControlDevice + 'static> {
    context: Rc<EGLContext<Gbm<framebuffer::Info>, GbmDevice<A>>>,
    old_state: HashMap<crtc::Handle, (crtc::Info, Vec<connector::Handle>)>,
    device_id: dev_t,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<DrmBackendInternal<A>>>>>,
    active: Arc<AtomicBool>,
    priviledged: bool,
    logger: ::slog::Logger,
}

impl<A: ControlDevice + 'static> DrmDevice<A> {
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

        let device_id = fstat(dev.as_raw_fd())
            .chain_err(|| ErrorKind::UnableToGetDeviceId)?
            .st_rdev;

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
                Default::default(),
                log.clone(),
            ).map_err(Error::from)?),
            backends: Rc::new(RefCell::new(HashMap::new())),
            device_id,
            old_state: HashMap::new(),
            active: Arc::new(AtomicBool::new(true)),
            priviledged: true,
            logger: log.clone(),
        };

        info!(log, "DrmDevice initializing");

        // we want to mode-set, so we better be the master, if we run via a tty session
        if let Err(_) = drm.set_master() {
            warn!(
                log,
                "Unable to become drm master, assuming unpriviledged mode"
            );
            drm.priviledged = false;
        };

        let res_handles = drm.resource_handles().chain_err(|| {
            ErrorKind::DrmDev(format!(
                "Error loading drm resources on {:?}",
                drm.dev_path()
            ))
        })?;
        for &con in res_handles.connectors() {
            let con_info = connector::Info::load_from_device(&drm, con).chain_err(|| {
                ErrorKind::DrmDev(format!(
                    "Error loading connector info on {:?}",
                    drm.dev_path()
                ))
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = encoder::Info::load_from_device(&drm, enc).chain_err(|| {
                    ErrorKind::DrmDev(format!(
                        "Error loading encoder info on {:?}",
                        drm.dev_path()
                    ))
                })?;
                if let Some(crtc) = enc_info.current_crtc() {
                    let info = crtc::Info::load_from_device(&drm, crtc).chain_err(|| {
                        ErrorKind::DrmDev(format!("Error loading crtc info on {:?}", drm.dev_path()))
                    })?;
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
    pub fn create_backend<I>(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: I,
    ) -> Result<DrmBackend<A>>
    where
        I: Into<Vec<connector::Handle>>,
    {
        if self.backends.borrow().contains_key(&crtc) {
            bail!(ErrorKind::CrtcAlreadyInUse(crtc));
        }

        if !self.active.load(Ordering::SeqCst) {
            bail!(ErrorKind::DeviceInactive);
        }

        // check if the given connectors and crtc match
        let connectors = connectors.into();

        // check if we have an encoder for every connector and the mode mode
        for connector in &connectors {
            let con_info = connector::Info::load_from_device(self, *connector).chain_err(|| {
                ErrorKind::DrmDev(format!(
                    "Error loading connector info on {:?}",
                    self.dev_path()
                ))
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
                        ErrorKind::DrmDev(format!(
                            "Error loading encoder info on {:?}",
                            self.dev_path()
                        ))
                    })
                })
                .collect::<Result<Vec<encoder::Info>>>()?;

            // and if any encoder supports the selected crtc
            let resource_handles = self.resource_handles().chain_err(|| {
                ErrorKind::DrmDev(format!(
                    "Error loading drm resources on {:?}",
                    self.dev_path()
                ))
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
        self.backends.borrow_mut().insert(crtc, backend.weak());
        Ok(backend)
    }

    /// Returns an internal device id, that is unique per boot per system
    pub fn device_id(&self) -> u64 {
        self.device_id
    }
}

/// Trait for types representing open devices
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

impl<A: ControlDevice + 'static> PartialEq for DrmDevice<A> {
    fn eq(&self, other: &DrmDevice<A>) -> bool {
        self.device_id == other.device_id
    }
}
impl<A: ControlDevice + 'static> Eq for DrmDevice<A> {}

impl<A: ControlDevice + 'static> Hash for DrmDevice<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.device_id.hash(state);
    }
}

// for users convinience and FdEventSource registering
impl<A: ControlDevice + 'static> AsRawFd for DrmDevice<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.context.as_raw_fd()
    }
}

impl<A: ControlDevice + 'static> BasicDevice for DrmDevice<A> {}
impl<A: ControlDevice + 'static> ControlDevice for DrmDevice<A> {}

impl<A: ControlDevice + 'static> EGLWaylandExtensions for DrmDevice<A> {
    fn bind_wl_display(&self, display: &Display) -> EGLResult<EGLDisplay> {
        self.context.bind_wl_display(display)
    }
}

impl<A: ControlDevice + 'static> Drop for DrmDevice<A> {
    fn drop(&mut self) {
        if Rc::strong_count(&self.context) > 1 {
            panic!("Pending DrmBackends. You need to free all backends before the DrmDevice gets destroyed");
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
        if self.priviledged {
            if let Err(err) = self.drop_master() {
                error!(
                    self.logger,
                    "Failed to drop drm master state. Error: {}", err
                );
            }
        }
    }
}

/// Handler for drm node events
///
/// See module-level documentation for its use
pub trait DrmHandler<A: ControlDevice + 'static> {
    /// The `DrmBackend` of crtc has finished swapping buffers and new frame can now
    /// (and should be immediately) be rendered.
    fn ready(&mut self, device: &mut DrmDevice<A>, crtc: crtc::Handle, frame: u32, duration: Duration);
    /// The `DrmDevice` has thrown an error.
    ///
    /// The related backends are most likely *not* usable anymore and
    /// the whole stack has to be recreated..
    fn error(&mut self, device: &mut DrmDevice<A>, error: DrmError);
}

/// Bind a `DrmDevice` to an `EventLoop`,
///
/// This will cause it to recieve events and feed them into an `DrmHandler`
pub fn drm_device_bind<A, H>(
    token: &LoopToken,
    device: DrmDevice<A>,
    handler: H,
) -> ::std::result::Result<(Source<FdEvent>, Rc<RefCell<DrmDevice<A>>>), (IoError, (DrmDevice<A>, H))>
where
    A: ControlDevice + 'static,
    H: DrmHandler<A> + 'static,
{
    let fd = device.as_raw_fd();
    let device = Rc::new(RefCell::new(device));
    match token.add_fd_event_source(
        fd,
        FdInterest::READ,
        DrmFdImpl {
            device: device.clone(),
            handler,
        },
    ) {
        Ok(source) => Ok((source, device)),
        Err((
            ioerror,
            DrmFdImpl {
                device: device2,
                handler,
            },
        )) => {
            // make the Rc unique again
            ::std::mem::drop(device2);
            let device = Rc::try_unwrap(device).unwrap_or_else(|_| unreachable!());
            Err((ioerror, (device.into_inner(), handler)))
        }
    }
}

struct DrmFdImpl<A: ControlDevice + 'static, H> {
    device: Rc<RefCell<DrmDevice<A>>>,
    handler: H,
}

impl<A, H> Implementation<(), FdEvent> for DrmFdImpl<A, H>
where
    A: ControlDevice + 'static,
    H: DrmHandler<A> + 'static,
{
    fn receive(&mut self, event: FdEvent, (): ()) {
        let mut device = self.device.borrow_mut();
        match event {
            FdEvent::Ready { .. } => match crtc::receive_events(&mut *device) {
                Ok(events) => for event in events {
                    if let crtc::Event::PageFlip(event) = event {
                        if device.active.load(Ordering::SeqCst) {
                            let backends = device.backends.borrow().clone();
                            if let Some(backend) = backends
                                .get(&event.crtc)
                                .iter()
                                .flat_map(|x| x.upgrade())
                                .next()
                            {
                                // we can now unlock the buffer
                                backend.unlock_buffer();
                                trace!(device.logger, "Handling event for backend {:?}", event.crtc);
                                // and then call the user to render the next frame
                                self.handler
                                    .ready(&mut device, event.crtc, event.frame, event.duration);
                            } else {
                                device.backends.borrow_mut().remove(&event.crtc);
                            }
                        }
                    }
                },
                Err(err) => self.handler.error(&mut device, err),
            },
            FdEvent::Error { error, .. } => {
                warn!(device.logger, "DrmDevice errored: {}", error);
                self.handler.error(&mut device, error.into());
            }
        }
    }
}

/// `SessionObserver` linked to the `DrmDevice` it was created from.
pub struct DrmDeviceObserver<A: ControlDevice + 'static> {
    context: Weak<EGLContext<Gbm<framebuffer::Info>, GbmDevice<A>>>,
    device_id: dev_t,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<DrmBackendInternal<A>>>>>,
    old_state: HashMap<crtc::Handle, (crtc::Info, Vec<connector::Handle>)>,
    active: Arc<AtomicBool>,
    priviledged: bool,
    logger: ::slog::Logger,
}

#[cfg(feature = "backend_session")]
impl<A: ControlDevice + 'static> AsSessionObserver<DrmDeviceObserver<A>> for DrmDevice<A> {
    fn observer(&mut self) -> DrmDeviceObserver<A> {
        DrmDeviceObserver {
            context: Rc::downgrade(&self.context),
            device_id: self.device_id.clone(),
            backends: self.backends.clone(),
            old_state: self.old_state.clone(),
            active: self.active.clone(),
            priviledged: self.priviledged,
            logger: self.logger.clone(),
        }
    }
}

#[cfg(feature = "backend_session")]
impl<A: ControlDevice + 'static> SessionObserver for DrmDeviceObserver<A> {
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        if let Some((major, minor)) = devnum {
            if major as u64 != stat::major(self.device_id) || minor as u64 != stat::minor(self.device_id) {
                return;
            }
        }
        if let Some(device) = self.context.upgrade() {
            for (handle, &(ref info, ref connectors)) in self.old_state.iter() {
                if let Err(err) = crtc::set(
                    &*device,
                    *handle,
                    info.fb(),
                    connectors,
                    info.position(),
                    info.mode(),
                ) {
                    error!(
                        self.logger,
                        "Failed to reset crtc ({:?}). Error: {}", handle, err
                    );
                }
            }
        }
        self.active.store(false, Ordering::SeqCst);
        if self.priviledged {
            if let Some(device) = self.context.upgrade() {
                if let Err(err) = device.drop_master() {
                    error!(
                        self.logger,
                        "Failed to drop drm master state. Error: {}", err
                    );
                }
            }
        }
    }

    fn activate(&mut self, devnum: Option<(u32, u32, Option<RawFd>)>) {
        if let Some((major, minor, fd)) = devnum {
            if major as u64 != stat::major(self.device_id) || minor as u64 != stat::minor(self.device_id) {
                return;
            } else if let Some(fd) = fd {
                info!(self.logger, "Replacing fd");
                if let Some(device) = self.context.upgrade() {
                    nix::unistd::dup2(device.as_raw_fd(), fd)
                        .expect("Failed to replace file descriptor of drm device");
                }
            }
        }
        self.active.store(true, Ordering::SeqCst);
        if self.priviledged {
            if let Some(device) = self.context.upgrade() {
                if let Err(err) = device.set_master() {
                    crit!(
                        self.logger,
                        "Failed to acquire drm master again. Error: {}",
                        err
                    );
                }
            }
        }
        let mut crtcs = Vec::new();
        for (crtc, backend) in self.backends.borrow().iter() {
            if let Some(backend) = backend.upgrade() {
                backend.unlock_buffer();
                if let Err(err) = backend.page_flip(None) {
                    error!(
                        self.logger,
                        "Failed to activate crtc ({:?}) again. Error: {}", crtc, err
                    );
                }
                // reset cursor
                {
                    let &(ref cursor, ref hotspot): &(BufferObject<()>, (u32, u32)) =
                        unsafe { &*backend.cursor.as_ptr() };
                    if crtc::set_cursor2(
                        &*backend.context,
                        *crtc,
                        cursor,
                        ((*hotspot).0 as i32, (*hotspot).1 as i32),
                    ).is_err()
                    {
                        if let Err(err) = crtc::set_cursor(&*backend.context, *crtc, cursor) {
                            error!(self.logger, "Failed to reset cursor. Error: {}", err);
                        }
                    }
                }
            } else {
                crtcs.push(*crtc);
            }
        }
        for crtc in crtcs {
            self.backends.borrow_mut().remove(&crtc);
        }
    }
}
