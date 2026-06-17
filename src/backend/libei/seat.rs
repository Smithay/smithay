use reis::{eis::device::DeviceType, enumflags2::BitFlags, request::DeviceCapability};
use std::sync::{Arc, Mutex, Weak};
use xkbcommon::xkb;

use crate::{
    backend::input::InputEvent,
    input::keyboard::{KeymapFile, XkbConfig},
    utils::{Logical, Rectangle},
};

use super::{EiInput, EiInputConnection, EiInputConnectionInner};

/// A region advertised on an EI absolute pointer device.
///
/// Describes a rectangular coordinate space the client may point within, in the
/// compositor's logical coordinate system. Clients that capture at a different
/// (e.g. physical) resolution use `scale` to convert their coordinates into the
/// logical space the compositor expects.
///
/// `rect` is the logical offset and size of the region; `scale` the
/// physical-to-logical scale of the underlying output (e.g. `2.0` for a 200%
/// display). `mapping_id`, when set, identifies which screen/stream the region
/// maps to (it must match the `mapping_id` the client sees for the corresponding
/// screencast stream), letting a client correlate a region with the video it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct EiRegion {
    /// Logical offset and size of the region.
    pub rect: Rectangle<i32, Logical>,
    /// Physical-to-logical scale of the region (e.g. `2.0` at 200%).
    pub scale: f32,
    /// Identifier tying this region to a screencast stream (e.g. the output name).
    pub mapping_id: Option<String>,
}

/// A seat advertised on an EI sender context
#[derive(Clone, Debug)]
pub struct EiInputSeat(Arc<Mutex<EiInputSeatInner>>);

impl PartialEq<reis::request::Seat> for EiInputSeat {
    fn eq(&self, other: &reis::request::Seat) -> bool {
        self.0.lock().unwrap().seat == *other
    }
}

impl EiInputSeat {
    pub(super) fn new(
        connection: &EiInputConnection,
        seat: reis::request::Seat,
        event_sender: calloop::channel::Sender<InputEvent<EiInput>>,
    ) -> Self {
        Self(Arc::new(Mutex::new(EiInputSeatInner {
            connection: Arc::downgrade(&connection.0),
            seat,
            event_sender,
            keyboard: None,
            pointer: None,
            pointer_absolute: None,
            pointer_absolute_regions: Vec::new(),
            touch: None,
            text: None,
            device_keyboard: None,
            device_pointer: None,
            device_pointer_absolute: None,
            device_touch: None,
            device_text: None,
            bound_capabilities: BitFlags::empty(),
        })))
    }

    pub(super) fn bind(&self, capabilities: BitFlags<DeviceCapability>) {
        let mut inner = self.0.lock().unwrap();
        inner.bound_capabilities = capabilities;
        inner.refresh_devices();
    }

    /// Add a keyboard device to the EI seat
    ///
    /// Calling on a seat that already has a keyboard device will remove
    /// that device and add a new one.
    pub fn add_keyboard(
        &self,
        name: &str,
        xkb_config: XkbConfig<'_>,
    ) -> Result<(), crate::input::keyboard::Error> {
        let mut inner = self.0.lock().unwrap();
        inner.device_keyboard = None;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb_config.compile_keymap(&context).map_err(|_| {
            tracing::debug!("Loading keymap from XkbConfig failed");
            crate::input::keyboard::Error::BadKeymap
        })?;
        let keymap_file = KeymapFile::new(&keymap);
        inner.keyboard = Some((name.to_string(), keymap_file));
        inner.refresh_devices();
        Ok(())
    }

    /// Remove keyboard device from the EI seat
    pub fn remove_keyboard(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.device_keyboard = None;
        inner.keyboard = None;
    }

    /// Send the compositor's current modifier state to this seat's keyboard device via
    /// `ei_keyboard.modifiers`, so the client keeps its keyboard state in sync with the
    /// compositor (mirroring `wl_keyboard.modifiers`). No-op if there is no bound keyboard.
    pub fn keyboard_modifiers(&self, depressed: u32, locked: u32, latched: u32, group: u32) {
        let inner = self.0.lock().unwrap();
        let Some(device) = inner.device_keyboard.as_ref() else {
            return;
        };
        let Some(keyboard) = device.device.interface::<reis::eis::Keyboard>() else {
            return;
        };
        let Some(connection) = inner.connection.upgrade() else {
            return;
        };
        connection.connection.with_next_serial(|serial| {
            keyboard.modifiers(serial, depressed, locked, latched, group);
        });
        let _ = connection.connection.flush();
    }

    /// Add a pointer device to the EI seat
    ///
    /// The EI device will have button and scroll capabilities in addition
    /// to the pointer capability.
    ///
    /// Calling on a seat that already has a pointer device will remove
    /// that device and add a new one.
    pub fn add_pointer(&self, name: &str) {
        let mut inner = self.0.lock().unwrap();
        inner.device_pointer = None;
        inner.pointer = Some(name.to_string());
        inner.refresh_devices();
    }

    /// Remove pointer device from the EI seat
    pub fn remove_pointer(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.device_pointer = None;
        inner.pointer = None;
    }

    /// Add an absolute pointer device to the EI seat
    ///
    /// The EI device will have button and scroll capabilities in addition
    /// to the pointer absolute capability.
    ///
    /// `regions` describes the logical coordinate space(s) the client may point
    /// within (see [`EiRegion`]). Pass an empty slice to advertise no region, in
    /// which case the client must guess the coordinate space. Advertising a
    /// region lets clients that capture at a different resolution (e.g. physical
    /// pixels on a HiDPI output) convert their coordinates correctly.
    ///
    /// Calling on a seat that already has an absolute pointer device will remove
    /// that device and add a new one.
    pub fn add_pointer_absolute(&self, name: &str, regions: &[EiRegion]) {
        let mut inner = self.0.lock().unwrap();
        inner.device_pointer_absolute = None;
        inner.pointer_absolute = Some(name.to_string());
        inner.pointer_absolute_regions = regions.to_vec();
        inner.refresh_devices();
    }

    /// Remove absolute pointer device from the EI seat
    pub fn remove_pointer_absolute(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.device_pointer_absolute = None;
        inner.pointer_absolute = None;
        inner.pointer_absolute_regions.clear();
    }

    /// Add a touch device to the EI seat
    ///
    /// Calling on a seat that already has a touch device will remove
    /// that device and add a new one.
    pub fn add_touch(&self, name: &str) {
        let mut inner = self.0.lock().unwrap();
        inner.device_touch = None;
        inner.touch = Some(name.to_string());
        inner.refresh_devices();
    }

    /// Remove touch device from the EI seat
    pub fn remove_touch(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.device_touch = None;
        inner.touch = None;
    }

    /// Add a text device to the EI seat.
    ///
    /// A text device lets clients inject input by keysym or UTF-8 string (`ei_text`), independent
    /// of any keymap. Calling on a seat that already has a text device will remove that device and
    /// add a new one.
    pub fn add_text(&self, name: &str) {
        let mut inner = self.0.lock().unwrap();
        inner.device_text = None;
        inner.text = Some(name.to_string());
        inner.refresh_devices();
    }

    /// Remove text device from the EI seat
    pub fn remove_text(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.device_text = None;
        inner.text = None;
    }

    /// Remove seat from EI connection
    pub fn remove(&self) {
        let inner = self.0.lock().unwrap();
        inner.seat.remove();
        if let Some(connection) = inner.connection.upgrade() {
            let mut seats = connection.seats.lock().unwrap();
            if let Some(idx) = seats.iter().position(|s| Arc::ptr_eq(&s.0, &self.0)) {
                seats.remove(idx);
            }
        }
    }
}

#[derive(Debug)]
struct EiInputSeatInner {
    connection: Weak<EiInputConnectionInner>,
    seat: reis::request::Seat,
    bound_capabilities: BitFlags<DeviceCapability>,
    event_sender: calloop::channel::Sender<InputEvent<EiInput>>,
    // Interfaces advertised by the server
    keyboard: Option<(String, KeymapFile)>,
    pointer: Option<String>,
    pointer_absolute: Option<String>,
    // Regions advertised on the absolute pointer device, describing the logical
    // coordinate space(s) the client may address (see `EiRegion`).
    pointer_absolute_regions: Vec<EiRegion>,
    touch: Option<String>,
    text: Option<String>,
    // Devices created in response to client bind
    device_keyboard: Option<DeviceDropWrapper>,
    device_pointer: Option<DeviceDropWrapper>,
    device_pointer_absolute: Option<DeviceDropWrapper>,
    device_touch: Option<DeviceDropWrapper>,
    device_text: Option<DeviceDropWrapper>,
}

impl EiInputSeatInner {
    // Add any devices the server provides, for capabilities the client has bound, if no device yet
    fn refresh_devices(&mut self) {
        if self.device_keyboard.is_none() && self.bound_capabilities.contains(DeviceCapability::Keyboard) {
            if let Some((name, keymap_file)) = self.keyboard.as_ref() {
                let device = self.seat.add_device(
                    Some(name),
                    DeviceType::Virtual,
                    DeviceCapability::Keyboard.into(),
                    |device| {
                        let keyboard = device.interface::<reis::eis::Keyboard>().unwrap();
                        let _ = keymap_file.with_fd(true, |fd, len| {
                            keyboard.keymap(reis::eis::keyboard::KeymapType::Xkb, len as u32, fd);
                        });
                    },
                );
                device.resumed();
                let _ = self.event_sender.send(InputEvent::DeviceAdded {
                    device: device.clone(),
                });
                self.device_keyboard = Some(DeviceDropWrapper::new(device, &self.event_sender));
            }
        }

        if self.device_pointer.is_none() && self.bound_capabilities.contains(DeviceCapability::Pointer) {
            if let Some(name) = self.pointer.as_ref() {
                let device = self.seat.add_device(
                    Some(name),
                    DeviceType::Virtual,
                    DeviceCapability::Pointer | DeviceCapability::Button | DeviceCapability::Scroll,
                    |_| {},
                );
                device.resumed();
                let _ = self.event_sender.send(InputEvent::DeviceAdded {
                    device: device.clone(),
                });
                self.device_pointer = Some(DeviceDropWrapper::new(device, &self.event_sender));
            }
        }

        if self.device_pointer_absolute.is_none()
            && self
                .bound_capabilities
                .contains(DeviceCapability::PointerAbsolute)
        {
            if let Some(name) = self.pointer_absolute.as_ref() {
                let regions = self.pointer_absolute_regions.clone();
                let device = self.seat.add_device(
                    Some(name),
                    DeviceType::Virtual,
                    DeviceCapability::PointerAbsolute | DeviceCapability::Button | DeviceCapability::Scroll,
                    |device| {
                        // Advertise the coordinate space(s) the client may point within.
                        // These `ei_device.region` events must be sent after the device is
                        // created but before `done`, which is exactly when this closure runs.
                        for region in &regions {
                            // `region_mapping_id` applies to the region created by the
                            // next `region` request, so send it first when present.
                            if let Some(mapping_id) = &region.mapping_id {
                                device.device().region_mapping_id(mapping_id);
                            }
                            // `ei_device.region` coordinates are unsigned
                            device.device().region(
                                region.rect.loc.x.max(0) as u32,
                                region.rect.loc.y.max(0) as u32,
                                region.rect.size.w.max(0) as u32,
                                region.rect.size.h.max(0) as u32,
                                region.scale,
                            );
                        }
                    },
                );
                device.resumed();
                let _ = self.event_sender.send(InputEvent::DeviceAdded {
                    device: device.clone(),
                });
                self.device_pointer_absolute = Some(DeviceDropWrapper::new(device, &self.event_sender));
            }
        }

        if self.device_touch.is_none() && self.bound_capabilities.contains(DeviceCapability::Touch) {
            if let Some(name) = self.touch.as_ref() {
                let device = self.seat.add_device(
                    Some(name),
                    DeviceType::Virtual,
                    DeviceCapability::Touch.into(),
                    |_| {},
                );
                device.resumed();
                let _ = self.event_sender.send(InputEvent::DeviceAdded {
                    device: device.clone(),
                });
                self.device_touch = Some(DeviceDropWrapper::new(device, &self.event_sender));
            }
        }

        if self.device_text.is_none() && self.bound_capabilities.contains(DeviceCapability::Text) {
            if let Some(name) = self.text.as_ref() {
                let device = self.seat.add_device(
                    Some(name),
                    DeviceType::Virtual,
                    DeviceCapability::Text.into(),
                    |_| {},
                );
                device.resumed();
                let _ = self.event_sender.send(InputEvent::DeviceAdded {
                    device: device.clone(),
                });
                self.device_text = Some(DeviceDropWrapper::new(device, &self.event_sender));
            }
        }
    }
}

// Helper that remove the device on drop, and send `DeviceRemoved`
#[derive(Debug)]
struct DeviceDropWrapper {
    device: reis::request::Device,
    event_sender: calloop::channel::Sender<InputEvent<EiInput>>,
}

impl DeviceDropWrapper {
    fn new(
        device: reis::request::Device,
        event_sender: &calloop::channel::Sender<InputEvent<EiInput>>,
    ) -> Self {
        Self {
            device,
            event_sender: event_sender.clone(),
        }
    }
}

impl Drop for DeviceDropWrapper {
    fn drop(&mut self) {
        let _ = self.event_sender.send(InputEvent::DeviceRemoved {
            device: self.device.clone(),
        });
        self.device.remove();
    }
}
