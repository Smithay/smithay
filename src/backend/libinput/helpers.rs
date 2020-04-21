use crate::backend::input::{self as backend};
use input as libinput;
use input::event::{
    device::DeviceEvent, keyboard::KeyboardEvent, pointer::PointerEvent, touch::TouchEvent, EventTrait,
};
use slog::Logger;

use super::LibinputInputBackend;
use std::{
    collections::hash_map::{DefaultHasher, Entry, HashMap},
    hash::{Hash, Hasher},
};

#[inline(always)]
pub fn on_device_event<H>(
    handler: &mut Option<H>,
    seats: &mut HashMap<libinput::Seat, backend::Seat>,
    devices: &mut Vec<libinput::Device>,
    event: DeviceEvent,
    logger: &Logger,
) where
    H: backend::InputHandler<LibinputInputBackend>,
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
            devices.push(added);

            match seats.entry(device_seat.clone()) {
                Entry::Occupied(mut seat_entry) => {
                    let old_seat = seat_entry.get_mut();
                    {
                        let caps = old_seat.capabilities_mut();
                        caps.pointer = new_caps.pointer || caps.pointer;
                        caps.keyboard = new_caps.keyboard || caps.keyboard;
                        caps.touch = new_caps.touch || caps.touch;
                    }
                    if let Some(ref mut handler) = handler {
                        trace!(logger, "Calling on_seat_changed with {:?}", old_seat);
                        handler.on_seat_changed(old_seat);
                    }
                }
                Entry::Vacant(seat_entry) => {
                    let mut hasher = DefaultHasher::default();
                    seat_entry.key().hash(&mut hasher);
                    let seat = seat_entry.insert(backend::Seat::new(
                        hasher.finish(),
                        format!("{}:{}", device_seat.physical_name(), device_seat.logical_name()),
                        new_caps,
                    ));
                    if let Some(ref mut handler) = handler {
                        trace!(logger, "Calling on_seat_created with {:?}", seat);
                        handler.on_seat_created(seat);
                    }
                }
            }
        }
        DeviceEvent::Removed(device_removed_event) => {
            let removed = device_removed_event.device();

            // remove device
            devices.retain(|dev| *dev != removed);

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
                caps.pointer = devices
                    .iter()
                    .filter(|x| x.seat() == device_seat)
                    .any(|x| x.has_capability(libinput::DeviceCapability::Pointer));
                caps.keyboard = devices
                    .iter()
                    .filter(|x| x.seat() == device_seat)
                    .any(|x| x.has_capability(libinput::DeviceCapability::Keyboard));
                caps.touch = devices
                    .iter()
                    .filter(|x| x.seat() == device_seat)
                    .any(|x| x.has_capability(libinput::DeviceCapability::Touch));
            } else {
                warn!(logger, "Seat changed that was never created");
                return;
            }

            // check if the seat has any other devices
            if !devices.iter().any(|x| x.seat() == device_seat) {
                // it has not, lets destroy it
                if let Some(seat) = seats.remove(&device_seat) {
                    info!(
                        logger,
                        "Removing seat {} which no longer has any device",
                        device_seat.logical_name()
                    );
                    if let Some(ref mut handler) = handler {
                        trace!(logger, "Calling on_seat_destroyed with {:?}", seat);
                        handler.on_seat_destroyed(&seat);
                    }
                } else {
                    warn!(logger, "Seat destroyed that was never created");
                    return;
                }
            // it has, notify about updates
            } else if let Some(ref mut handler) = handler {
                if let Some(seat) = seats.get(&device_seat) {
                    trace!(logger, "Calling on_seat_changed with {:?}", seat);
                    handler.on_seat_changed(&seat);
                } else {
                    warn!(logger, "Seat changed that was never created");
                    return;
                }
            }
        }
    }
    if let Some(ref mut handler) = handler {
        handler.on_input_config_changed(devices);
    }
}

#[inline(always)]
pub fn on_touch_event<H>(
    handler: &mut Option<H>,
    seats: &HashMap<libinput::Seat, backend::Seat>,
    event: TouchEvent,
    logger: &Logger,
) where
    H: backend::InputHandler<LibinputInputBackend>,
{
    if let Some(ref mut handler) = handler {
        let device_seat = event.device().seat();
        if let Some(ref seat) = seats.get(&device_seat) {
            match event {
                TouchEvent::Down(down_event) => {
                    trace!(logger, "Calling on_touch_down with {:?}", down_event);
                    handler.on_touch_down(seat, down_event)
                }
                TouchEvent::Motion(motion_event) => {
                    trace!(logger, "Calling on_touch_motion with {:?}", motion_event);
                    handler.on_touch_motion(seat, motion_event)
                }
                TouchEvent::Up(up_event) => {
                    trace!(logger, "Calling on_touch_up with {:?}", up_event);
                    handler.on_touch_up(seat, up_event)
                }
                TouchEvent::Cancel(cancel_event) => {
                    trace!(logger, "Calling on_touch_cancel with {:?}", cancel_event);
                    handler.on_touch_cancel(seat, cancel_event)
                }
                TouchEvent::Frame(frame_event) => {
                    trace!(logger, "Calling on_touch_frame with {:?}", frame_event);
                    handler.on_touch_frame(seat, frame_event)
                }
            }
        } else {
            warn!(logger, "Received touch event of non existing Seat");
            return;
        }
    }
}

#[inline(always)]
pub fn on_keyboard_event<H>(
    handler: &mut Option<H>,
    seats: &HashMap<libinput::Seat, backend::Seat>,
    event: KeyboardEvent,
    logger: &Logger,
) where
    H: backend::InputHandler<LibinputInputBackend>,
{
    match event {
        KeyboardEvent::Key(key_event) => {
            if let Some(ref mut handler) = handler {
                let device_seat = key_event.device().seat();
                if let Some(ref seat) = seats.get(&device_seat) {
                    trace!(logger, "Calling on_keyboard_key with {:?}", key_event);
                    handler.on_keyboard_key(seat, key_event);
                } else {
                    warn!(logger, "Received key event of non existing Seat");
                    return;
                }
            }
        }
    }
}

#[inline(always)]
pub fn on_pointer_event<H>(
    handler: &mut Option<H>,
    seats: &HashMap<libinput::Seat, backend::Seat>,
    event: PointerEvent,
    logger: &Logger,
) where
    H: backend::InputHandler<LibinputInputBackend>,
{
    if let Some(ref mut handler) = handler {
        let device_seat = event.device().seat();
        if let Some(ref seat) = seats.get(&device_seat) {
            match event {
                PointerEvent::Motion(motion_event) => {
                    trace!(logger, "Calling on_pointer_move with {:?}", motion_event);
                    handler.on_pointer_move(seat, motion_event);
                }
                PointerEvent::MotionAbsolute(motion_abs_event) => {
                    trace!(
                        logger,
                        "Calling on_pointer_move_absolute with {:?}",
                        motion_abs_event
                    );
                    handler.on_pointer_move_absolute(seat, motion_abs_event);
                }
                PointerEvent::Axis(axis_event) => {
                    trace!(logger, "Calling on_pointer_axis with {:?}", axis_event);
                    handler.on_pointer_axis(seat, axis_event);
                }
                PointerEvent::Button(button_event) => {
                    trace!(logger, "Calling on_pointer_button with {:?}", button_event);
                    handler.on_pointer_button(seat, button_event);
                }
            }
        } else {
            warn!(logger, "Received pointer event of non existing Seat");
        }
    }
}
