//!
//! Provides `udev` related functionality for automated device scanning.
//!
//! This module mainly provides the [`UdevBackend`](::backend::udev::UdevBackend), which constantly monitors available DRM devices
//! and notifies a user supplied [`UdevHandler`](::backend::udev::UdevHandler) of any changes.
//!
//! Additionally this contains some utility functions related to scanning.
//!
//! See also `anvil/src/udev.rs` for pure hardware backed example of a compositor utilizing this
//! backend.

use nix::sys::stat::{dev_t, stat};
use std::{
    collections::HashSet,
    ffi::OsString,
    os::unix::io::{AsRawFd, RawFd},
    path::{Path, PathBuf},
};
use udev::{Enumerator, EventType, MonitorBuilder, MonitorSocket};

use calloop::{
    generic::{Generic, SourceFd},
    mio::Interest,
    InsertError, LoopHandle, Source,
};

/// Backend to monitor available drm devices.
///
/// Provides a way to automatically scan for available gpus and notifies the
/// given handler of any changes. Can be used to provide hot-plug functionality for gpus and
/// attached monitors.
pub struct UdevBackend<T: UdevHandler + 'static> {
    devices: HashSet<dev_t>,
    monitor: MonitorSocket,
    handler: T,
    logger: ::slog::Logger,
}

impl<T: UdevHandler + 'static> AsRawFd for UdevBackend<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.monitor.as_raw_fd()
    }
}

impl<T: UdevHandler + 'static> UdevBackend<T> {
    /// Creates a new [`UdevBackend`]
    ///
    /// ## Arguments
    /// `handler` - User-provided handler to respond to any detected changes
    /// `seat`    -
    /// `logger`  - slog Logger to be used by the backend and its `DrmDevices`.
    pub fn new<L, S: AsRef<str>>(
        mut handler: T,
        seat: S,
        logger: L,
    ) -> ::std::io::Result<UdevBackend<T>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_udev"));

        let devices = all_gpus(seat)?
            .into_iter()
            // Create devices
            .flat_map(|path| match stat(&path) {
                Ok(stat) => {
                    handler.device_added(stat.st_rdev, path);
                    Some(stat.st_rdev)
                }
                Err(err) => {
                    warn!(log, "Unable to get id of {:?}, Error: {:?}. Skipping", path, err);
                    None
                }
            })
            .collect();

        let monitor = MonitorBuilder::new()?
            .match_subsystem("drm")?
            .listen()?;

        Ok(UdevBackend {
            devices,
            monitor,
            handler,
            logger: log,
        })
    }
}

impl<T: UdevHandler + 'static> Drop for UdevBackend<T> {
    fn drop(&mut self) {
        for device in &self.devices {
            self.handler.device_removed(*device);
        }
    }
}

/// Binds a [`UdevBackend`] to a given [`EventLoop`](calloop::EventLoop).
///
/// Allows the backend to receive kernel events and thus to drive the [`UdevHandler`].
/// No runtime functionality can be provided without using this function.
pub fn udev_backend_bind<T: UdevHandler + 'static, Data: 'static>(
    udev: UdevBackend<T>,
    handle: &LoopHandle<Data>,
) -> Result<Source<Generic<SourceFd<UdevBackend<T>>>>, InsertError<Generic<SourceFd<UdevBackend<T>>>>> {
    let mut source = Generic::from_fd_source(udev);
    source.set_interest(Interest::READABLE);

    handle.insert_source(source, |evt, _| {
        evt.source.borrow_mut().0.process_events();
    })
}

impl<T: UdevHandler + 'static> UdevBackend<T> {
    fn process_events(&mut self) {
        let monitor = self.monitor.clone();
        for event in monitor {
            match event.event_type() {
                // New device
                EventType::Add => {
                    info!(self.logger, "Device Added");
                    if let (Some(path), Some(devnum)) = (event.devnode(), event.devnum()) {
                        if self.devices.insert(devnum) {
                            self.handler.device_added(devnum, path.to_path_buf());
                        }
                    }
                }
                // Device removed
                EventType::Remove => {
                    info!(self.logger, "Device Remove");
                    if let Some(devnum) = event.devnum() {
                        if self.devices.remove(&devnum) {
                            self.handler.device_removed(devnum);
                        }
                    }
                }
                // New connector
                EventType::Change => {
                    info!(self.logger, "Device Changed");
                    if let Some(devnum) = event.devnum() {
                        info!(self.logger, "Devnum: {:b}", devnum);
                        if self.devices.contains(&devnum) {
                            self.handler.device_changed(devnum);
                        } else {
                            info!(self.logger, "changed, but device not tracked by backend");
                        };
                    } else {
                        info!(self.logger, "changed, but no devnum");
                    }
                }
                _ => {}
            }
        }
    }
}

/// Handler for the [`UdevBackend`], allows to open, close and update drm devices as they change during runtime.
pub trait UdevHandler {
    /// Called when a new device is detected.
    fn device_added(&mut self, device: dev_t, path: PathBuf);
    /// Called when an open device is changed.
    ///
    /// This usually indicates that some connectors did become available or were unplugged. The handler
    /// should scan again for connected monitors and mode switch accordingly.
    fn device_changed(&mut self, device: dev_t);
    /// Called when a device was removed.
    fn device_removed(&mut self, device: dev_t);
}

/// Returns the path of the primary GPU device if any
///
/// Might be used for filtering in [`UdevHandler::device_added`] or for manual
/// [`LegacyDrmDevice`](::backend::drm::legacy::LegacyDrmDevice) initialization.
pub fn primary_gpu<S: AsRef<str>>(seat: S) -> ::std::io::Result<Option<PathBuf>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;

    let mut result = None;
    for device in enumerator.scan_devices()? {
        if device
            .property_value("ID_SEAT")
            .map(|x| x.to_os_string())
            .unwrap_or_else(|| OsString::from("seat0"))
            == *seat.as_ref()
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

/// Returns the paths of all available GPU devices
///
/// Might be used for manual  [`LegacyDrmDevice`](::backend::drm::legacy::LegacyDrmDevice)
/// initialization.
pub fn all_gpus<S: AsRef<str>>(seat: S) -> ::std::io::Result<Vec<PathBuf>> {
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
