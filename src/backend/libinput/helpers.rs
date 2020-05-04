use crate::backend::input::{self as backend, InputEvent};
use input as libinput;
use input::event::{
    device::DeviceEvent, keyboard::KeyboardEvent, pointer::PointerEvent, touch::TouchEvent, EventTrait,
};
use slog::Logger;

use super::{LibinputConfig, LibinputEvent, LibinputInputBackend};
use std::{
    collections::hash_map::{DefaultHasher, Entry, HashMap},
    hash::{Hash, Hasher},
};

#[inline(always)]
pub fn on_device_event<F>(
    callback: &mut F,
    seats: &mut HashMap<libinput::Seat, backend::Seat>,
    config: &mut LibinputConfig,
    event: DeviceEvent,
    logger: &Logger,
) where
    F: FnMut(InputEvent<LibinputInputBackend>, &mut LibinputConfig),
{
    match event {
        DeviceEvent::Added(device_added_event) => {
            let added = device_added_event.device();

            let new_caps = backend::SeatCapabilities {
                pointer: added.has_capability(libinput::DeviceCapability::Pointer),
                keyboard: added.has_capability(libinput::DeviceCapability::Keyboard),
                touch: added.has_capability(libinput::DeviceCapability::Touch),
            };

            let device_seat = added.seat();
            info!(
                logger,
                "New device {:?} on seat {:?}",
                added.sysname(),
                device_seat.logical_name()
            );
            config.devices.push(added.clone());

            match seats.entry(device_seat.clone()) {
                Entry::Occupied(mut seat_entry) => {
                    let old_seat = seat_entry.get_mut();
                    {
                        let caps = old_seat.capabilities_mut();
                        caps.pointer = new_caps.pointer || caps.pointer;
                        caps.keyboard = new_caps.keyboard || caps.keyboard;
                        caps.touch = new_caps.touch || caps.touch;
                    }
                    callback(InputEvent::SeatChanged(old_seat.clone()), config);
                }
                Entry::Vacant(seat_entry) => {
                    let mut hasher = DefaultHasher::default();
                    seat_entry.key().hash(&mut hasher);
                    let seat = seat_entry.insert(backend::Seat::new(
                        hasher.finish(),
                        format!("{}:{}", device_seat.physical_name(), device_seat.logical_name()),
                        new_caps,
                    ));
                    callback(InputEvent::NewSeat(seat.clone()), config);
                }
            }

            callback(InputEvent::Special(LibinputEvent::NewDevice(added)), config);
        }
        DeviceEvent::Removed(device_removed_event) => {
            let removed = device_removed_event.device();

            // remove device
            config.devices.retain(|dev| *dev != removed);

            let device_seat = removed.seat();
            info!(
                logger,
                "Removed device {:?} on seat {:?}",
                removed.sysname(),
                device_seat.logical_name()
            );

            // update capabilities, so they appear correctly on `on_seat_changed` and `on_seat_destroyed`.
            if let Some(seat) = seats.get_mut(&device_seat) {
                let caps = seat.capabilities_mut();
                caps.pointer = config
                    .devices
                    .iter()
                    .filter(|x| x.seat() == device_seat)
                    .any(|x| x.has_capability(libinput::DeviceCapability::Pointer));
                caps.keyboard = config
                    .devices
                    .iter()
                    .filter(|x| x.seat() == device_seat)
                    .any(|x| x.has_capability(libinput::DeviceCapability::Keyboard));
                caps.touch = config
                    .devices
                    .iter()
                    .filter(|x| x.seat() == device_seat)
                    .any(|x| x.has_capability(libinput::DeviceCapability::Touch));
            } else {
                warn!(logger, "Seat changed that was never created");
                return;
            }

            // check if the seat has any other devices
            if !config.devices.iter().any(|x| x.seat() == device_seat) {
                // it has not, lets destroy it
                if let Some(seat) = seats.remove(&device_seat) {
                    info!(
                        logger,
                        "Removing seat {} which no longer has any device",
                        device_seat.logical_name()
                    );
                    callback(InputEvent::SeatRemoved(seat), config);
                } else {
                    warn!(logger, "Seat destroyed that was never created");
                    return;
                }
            // it has, notify about updates
            } else if let Some(seat) = seats.get(&device_seat) {
                callback(InputEvent::SeatChanged(seat.clone()), config);
            } else {
                warn!(logger, "Seat changed that was never created");
                return;
            }

            callback(InputEvent::Special(LibinputEvent::RemovedDevice(removed)), config);
        }
    }
}

#[inline(always)]
pub fn on_touch_event<F>(
    callback: &mut F,
    seats: &HashMap<libinput::Seat, backend::Seat>,
    config: &mut LibinputConfig,
    event: TouchEvent,
    logger: &Logger,
) where
    F: FnMut(InputEvent<LibinputInputBackend>, &mut LibinputConfig),
{
    let device_seat = event.device().seat();
    if let Some(seat) = seats.get(&device_seat).cloned() {
        match event {
            TouchEvent::Down(down_event) => {
                callback(
                    InputEvent::TouchDown {
                        seat,
                        event: down_event,
                    },
                    config,
                );
            }
            TouchEvent::Motion(motion_event) => {
                callback(
                    InputEvent::TouchMotion {
                        seat,
                        event: motion_event,
                    },
                    config,
                );
            }
            TouchEvent::Up(up_event) => {
                callback(
                    InputEvent::TouchUp {
                        seat,
                        event: up_event,
                    },
                    config,
                );
            }
            TouchEvent::Cancel(cancel_event) => {
                callback(
                    InputEvent::TouchCancel {
                        seat,
                        event: cancel_event,
                    },
                    config,
                );
            }
            TouchEvent::Frame(frame_event) => {
                callback(
                    InputEvent::TouchFrame {
                        seat,
                        event: frame_event,
                    },
                    config,
                );
            }
        }
    } else {
        warn!(logger, "Received touch event of non existing Seat");
    }
}

#[inline(always)]
pub fn on_keyboard_event<F>(
    callback: &mut F,
    seats: &HashMap<libinput::Seat, backend::Seat>,
    config: &mut LibinputConfig,
    event: KeyboardEvent,
    logger: &Logger,
) where
    F: FnMut(InputEvent<LibinputInputBackend>, &mut LibinputConfig),
{
    match event {
        KeyboardEvent::Key(key_event) => {
            let device_seat = key_event.device().seat();
            if let Some(seat) = seats.get(&device_seat).cloned() {
                callback(
                    InputEvent::Keyboard {
                        seat,
                        event: key_event,
                    },
                    config,
                );
            } else {
                warn!(logger, "Received key event of non existing Seat");
            }
        }
    }
}

#[inline(always)]
pub fn on_pointer_event<F>(
    callback: &mut F,
    seats: &HashMap<libinput::Seat, backend::Seat>,
    config: &mut LibinputConfig,
    event: PointerEvent,
    logger: &Logger,
) where
    F: FnMut(InputEvent<LibinputInputBackend>, &mut LibinputConfig),
{
    let device_seat = event.device().seat();
    if let Some(seat) = seats.get(&device_seat).cloned() {
        match event {
            PointerEvent::Motion(motion_event) => {
                callback(
                    InputEvent::PointerMotion {
                        seat,
                        event: motion_event,
                    },
                    config,
                );
            }
            PointerEvent::MotionAbsolute(motion_abs_event) => {
                callback(
                    InputEvent::PointerMotionAbsolute {
                        seat,
                        event: motion_abs_event,
                    },
                    config,
                );
            }
            PointerEvent::Axis(axis_event) => {
                callback(
                    InputEvent::PointerAxis {
                        seat,
                        event: axis_event,
                    },
                    config,
                );
            }
            PointerEvent::Button(button_event) => {
                callback(
                    InputEvent::PointerButton {
                        seat,
                        event: button_event,
                    },
                    config,
                );
            }
        }
    } else {
        warn!(logger, "Received pointer event of non existing Seat");
    }
}
