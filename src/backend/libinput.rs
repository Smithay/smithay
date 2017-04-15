//! Implementation of input backend trait for types provided by `libinput`

use backend::SeatInternal;
use backend::input::{InputBackend, InputHandler, Seat, SeatCapabilities};
use input::{Libinput, Device, Seat as LibinputSeat, DeviceCapability};
use input::event::*;

use std::io::Error as IoError;
use std::collections::hash_map::{DefaultHasher, Entry, HashMap};
use std::hash::{Hash, Hasher};

pub struct LibinputInputBackend {
    context: Libinput,
    devices: Vec<Device>,
    seats: HashMap<LibinputSeat, Seat>,
    handler: Option<Box<InputHandler<LibinputInputBackend> + 'static>>,
    logger: ::slog::Logger,
}

impl InputBackend for LibinputInputBackend {
    type InputConfig = [Device];
    type EventError = IoError;

    fn set_handler<H: InputHandler<Self> + 'static>(&mut self, mut handler: H) {
        if self.handler.is_some() {
            self.clear_handler();
        }
        info!(self.logger, "New input handler set.");
        for seat in self.seats.values() {
            trace!(self.logger, "Calling on_seat_created with {:?}", seat);
            handler.on_seat_created(&seat);
        }
        self.handler = Some(Box::new(handler));
    }

    fn get_handler(&mut self) -> Option<&mut InputHandler<Self>> {
        self.handler
            .as_mut()
            .map(|handler| handler as &mut InputHandler<Self>)
    }

    fn clear_handler(&mut self) {
        if let Some(mut handler) = self.handler.take() {
            for seat in self.seats.values() {
                trace!(self.logger, "Calling on_seat_destroyed with {:?}", seat);
                handler.on_seat_destroyed(&seat);
            }
            info!(self.logger, "Removing input handler");
        }
    }

    fn input_config(&mut self) -> &mut Self::InputConfig {
        &mut self.devices
    }

    fn set_cursor_position(&mut self, _x: u32, _y: u32) -> Result<(), ()> {
        // FIXME later.
        // This will be doable with the hardware cursor api and probably some more cases
        warn!(self.logger, "Setting the cursor position is currently unsupported by the libinput backend");
        Err(())
    }

    fn dispatch_new_events(&mut self) -> Result<(), IoError> {
        self.context.dispatch()?;
        for event in &mut self.context {
            match event {
                Event::Device(device_event) => {
                    use input::event::device::*;
                    match device_event {
                        DeviceEvent::Added(device_added_event) => {
                            let added = device_added_event.into_event().device();

                            let new_caps = SeatCapabilities {
                                pointer: added.has_capability(DeviceCapability::Pointer),
                                keyboard: added.has_capability(DeviceCapability::Keyboard),
                                touch: added.has_capability(DeviceCapability::Touch),
                            };

                            let device_seat = added.seat();
                            self.devices.push(added);

                            match self.seats.entry(device_seat) {
                                Entry::Occupied(mut seat_entry) => {
                                    let old_seat = seat_entry.get_mut();
                                    {
                                        let caps = old_seat.capabilities_mut();
                                        caps.pointer = new_caps.pointer || caps.pointer;
                                        caps.keyboard = new_caps.keyboard || caps.keyboard;
                                        caps.touch = new_caps.touch || caps.touch;
                                    }
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_changed with {:?}", old_seat);
                                        handler.on_seat_changed(old_seat);
                                    }
                                },
                                Entry::Vacant(seat_entry) => {
                                    let mut hasher = DefaultHasher::default();
                                    seat_entry.key().hash(&mut hasher);
                                    let seat = seat_entry.insert(Seat::new(hasher.finish(), new_caps));
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_created with {:?}", seat);
                                        handler.on_seat_created(seat);
                                    }
                                }
                            }
                        },
                        DeviceEvent::Removed(device_removed_event) => {
                            let removed = device_removed_event.into_event().device();

                            // remove device
                            self.devices.retain(|dev| *dev == removed);

                            let device_seat = removed.seat();

                            // update capabilities, so they appear correctly on `on_seat_changed` and `on_seat_destroyed`.
                            if let Some(seat) = self.seats.get_mut(&device_seat) {
                                let caps = seat.capabilities_mut();
                                caps.pointer = self.devices.iter().filter(|x| x.seat() == device_seat).any(|x| x.has_capability(DeviceCapability::Pointer));
                                caps.keyboard = self.devices.iter().filter(|x| x.seat() == device_seat).any(|x| x.has_capability(DeviceCapability::Keyboard));
                                caps.touch = self.devices.iter().filter(|x| x.seat() == device_seat).any(|x| x.has_capability(DeviceCapability::Touch));
                            } else {
                                panic!("Seat changed that was never created")
                            }

                            // check if the seat has any other devices
                            if !self.devices.iter().any(|x| x.seat() == device_seat) {
                                // it has not, lets destroy it
                                if let Some(seat) = self.seats.remove(&device_seat) {
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_destroyed with {:?}", seat);
                                        handler.on_seat_destroyed(&seat);
                                    }
                                } else {
                                    panic!("Seat destroyed that was never created");
                                }
                            } else {
                                // it has, notify about updates
                                if let Some(ref mut handler) = self.handler {
                                    let seat = self.seats.get(&device_seat).unwrap();
                                    trace!(self.logger, "Calling on_seat_changed with {:?}", seat);
                                    handler.on_seat_changed(seat);
                                }
                            }
                        },
                    }
                    if let Some(ref mut handler) = self.handler {
                       handler.on_input_config_changed(&mut self.devices);
                    }
                },
                Event::Touch(touch_event) => {},
                Event::Keyboard(keyboard_event) => {
                    use ::input::event::keyboard::*;
                    match keyboard_event {
                        KeyboardEvent::Key(event) => {
                            if let Some(ref mut handler) = self.handler {
                                let device_seat = event.device().seat();
                                handler.on_keyboard_key(self.seats.get(&device_seat).expect("Recieved key event of non existing Seat"),
                                                        event.time(), event.key(), event.key_state().into(), event.seat_key_count());
                            }
                        }
                    }
                },
                Event::Pointer(pointer_event) => {},
                _ => {}, //FIXME: What to do with the rest.
            }
        };
        Ok(())
    }
}

impl From<::input::event::keyboard::KeyState> for ::backend::input::KeyState {
    fn from(libinput: ::input::event::keyboard::KeyState) -> Self {
        match libinput {
            ::input::event::keyboard::KeyState::Pressed => ::backend::input::KeyState::Pressed,
            ::input::event::keyboard::KeyState::Released => ::backend::input::KeyState::Released,
        }
    }
}
