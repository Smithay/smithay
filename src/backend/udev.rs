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

use libudev::{Context, MonitorBuilder, MonitorSocket, Event, EventType, Enumerator, Result as UdevResult};
use nix::fcntl;
use nix::sys::stat::{dev_t, fstat};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::io::{Error as IoError, Result as IoResult};
use std::ffi::OsString;
use std::mem::drop;
use std::path::{PathBuf, Path};
use std::os::unix::io::AsRawFd;
use wayland_server::{EventLoopHandle, StateToken, StateProxy};
use wayland_server::sources::{FdEventSource, FdEventSourceImpl, FdInterest};

use ::backend::drm::{DrmDevice, DrmBackend, DrmHandler, drm_device_bind};
use ::backend::session::{Session, SessionObserver};

/// Graphical backend that monitors available drm devices.
///
/// Provides a way to automatically initialize a `DrmDevice` for available gpus and notifies the
/// given handler of any changes. Can be used to provide hot-plug functionality for gpus and
/// attached monitors.
pub struct UdevBackend<B: Borrow<DrmBackend> + 'static, H: DrmHandler<B> + 'static, S: Session + 'static, T: UdevHandler<B, H> + 'static> {
    devices: HashMap<dev_t, (StateToken<DrmDevice<B>>, FdEventSource<(StateToken<DrmDevice<B>>, H)>)>,
    monitor: MonitorSocket,
    session: S,
    handler: T,
    logger: ::slog::Logger,
}

impl<B: From<DrmBackend> + Borrow<DrmBackend> + 'static, H: DrmHandler<B> + 'static, S: Session + 'static, T: UdevHandler<B, H> + 'static> UdevBackend<B, H, S, T> {
    /// Creates a new `UdevBackend` and adds it to the given `EventLoop`'s state.
    ///
    /// ## Arguments
    /// `evlh` - An event loop to use for binding `DrmDevices`
    /// `context` - An initialized udev context
    /// `session` - A session used to open and close devices as they become available
    /// `handler` - User-provided handler to respond to any detected changes
    /// `logger`  - slog Logger to be used by the backend and its `DrmDevices`.
    pub fn new<'a, L>(mut evlh: &mut EventLoopHandle,
                      context: &Context,
                      mut session: S,
                      mut handler: T,
                      logger: L)
        -> Result<StateToken<UdevBackend<B, H, S, T>>>
    where
        L: Into<Option<::slog::Logger>>
    {
        let logger = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_udev"));
        let seat = session.seat();
        let devices = all_gpus(context, seat)
            .chain_err(|| ErrorKind::FailedToScan)?
            .into_iter()
            // Create devices
            .flat_map(|path| {
                match unsafe { DrmDevice::new_from_fd(
                    {
                        match session.open(&path, fcntl::O_RDWR | fcntl::O_CLOEXEC | fcntl::O_NOCTTY | fcntl::O_NONBLOCK) {
                            Ok(fd) => fd,
                            Err(err) => {
                                warn!(logger, "Unable to open drm device {:?}, Error: {:?}. Skipping", path, err);
                                return None;
                            }
                        }
                    }, logger.clone()
                ) } {
                    // Call the handler, which might add it to the runloop
                    Ok(mut device) => match handler.device_added(&mut evlh.state().as_proxy(), &mut device) {
                        // fstat them
                        Some(drm_handler) => match fstat(device.as_raw_fd()) {
                            Ok(stat) => {
                                let token = evlh.state().insert(device);
                                if let Ok(event_source) = drm_device_bind(&mut evlh, token.clone(), drm_handler) {
                                    Some((stat.st_rdev, (token, event_source)))
                                } else {
                                    handler.device_removed(evlh.state(), &token);
                                    let device = evlh.state().remove(token);
                                    let fd = device.as_raw_fd();
                                    drop(device);
                                    if let Err(err) = session.close(fd) {
                                        warn!(logger, "Failed to close dropped device. Error: {:?}. Ignoring", err);
                                    };
                                    None
                                }
                            },
                            Err(err) => {
                                // almost impossible to hit, but lets do it as good as possible
                                error!(logger, "Failed to get devnum of newly initialized device, dropping. Error: {:?}", err);
                                let token = evlh.state().insert(device);
                                handler.device_removed(evlh.state(), &token);
                                let device = evlh.state().remove(token);
                                let fd = device.as_raw_fd();
                                drop(device);
                                if let Err(err) = session.close(fd) {
                                    warn!(logger, "Failed to close dropped device. Error: {:?}. Ignoring", err);
                                };
                                None
                            }
                        },
                        None => {
                            let fd = device.as_raw_fd();
                            drop(device); //drops master
                            if let Err(err) = session.close(fd) {
                                warn!(logger, "Failed to close device. Error: {:?}. Ignoring", err);
                            }
                            None
                        }
                    },
                    Err(err) => {
                        warn!(logger, "Failed to initialize device {:?}. Error: {:?}. Skipping", path, err);
                        return None;
                    }
                }
            })
            .collect::<HashMap<dev_t, (StateToken<DrmDevice<B>>, FdEventSource<(StateToken<DrmDevice<B>>, H)>)>>();

        let mut builder = MonitorBuilder::new(context).chain_err(|| ErrorKind::FailedToInitMonitor)?;
        builder.match_subsystem("drm").chain_err(|| ErrorKind::FailedToInitMonitor)?;
        let monitor = builder.listen().chain_err(|| ErrorKind::FailedToInitMonitor)?;

        Ok(evlh.state().insert(UdevBackend {
            devices,
            monitor,
            session,
            handler,
            logger,
        }))
    }

    /// Closes the udev backend and frees all remaining open devices.
    ///
    /// Needs to be called after the `FdEventSource` was removed and the backend was removed from
    /// the `EventLoop`'s `State`.
    ///
    /// ## Panics
    /// The given state might be passed to the registered `UdevHandler::device_removed` callback.
    /// Make sure not to borrow any tokens twice.
    pub fn close<'a, ST: Into<StateProxy<'a>>>(mut self, state: ST) {
        let mut state = state.into();
        for (_, (mut device, event_source)) in self.devices.drain() {
            event_source.remove();
            self.handler.device_removed(&mut state, &device);
            let device = state.remove(device);
            let fd = device.as_raw_fd();
            drop(device);
            if let Err(err) = self.session.close(fd) {
                warn!(self.logger, "Failed to close device. Error: {:?}. Ignoring", err);
            };
        }
        info!(self.logger, "All devices closed");
    }
}

impl<B: Borrow<DrmBackend> + 'static, H: DrmHandler<B> + 'static, S: Session + 'static, T: UdevHandler<B, H> + 'static> SessionObserver for StateToken<UdevBackend<B, H, S, T>> {
    fn pause<'a>(&mut self, state: &mut StateProxy<'a>) {
        state.with_value(self, |state, udev| {
            for &mut (ref mut device, _) in udev.devices.values_mut() {
                device.pause(state);
            }
        });
    }

    fn activate<'a>(&mut self, state: &mut StateProxy<'a>) {
        state.with_value(self, |state, udev| {
            for &mut (ref mut device, _) in udev.devices.values_mut() {
                device.activate(state);
            }
        });
    }
}

/// Binds a `UdevBackend` to a given `EventLoop`.
///
/// Allows the backend to recieve kernel events and thus to drive the `UdevHandler`.
/// No runtime functionality can be provided without using this function.
pub fn udev_backend_bind<B, S, H, T>(evlh: &mut EventLoopHandle, udev: StateToken<UdevBackend<B, H, S, T>>)
    -> IoResult<FdEventSource<StateToken<UdevBackend<B, H, S, T>>>>
where
    B: From<DrmBackend> + Borrow<DrmBackend> + 'static,
    H: DrmHandler<B> + 'static,
    T: UdevHandler<B, H> + 'static,
    S: Session + 'static,
{
    let fd = evlh.state().get(&udev).monitor.as_raw_fd();
    evlh.add_fd_event_source(
        fd,
        fd_event_source_implementation(),
        udev,
        FdInterest::READ,
    )
}

fn fd_event_source_implementation<B, S, H, T>()
    -> FdEventSourceImpl<StateToken<UdevBackend<B, H, S, T>>>
where
    B: From<DrmBackend> + Borrow<DrmBackend> + 'static,
    H: DrmHandler<B> + 'static,
    T: UdevHandler<B, H> + 'static,
    S: Session + 'static,
{
    FdEventSourceImpl {
        ready: |mut evlh, token, _, _| {
            let events = evlh.state().get(token).monitor.clone().collect::<Vec<Event>>();
            for event in events {
                match event.event_type() {
                    // New device
                    EventType::Add => {
                        info!(evlh.state().get(token).logger, "Device Added");
                        if let (Some(path), Some(devnum)) = (event.devnode(), event.devnum()) {
                            let mut device = {
                                match unsafe { DrmDevice::new_from_fd(
                                    {
                                        let logger = evlh.state().get(token).logger.clone();
                                        match evlh.state().get_mut(token).session.open(path, fcntl::O_RDWR | fcntl::O_CLOEXEC | fcntl::O_NOCTTY | fcntl::O_NONBLOCK) {
                                            Ok(fd) => fd,
                                            Err(err) => {
                                                warn!(logger, "Unable to open drm device {:?}, Error: {:?}. Skipping", path, err);
                                                continue;
                                            }
                                        }
                                    }, evlh.state().get(token).logger.clone()
                                ) } {
                                    Ok(dev) => dev,
                                    Err(err) => {
                                        warn!(evlh.state().get(token).logger, "Failed to initialize device {:?}. Error: {}. Skipping", path, err);
                                        continue;
                                    }
                                }
                            };
                            match evlh.state().with_value(token, |state, udev| udev.handler.device_added(state, &mut device)) {
                                Some(drm_handler) => {
                                    let dev_token = evlh.state().insert(device);
                                    if let Ok(fd_event_source) = drm_device_bind(&mut evlh, dev_token.clone(), drm_handler) {
                                        evlh.state().get_mut(token).devices.insert(devnum, (dev_token, fd_event_source));
                                    } else {
                                        evlh.state().with_value(token, |state, udev| {
                                            let mut state: StateProxy = state.into();
                                            udev.handler.device_removed(&mut state, &dev_token);
                                            let device = state.remove(dev_token);
                                            let fd = device.as_raw_fd();
                                            drop(device);
                                            if let Err(err) = udev.session.close(fd) {
                                                warn!(udev.logger, "Failed to close dropped device. Error: {:?}. Ignoring", err);
                                            };
                                        })
                                    }
                                },
                                None => {
                                    let fd = device.as_raw_fd();
                                    drop(device);
                                    evlh.state().with_value(token, |_state, udev| {
                                        if let Err(err) = udev.session.close(fd) {
                                            warn!(udev.logger, "Failed to close unused device. Error: {:?}", err);
                                        }
                                    })
                                },
                            };
                        }
                    },
                    // Device removed
                    EventType::Remove => {
                        evlh.state().with_value(token, |state, udev| {
                            info!(udev.logger, "Device Remove");
                            if let Some(devnum) = event.devnum() {
                                if let Some((device, fd_event_source)) = udev.devices.remove(&devnum) {
                                    fd_event_source.remove();
                                    let mut state: StateProxy = state.into();
                                    udev.handler.device_removed(&mut state, &device);
                                    let device = state.remove(device);
                                    let fd = device.as_raw_fd();
                                    drop(device);
                                    if let Err(err) = udev.session.close(fd) {
                                        warn!(udev.logger, "Failed to close device {:?}. Error: {:?}. Ignoring", event.sysname(), err);
                                    };
                                }
                            }
                        })
                    },
                    // New connector
                    EventType::Change => evlh.state().with_value(token, |state, udev| {
                        info!(udev.logger, "Device Changed");
                        if let Some(devnum) = event.devnum() {
                            info!(udev.logger, "Devnum: {:b}", devnum);
                            if let Some(&(ref device, _)) = udev.devices.get(&devnum) {
                                info!(udev.logger, "changed successful");
                                udev.handler.device_changed(state, device);
                            } else {
                                info!(udev.logger, "changed, but device not tracked by backend");
                            }
                        } else {
                            info!(udev.logger, "changed, but no devnum");
                        }
                    }),
                    _ => {},
                }
            }
        },
        error: |evlh, token, _, err| {
            evlh.state().with_value(token, |state, udev| udev.handler.error(state, err))
        },
    }
}

/// Handler for the `UdevBackend`, allows to open, close and update drm devices as they change during runtime.
pub trait UdevHandler<B: Borrow<DrmBackend> + 'static, H: DrmHandler<B> + 'static> {
    /// Called on initialization for every known device and when a new device is detected.
    ///
    /// Returning a `DrmHandler` will initialize the device, returning `None` will ignore the device.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn device_added<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, device: &mut DrmDevice<B>) -> Option<H>;
    /// Called when an open device is changed.
    ///
    /// This usually indicates that some connectors did become available or were unplugged. The handler
    /// should scan again for connected monitors and mode switch accordingly.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn device_changed<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, device: &StateToken<DrmDevice<B>>);
    /// Called when a device was removed.
    ///
    /// The device will not accept any operations anymore and its file descriptor will be closed once
    /// this function returns, any open references/tokens to this device need to be released.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn device_removed<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, device: &StateToken<DrmDevice<B>>);
    /// Called when the udev context has encountered and error.
    ///
    /// ## Panics
    /// Panics if you try to borrow the token of the belonging `UdevBackend` using this `StateProxy`.
    fn error<'a, S: Into<StateProxy<'a>>>(&mut self, state: S, error: IoError);
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
        if device.property_value("ID_SEAT").map(|x| x.to_os_string()).unwrap_or(OsString::from("seat0")) == *seat.as_ref() {
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
    Ok(enumerator.scan_devices()?
        .filter(|device| device.property_value("ID_SEAT").map(|x| x.to_os_string()).unwrap_or(OsString::from("seat0")) == *seat.as_ref())
        .flat_map(|device| device.devnode().map(PathBuf::from))
        .collect()
    )
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
