//! `udev` related functionality for automated device scanning
//!
//! This module mainly provides the [`UdevBackend`], which monitors available DRM devices and acts as
//! an event source to be inserted in [`calloop`], generating events whenever these devices change.
//!
//! *Note:* Once inserted into the event loop, the [`UdevBackend`] will only notify you about *changes*
//! in the device list. To get an initial snapshot of the state during your initialization, you need to
//! call its `device_list` method.
//!
//! ```no_run
//! use smithay::backend::udev::{UdevBackend, UdevEvent};
//!
//! let udev = UdevBackend::new("seat0", None).expect("Failed to monitor udev.");
//!
//! for (dev_id, node_path) in udev.device_list() {
//!     // process the initial list of devices
//! }
//!
//! # let event_loop = smithay::reexports::calloop::EventLoop::<()>::try_new().unwrap();
//! # let loop_handle = event_loop.handle();
//! // setup the event source for long-term monitoring
//! loop_handle.insert_source(udev, |event, _, _dispatch_data| match event {
//!     UdevEvent::Added { device_id, path } => {
//!         // a new device has been added
//!     },
//!     UdevEvent::Changed { device_id } => {
//!         // a device has been changed
//!     },
//!     UdevEvent::Removed { device_id } => {
//!         // a device has been removed
//!     }
//! }).expect("Failed to insert the udev source into the event loop");
//! ```
//!
//! Additionally this contains some utility functions related to scanning.
//!
//! See also `anvil/src/udev.rs` for pure hardware backed example of a compositor utilizing this
//! backend.

use nix::sys::stat::{dev_t, stat};
use std::{
    collections::HashMap,
    ffi::OsString,
    fmt,
    io::Result as IoResult,
    os::unix::io::{AsRawFd, RawFd},
    path::{Path, PathBuf},
};
use udev::{Enumerator, EventType, MonitorBuilder, MonitorSocket};

use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

use slog::{debug, info, o, warn};

/// Backend to monitor available drm devices.
///
/// Provides a way to automatically scan for available gpus and notifies the
/// given handler of any changes. Can be used to provide hot-plug functionality for gpus and
/// attached monitors.
pub struct UdevBackend {
    devices: HashMap<dev_t, PathBuf>,
    monitor: MonitorSocket,
    token: Token,
    logger: ::slog::Logger,
}

// MonitorSocket does not implement debug, so we have to impl Debug manually
impl fmt::Debug for UdevBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use udev::AsRaw;
        f.debug_struct("UdevBackend")
            .field("devices", &self.devices)
            .field("monitor", &format!("MonitorSocket ({:?})", self.monitor.as_raw()))
            .field("logger", &self.logger)
            .finish()
    }
}

impl AsRawFd for UdevBackend {
    fn as_raw_fd(&self) -> RawFd {
        self.monitor.as_raw_fd()
    }
}

impl UdevBackend {
    /// Creates a new [`UdevBackend`]
    ///
    /// ## Arguments
    /// `seat`    - system seat which should be bound
    /// `logger`  - slog Logger to be used by the backend and its `DrmDevices`.
    pub fn new<L, S: AsRef<str>>(seat: S, logger: L) -> IoResult<UdevBackend>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_udev"));

        let devices = all_gpus(seat)?
            .into_iter()
            // Create devices
            .flat_map(|path| match stat(&path) {
                Ok(stat) => Some((stat.st_rdev, path)),
                Err(err) => {
                    warn!(log, "Unable to get id of {:?}, Error: {:?}. Skipping", path, err);
                    None
                }
            })
            .collect();

        let monitor = MonitorBuilder::new()?.match_subsystem("drm")?.listen()?;

        Ok(UdevBackend {
            devices,
            monitor,
            token: Token::invalid(),
            logger: log,
        })
    }

    /// Get a list of DRM devices currently known to the backend
    ///
    /// You should call this once before inserting the event source into your
    /// event loop, to get an initial snapshot of the device state.
    pub fn device_list(&self) -> impl Iterator<Item = (dev_t, &Path)> {
        self.devices.iter().map(|(&id, path)| (id, path.as_ref()))
    }
}

impl EventSource for UdevBackend {
    type Event = UdevEvent;
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(
        &mut self,
        _: Readiness,
        token: Token,
        mut callback: F,
    ) -> std::io::Result<PostAction>
    where
        F: FnMut(UdevEvent, &mut ()),
    {
        if token != self.token {
            return Ok(PostAction::Continue);
        }
        let monitor = self.monitor.clone();
        for event in monitor {
            debug!(
                self.logger,
                "Udev event: type={}, devnum={:?} devnode={:?}",
                event.event_type(),
                event.devnum(),
                event.devnode()
            );
            match event.event_type() {
                // New device
                EventType::Add => {
                    if let (Some(path), Some(devnum)) = (event.devnode(), event.devnum()) {
                        info!(self.logger, "New device: #{} at {}", devnum, path.display());
                        if self.devices.insert(devnum, path.to_path_buf()).is_none() {
                            callback(
                                UdevEvent::Added {
                                    device_id: devnum,
                                    path: path.to_path_buf(),
                                },
                                &mut (),
                            );
                        }
                    }
                }
                // Device removed
                EventType::Remove => {
                    if let Some(devnum) = event.devnum() {
                        info!(self.logger, "Device removed: #{}", devnum);
                        if self.devices.remove(&devnum).is_some() {
                            callback(UdevEvent::Removed { device_id: devnum }, &mut ());
                        }
                    }
                }
                // New connector
                EventType::Change => {
                    if let Some(devnum) = event.devnum() {
                        info!(self.logger, "Device changed: #{}", devnum);
                        if self.devices.contains_key(&devnum) {
                            callback(UdevEvent::Changed { device_id: devnum }, &mut ());
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.token = factory.token();
        poll.register(self.as_raw_fd(), Interest::READ, Mode::Level, self.token)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.token = factory.token();
        poll.reregister(self.as_raw_fd(), Interest::READ, Mode::Level, self.token)
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        self.token = Token::invalid();
        poll.unregister(self.as_raw_fd())
    }
}

/// Events generated by the [`UdevBackend`], notifying you of changes in system devices
#[derive(Debug)]
pub enum UdevEvent {
    /// A new device has been detected
    Added {
        /// ID of the new device
        device_id: dev_t,
        /// Path of the new device
        path: PathBuf,
    },
    /// A device has changed
    Changed {
        /// ID of the changed device
        device_id: dev_t,
    },
    /// A device has been removed
    Removed {
        /// ID of the removed device
        device_id: dev_t,
    },
}

/// Returns the path of the primary GPU device if any
///
/// Might be used for filtering of [`UdevEvent::Added`] or for manual
/// [`DrmDevice`](crate::backend::drm::DrmDevice) initialization.
pub fn primary_gpu<S: AsRef<str>>(seat: S) -> IoResult<Option<PathBuf>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;

    if let Some(path) = enumerator
        .scan_devices()?
        .filter(|device| {
            let seat_name = device
                .property_value("ID_SEAT")
                .map(|x| x.to_os_string())
                .unwrap_or_else(|| OsString::from("seat0"));
            if seat_name == *seat.as_ref() {
                if let Ok(Some(pci)) = device.parent_with_subsystem(Path::new("pci")) {
                    if let Some(id) = pci.attribute_value("boot_vga") {
                        return id == "1";
                    }
                }
            }
            false
        })
        .flat_map(|device| device.devnode().map(PathBuf::from))
        .next()
    {
        Ok(Some(path))
    } else {
        all_gpus(seat).map(|all| all.into_iter().next())
    }
}

/// Returns the paths of all available GPU devices
///
/// Might be used for manual  [`DrmDevice`](crate::backend::drm::DrmDevice)
/// initialization.
pub fn all_gpus<S: AsRef<str>>(seat: S) -> IoResult<Vec<PathBuf>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;
    Ok(enumerator
        .scan_devices()?
        .filter(|device| {
            device
                .property_value("ID_SEAT")
                .map(|x| x.to_os_string())
                .unwrap_or_else(|| OsString::from("seat0"))
                == *seat.as_ref()
        })
        .flat_map(|device| device.devnode().map(PathBuf::from))
        .collect())
}

/// Returns the loaded driver for a device named by it's [`dev_t`](::nix::sys::stat::dev_t).
pub fn driver(dev: dev_t) -> IoResult<Option<OsString>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;
    Ok(enumerator
        .scan_devices()?
        .filter(|device| device.devnum() == Some(dev))
        .flat_map(|dev| {
            let mut device = Some(dev);
            while let Some(dev) = device {
                if dev.driver().is_some() {
                    return dev.driver().map(std::ffi::OsStr::to_os_string);
                }
                device = dev.parent();
            }
            None
        })
        .next())
}
