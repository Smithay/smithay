//!
//! Provides `udev` related functionality for automated device scanning.
//!
//! This module mainly provides the `UdevBackend`, which constantly monitors available DRM devices
//! and notifies a user supplied `UdevHandler` of any changes.
//!
//! Additionally this contains some utility functions related to scanning.
//!
//! See also `examples/udev.rs` for pure hardware backed example of a compositor utilizing this
//! backend.

use nix::sys::stat::{dev_t, stat};
use std::{
    collections::HashSet,
    ffi::OsString,
    os::unix::io::{AsRawFd, RawFd},
    path::{Path, PathBuf},
};
use udev::{Context, Enumerator, EventType, MonitorBuilder, MonitorSocket, Result as UdevResult};

use wayland_server::calloop::{
    generic::{EventedFd, Generic},
    mio::Ready,
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
    /// Creates a new `UdevBackend` and adds it to the given `EventLoop`'s state.
    ///
    /// ## Arguments
    /// `context` - An initialized udev context
    /// `handler` - User-provided handler to respond to any detected changes
    /// `seat`    -
    /// `logger`  - slog Logger to be used by the backend and its `DrmDevices`.
    pub fn new<L, S: AsRef<str>>(
        context: &Context,
        mut handler: T,
        seat: S,
        logger: L,
    ) -> UdevResult<UdevBackend<T>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_udev"));

        let devices = all_gpus(context, seat)?
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
            }).collect();

        let mut builder = MonitorBuilder::new(context)?;
        builder.match_subsystem("drm")?;
        let monitor = builder.listen()?;

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

/// Binds a `UdevBackend` to a given `EventLoop`.
///
/// Allows the backend to receive kernel events and thus to drive the `UdevHandler`.
/// No runtime functionality can be provided without using this function.
pub fn udev_backend_bind<T: UdevHandler + 'static, Data: 'static>(
    handle: &LoopHandle<Data>,
    udev: UdevBackend<T>,
) -> Result<Source<Generic<EventedFd<UdevBackend<T>>>>, InsertError<Generic<EventedFd<UdevBackend<T>>>>> {
    let mut source = Generic::from_fd_source(udev);
    source.set_interest(Ready::readable());

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

/// Handler for the `UdevBackend`, allows to open, close and update drm devices as they change during runtime.
pub trait UdevHandler {
    /// Called when a new device is detected.
    fn device_added(&mut self, device: dev_t, path: PathBuf);
    /// Called when an open device is changed.
    ///
    /// This usually indicates that some connectors did become available or were unplugged. The handler
    /// should scan again for connected monitors and mode switch accordingly.
    fn device_changed(&mut self, device: dev_t);
    /// Called when a device was removed.
    ///
    /// The corresponding `UdevRawFd` will never return a valid `RawFd` anymore
    /// and its file descriptor will be closed once this function returns,
    /// any open references/tokens to this device need to be released.
    fn device_removed(&mut self, device: dev_t);
}

/// Returns the path of the primary GPU device if any
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
            .unwrap_or(OsString::from("seat0"))
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
                .unwrap_or(OsString::from("seat0"))
                == *seat.as_ref()
        }).flat_map(|device| device.devnode().map(PathBuf::from))
        .collect())
}
