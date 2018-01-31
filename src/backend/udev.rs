//!
//! Provides `udev` related functionality for automated device scanning.
//!
//! This module mainly provides the `UdevBackend`, which constantly monitors available drm devices
//! and notifies a user supplied `UdevHandler` of any changes.
//!
//! Additionally this contains some utility functions related to scanning.
//!
//! See also `examples/udev.rs` for pure hardware backed example of a compositor utilizing this
//! backend.

use backend::drm::{drm_device_bind, DrmDevice, DrmHandler};
use backend::session::{Session, SessionObserver, AsSessionObserver};
use drm::Device as BasicDevice;
use drm::control::Device as ControlDevice;
use nix::fcntl;
use nix::sys::stat::dev_t;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Error as IoError, Result as IoResult};
use std::mem::drop;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use udev::{Context, Enumerator, Event, EventType, MonitorBuilder, MonitorSocket, Result as UdevResult};
use wayland_server::EventLoopHandle;
use wayland_server::sources::{EventSource, FdEventSource, FdEventSourceImpl, FdInterest};

/// Udev's `DrmDevice` type based on the underlying session
pub struct SessionFdDrmDevice(RawFd);

impl AsRawFd for SessionFdDrmDevice {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}
impl BasicDevice for SessionFdDrmDevice {}
impl ControlDevice for SessionFdDrmDevice {}

/// Graphical backend that monitors available drm devices.
///
/// Provides a way to automatically initialize a `DrmDevice` for available gpus and notifies the
/// given handler of any changes. Can be used to provide hot-plug functionality for gpus and
/// attached monitors.
pub struct UdevBackend<
    H: DrmHandler<SessionFdDrmDevice> + 'static,
    S: Session + 'static,
    T: UdevHandler<H> + 'static,
> {
    devices: Rc<RefCell<HashMap<dev_t, FdEventSource<(DrmDevice<SessionFdDrmDevice>, H)>>>>,
    monitor: MonitorSocket,
    session: S,
    handler: T,
    logger: ::slog::Logger,
}

impl<H: DrmHandler<SessionFdDrmDevice> + 'static, S: Session + 'static, T: UdevHandler<H> + 'static>
    UdevBackend<H, S, T>
{
    /// Creates a new `UdevBackend` and adds it to the given `EventLoop`'s state.
    ///
    /// ## Arguments
    /// `evlh` - An event loop to use for binding `DrmDevices`
    /// `context` - An initialized udev context
    /// `session` - A session used to open and close devices as they become available
    /// `handler` - User-provided handler to respond to any detected changes
    /// `logger`  - slog Logger to be used by the backend and its `DrmDevices`.
    pub fn new<'a, L>(
        mut evlh: &mut EventLoopHandle, context: &Context, mut session: S, mut handler: T, logger: L
    ) -> Result<UdevBackend<H, S, T>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_udev"));
        let seat = session.seat();
        let devices = all_gpus(context, seat)
            .chain_err(|| ErrorKind::FailedToScan)?
            .into_iter()
            // Create devices
            .flat_map(|path| {
                match DrmDevice::new(
                    {
                        match session.open(&path, fcntl::O_RDWR | fcntl::O_CLOEXEC | fcntl::O_NOCTTY | fcntl::O_NONBLOCK) {
                            Ok(fd) => SessionFdDrmDevice(fd),
                            Err(err) => {
                                warn!(logger, "Unable to open drm device {:?}, Error: {:?}. Skipping", path, err);
                                return None;
                            }
                        }
                    }, logger.clone()
                ) {
                    // Call the handler, which might add it to the runloop
                    Ok(mut device) => {
                        let fd = device.as_raw_fd();
                        let devnum = device.device_id();
                        match handler.device_added(evlh, &mut device) {
                            Some(drm_handler) => {
                                if let Ok(event_source) = drm_device_bind(&mut evlh, device, drm_handler) {
                                    Some((devnum, event_source))
                                } else {
                                    //TODO fix wayland_server to return idata on error
                                    // handler.device_removed(evlh, &mut device);
                                    // drop(device);
                                    if let Err(err) = session.close(fd) {
                                        warn!(logger, "Failed to close dropped device. Error: {:?}. Ignoring", err);
                                    };
                                    None
                                }
                            },
                            None => {
                                drop(device); //drops master
                                if let Err(err) = session.close(fd) {
                                    warn!(logger, "Failed to close device. Error: {:?}. Ignoring", err);
                                }
                                None
                            }
                        }
                    },
                    Err(err) => {
                        warn!(logger, "Failed to initialize device {:?}. Error: {:?}. Skipping", path, err);
                        None
                    }
                }
            })
            .collect::<HashMap<dev_t, FdEventSource<(DrmDevice<SessionFdDrmDevice>, H)>>>();

        let mut builder = MonitorBuilder::new(context).chain_err(|| ErrorKind::FailedToInitMonitor)?;
        builder
            .match_subsystem("drm")
            .chain_err(|| ErrorKind::FailedToInitMonitor)?;
        let monitor = builder
            .listen()
            .chain_err(|| ErrorKind::FailedToInitMonitor)?;

        Ok(UdevBackend {
            devices: Rc::new(RefCell::new(devices)),
            monitor,
            session,
            handler,
            logger,
        })
    }

    /// Closes the udev backend and frees all remaining open devices.
    pub fn close(&mut self, evlh: &mut EventLoopHandle) {
        let mut devices = self.devices.borrow_mut();
        for (_, event_source) in devices.drain() {
            let (mut device, _) = event_source.remove();
            self.handler.device_removed(evlh, &mut device);
            let fd = device.as_raw_fd();
            drop(device);
            if let Err(err) = self.session.close(fd) {
                warn!(
                    self.logger,
                    "Failed to close device. Error: {:?}. Ignoring", err
                );
            };
        }
        info!(self.logger, "All devices closed");
    }
}

/// `SessionObserver` linked to the `UdevBackend` it was created from.
pub struct UdevBackendObserver<H: DrmHandler<SessionFdDrmDevice> + 'static> {
    devices: Weak<RefCell<HashMap<dev_t, FdEventSource<(DrmDevice<SessionFdDrmDevice>, H)>>>>,
    logger: ::slog::Logger,
}

impl<
    H: DrmHandler<SessionFdDrmDevice> + 'static,
    S: Session + 'static,
    T: UdevHandler<H> + 'static,
> AsSessionObserver<UdevBackendObserver<H>> for UdevBackend<H, S, T> {
    fn observer(&mut self) -> UdevBackendObserver<H> {
        UdevBackendObserver {
            devices: Rc::downgrade(&self.devices),
            logger: self.logger.clone(),
        }
    }
}

impl<H: DrmHandler<SessionFdDrmDevice> + 'static> SessionObserver for UdevBackendObserver<H>
{
    fn pause<'a>(&mut self, evlh: &mut EventLoopHandle, devnum: Option<(u32, u32)>) {
        if let Some(devices) = self.devices.upgrade() {
            for fd_event_source in devices.borrow_mut().values_mut() {
                fd_event_source.with_idata(evlh, |&mut (ref mut device, _), evlh| {
                    info!(self.logger, "changed successful");
                    device.observer().pause(evlh, devnum);
                })
            }
        }
    }

    fn activate<'a>(&mut self, evlh: &mut EventLoopHandle, devnum: Option<(u32, u32, Option<RawFd>)>) {
        if let Some(devices) = self.devices.upgrade() {
            for fd_event_source in devices.borrow_mut().values_mut() {
                fd_event_source.with_idata(evlh, |&mut (ref mut device, _), evlh| {
                    info!(self.logger, "changed successful");
                    device.observer().activate(evlh, devnum);
                })
            }
        }
    }
}

/// Binds a `UdevBackend` to a given `EventLoop`.
///
/// Allows the backend to recieve kernel events and thus to drive the `UdevHandler`.
/// No runtime functionality can be provided without using this function.
pub fn udev_backend_bind<S, H, T>(
    evlh: &mut EventLoopHandle, udev: UdevBackend<H, S, T>
) -> IoResult<FdEventSource<UdevBackend<H, S, T>>>
where
    H: DrmHandler<SessionFdDrmDevice> + 'static,
    T: UdevHandler<H> + 'static,
    S: Session + 'static,
{
    let fd = udev.monitor.as_raw_fd();
    evlh.add_fd_event_source(fd, fd_event_source_implementation(), udev, FdInterest::READ)
}

fn fd_event_source_implementation<S, H, T>() -> FdEventSourceImpl<UdevBackend<H, S, T>>
where
    H: DrmHandler<SessionFdDrmDevice> + 'static,
    T: UdevHandler<H> + 'static,
    S: Session + 'static,
{
    FdEventSourceImpl {
        ready: |mut evlh, udev, _, _| {
            let events = udev.monitor.clone().collect::<Vec<Event>>();
            for event in events {
                match event.event_type() {
                    // New device
                    EventType::Add => {
                        info!(udev.logger, "Device Added");
                        if let (Some(path), Some(devnum)) = (event.devnode(), event.devnum()) {
                            let mut device = {
                                match DrmDevice::new(
                                    {
                                        let logger = udev.logger.clone();
                                        match udev.session.open(
                                            path,
                                            fcntl::O_RDWR | fcntl::O_CLOEXEC | fcntl::O_NOCTTY
                                                | fcntl::O_NONBLOCK,
                                        ) {
                                            Ok(fd) => SessionFdDrmDevice(fd),
                                            Err(err) => {
                                                warn!(
                                                    logger,
                                                    "Unable to open drm device {:?}, Error: {:?}. Skipping",
                                                    path,
                                                    err
                                                );
                                                continue;
                                            }
                                        }
                                    },
                                    udev.logger.clone(),
                                ) {
                                    Ok(dev) => dev,
                                    Err(err) => {
                                        warn!(
                                            udev.logger,
                                            "Failed to initialize device {:?}. Error: {}. Skipping",
                                            path,
                                            err
                                        );
                                        continue;
                                    }
                                }
                            };
                            let fd = device.as_raw_fd();
                            match udev.handler.device_added(evlh, &mut device) {
                                Some(drm_handler) => {
                                    if let Ok(fd_event_source) =
                                        drm_device_bind(&mut evlh, device, drm_handler)
                                    {
                                        udev.devices.borrow_mut().insert(devnum, fd_event_source);
                                    } else {
                                        //TODO fix wayland_server to return idata on error
                                        //udev.handler.device_removed(evlh, &mut device);
                                        //drop(device);
                                        if let Err(err) = udev.session.close(fd) {
                                            warn!(
                                                udev.logger,
                                                "Failed to close dropped device. Error: {:?}. Ignoring",
                                                err
                                            );
                                        };
                                    }
                                }
                                None => {
                                    drop(device);
                                    if let Err(err) = udev.session.close(fd) {
                                        warn!(
                                            udev.logger,
                                            "Failed to close unused device. Error: {:?}", err
                                        );
                                    }
                                }
                            };
                        }
                    }
                    // Device removed
                    EventType::Remove => {
                        info!(udev.logger, "Device Remove");
                        if let Some(devnum) = event.devnum() {
                            if let Some(fd_event_source) = udev.devices.borrow_mut().remove(&devnum) {
                                let (mut device, _) = fd_event_source.remove();
                                udev.handler.device_removed(evlh, &mut device);
                                let fd = device.as_raw_fd();
                                drop(device);
                                if let Err(err) = udev.session.close(fd) {
                                    warn!(
                                        udev.logger,
                                        "Failed to close device {:?}. Error: {:?}. Ignoring",
                                        event.sysname(),
                                        err
                                    );
                                };
                            }
                        }
                    },
                    // New connector
                    EventType::Change => {
                        info!(udev.logger, "Device Changed");
                        if let Some(devnum) = event.devnum() {
                            info!(udev.logger, "Devnum: {:b}", devnum);
                            if let Some(fd_event_source) = udev.devices.borrow_mut().get_mut(&devnum) {
                                let handler = &mut udev.handler;
                                let logger = &udev.logger;
                                fd_event_source.with_idata(evlh, move |&mut (ref mut device, _), evlh| {
                                    info!(logger, "changed successful");
                                    handler.device_changed(evlh, device);
                                })
                            } else {
                                info!(udev.logger, "changed, but device not tracked by backend");
                            };
                        } else {
                            info!(udev.logger, "changed, but no devnum");
                        }
                    },
                    _ => {}
                }
            }
        },
        error: |evlh, udev, _, err| {
            udev.handler.error(evlh, err)
        },
    }
}

/// Handler for the `UdevBackend`, allows to open, close and update drm devices as they change during runtime.
pub trait UdevHandler<H: DrmHandler<SessionFdDrmDevice> + 'static> {
    /// Called on initialization for every known device and when a new device is detected.
    ///
    /// Returning a `DrmHandler` will initialize the device, returning `None` will ignore the device.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn device_added(
        &mut self, evlh: &mut EventLoopHandle, device: &mut DrmDevice<SessionFdDrmDevice>
    ) -> Option<H>;
    /// Called when an open device is changed.
    ///
    /// This usually indicates that some connectors did become available or were unplugged. The handler
    /// should scan again for connected monitors and mode switch accordingly.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn device_changed(
        &mut self, evlh: &mut EventLoopHandle, device: &mut DrmDevice<SessionFdDrmDevice>
    );
    /// Called when a device was removed.
    ///
    /// The device will not accept any operations anymore and its file descriptor will be closed once
    /// this function returns, any open references/tokens to this device need to be released.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn device_removed(
        &mut self, evlh: &mut EventLoopHandle, device: &mut DrmDevice<SessionFdDrmDevice>
    );
    /// Called when the udev context has encountered and error.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn error(&mut self, evlh: &mut EventLoopHandle, error: IoError);
}

/// Returns the path of the primary gpu device if any
///
/// Might be used for filtering in `UdevHandler::device_added` or for manual `DrmDevice` initialization
pub fn primary_gpu<S: AsRef<str>>(context: &Context, seat: S) -> UdevResult<Option<PathBuf>> {
    let mut enumerator = Enumerator::new(context)?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;

    let mut result = None;
    for device in enumerator.scan_devices()? {
        if device
            .property_value("ID_SEAT")
            .map(|x| x.to_os_string())
            .unwrap_or(OsString::from("seat0")) == *seat.as_ref()
        {
            if let Some(pci) = device.parent_with_subsystem(Path::new("pci"))? {
                if let Some(id) = pci.attribute_value("boot_vga") {
                    if id == "1" {
                        result = Some(device);
                    }
                }
            } else if result.is_none() {
                result = Some(device);
            }
        }
    }
    Ok(result.and_then(|device| device.devnode().map(PathBuf::from)))
}

/// Returns the paths of all available gpu devices
///
/// Might be used for manual `DrmDevice` initialization
pub fn all_gpus<S: AsRef<str>>(context: &Context, seat: S) -> UdevResult<Vec<PathBuf>> {
    let mut enumerator = Enumerator::new(context)?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;
    Ok(enumerator
        .scan_devices()?
        .filter(|device| {
            device
                .property_value("ID_SEAT")
                .map(|x| x.to_os_string())
                .unwrap_or(OsString::from("seat0")) == *seat.as_ref()
        })
        .flat_map(|device| device.devnode().map(PathBuf::from))
        .collect())
}

error_chain! {
    errors {
        #[doc = "Failed to scan for devices"]
        FailedToScan {
            description("Failed to scan for devices"),
        }

        #[doc = "Failed to initialize udev monitor"]
        FailedToInitMonitor {
            description("Failed to initialize udev monitor"),
        }

        #[doc = "Failed to identify devices"]
        FailedToIdentifyDevices {
            description("Failed to identify devices"),
        }
    }
}
