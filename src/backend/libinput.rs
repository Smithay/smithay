//! Implementation of input backend trait for types provided by `libinput`

use backend::SeatInternal;
use backend::input::{InputBackend, InputHandler, Seat, SeatCapabilities, MouseButton};
use input::{Libinput, Device, Seat as LibinputSeat, DeviceCapability};
use input::event::*;

use std::io::Error as IoError;
use std::collections::hash_map::{DefaultHasher, Entry, HashMap};
use std::hash::{Hash, Hasher};

struct SeatDesc {
    seat: Seat,
    pointer: (u32, u32),
}

pub struct LibinputInputBackend {
    context: Libinput,
    devices: Vec<Device>,
    seats: HashMap<LibinputSeat, SeatDesc>,
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
        for desc in self.seats.values() {
            trace!(self.logger, "Calling on_seat_created with {:?}", desc.seat);
            handler.on_seat_created(&desc.seat);
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
            for desc in self.seats.values() {
                trace!(self.logger, "Calling on_seat_destroyed with {:?}", desc.seat);
                handler.on_seat_destroyed(&desc.seat);
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
                                        let caps = old_seat.seat.capabilities_mut();
                                        caps.pointer = new_caps.pointer || caps.pointer;
                                        caps.keyboard = new_caps.keyboard || caps.keyboard;
                                        caps.touch = new_caps.touch || caps.touch;
                                    }
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_changed with {:?}", old_seat.seat);
                                        handler.on_seat_changed(&old_seat.seat);
                                    }
                                },
                                Entry::Vacant(seat_entry) => {
                                    let mut hasher = DefaultHasher::default();
                                    seat_entry.key().hash(&mut hasher);
                                    let desc = seat_entry.insert(SeatDesc {
                                        seat: Seat::new(hasher.finish(), new_caps),
                                        pointer: (0, 0) //FIXME: What position to assume? Maybe center of the screen instead. Probably call `set_cursor_position` after this.
                                    });
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_created with {:?}", desc.seat);
                                        handler.on_seat_created(&desc.seat);
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
                            if let Some(desc) = self.seats.get_mut(&device_seat) {
                                let caps = desc.seat.capabilities_mut();
                                caps.pointer = self.devices.iter().filter(|x| x.seat() == device_seat).any(|x| x.has_capability(DeviceCapability::Pointer));
                                caps.keyboard = self.devices.iter().filter(|x| x.seat() == device_seat).any(|x| x.has_capability(DeviceCapability::Keyboard));
                                caps.touch = self.devices.iter().filter(|x| x.seat() == device_seat).any(|x| x.has_capability(DeviceCapability::Touch));
                            } else {
                                panic!("Seat changed that was never created")
                            }

                            // check if the seat has any other devices
                            if !self.devices.iter().any(|x| x.seat() == device_seat) {
                                // it has not, lets destroy it
                                if let Some(desc) = self.seats.remove(&device_seat) {
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_destroyed with {:?}", desc.seat);
                                        handler.on_seat_destroyed(&desc.seat);
                                    }
                                } else {
                                    panic!("Seat destroyed that was never created");
                                }
                            } else {
                                // it has, notify about updates
                                if let Some(ref mut handler) = self.handler {
                                    let desc = self.seats.get(&device_seat).unwrap();
                                    trace!(self.logger, "Calling on_seat_changed with {:?}", desc.seat);
                                    handler.on_seat_changed(&desc.seat);
                                }
                            }
                        },
                    }
                    if let Some(ref mut handler) = self.handler {
                       handler.on_input_config_changed(&mut self.devices);
                    }
                },
                Event::Touch(touch_event) => {
                    use ::input::event::touch::*;
                    if let Some(ref mut handler) = self.handler {
                        let device_seat = touch_event.device().seat();
                        handler.on_touch(&self.seats.get(&device_seat).expect("Recieved key event of non existing Seat").seat,
                                         touch_event.time(), touch_event.into())
                    }
                },
                Event::Keyboard(keyboard_event) => {
                    use ::input::event::keyboard::*;
                    match keyboard_event {
                        KeyboardEvent::Key(event) => {
                            if let Some(ref mut handler) = self.handler {
                                let device_seat = event.device().seat();
                                handler.on_keyboard_key(&self.seats.get(&device_seat).expect("Recieved key event of non existing Seat").seat,
                                                        event.time(), event.key(), event.key_state().into(), event.seat_key_count());
                            }
                        }
                    }
                },
                Event::Pointer(pointer_event) => {
                    use ::input::event::pointer::*;
                    match pointer_event {
                        PointerEvent::Motion(motion_event) => {
                            let device_seat = motion_event.device().seat();
                            let desc = self.seats.get_mut(&device_seat).expect("Recieved pointer event of non existing Seat");
                            desc.pointer.0 += motion_event.dx() as u32;
                            desc.pointer.1 += motion_event.dy() as u32;
                            if let Some(ref mut handler) = self.handler {
                                handler.on_pointer_move(&desc.seat, motion_event.time(), desc.pointer);
                            }
                        },
                        PointerEvent::MotionAbsolute(motion_event) => {
                            let device_seat = motion_event.device().seat();
                            let desc = self.seats.get_mut(&device_seat).expect("Recieved pointer event of non existing Seat");
                            desc.pointer = (
                                motion_event.absolute_x_transformed(
                                /*FIXME: global.get_focused_output_for_seat(&desc.seat).width() or something like that*/ 1280) as u32,
                                motion_event.absolute_y_transformed(
                                /*FIXME: global.get_focused_output_for_seat(&desc.seat).height() or something like that*/ 800) as u32,
                            );
                            if let Some(ref mut handler) = self.handler {
                                handler.on_pointer_move(&desc.seat, motion_event.time(), desc.pointer);
                            }
                        },
                        PointerEvent::Axis(axis_event) => {
                            if let Some(ref mut handler) = self.handler {
                                let device_seat = axis_event.device().seat();
                                let desc = self.seats.get_mut(&device_seat).expect("Recieved pointer event of non existing Seat");
                                if axis_event.has_axis(Axis::Vertical) {
                                    handler.on_pointer_scroll(&desc.seat, axis_event.time(), ::backend::input::Axis::Vertical,
                                                              axis_event.axis_source().into(), match axis_event.axis_source() {
                                                                  AxisSource::Finger | AxisSource::Continuous => axis_event.axis_value(Axis::Vertical),
                                                                  AxisSource::Wheel | AxisSource::WheelTilt => axis_event.axis_value_discrete(Axis::Vertical).unwrap(),
                                                              });
                                }
                                if axis_event.has_axis(Axis::Horizontal) {
                                    handler.on_pointer_scroll(&desc.seat, axis_event.time(), ::backend::input::Axis::Horizontal,
                                                              axis_event.axis_source().into(), match axis_event.axis_source() {
                                                                  AxisSource::Finger | AxisSource::Continuous => axis_event.axis_value(Axis::Horizontal),
                                                                  AxisSource::Wheel | AxisSource::WheelTilt => axis_event.axis_value_discrete(Axis::Horizontal).unwrap(),
                                                              });
                                }
                            }
                        },
                        PointerEvent::Button(button_event) => {
                            if let Some(ref mut handler) = self.handler {
                                let device_seat = button_event.device().seat();
                                let desc = self.seats.get_mut(&device_seat).expect("Recieved pointer event of non existing Seat");
                                handler.on_pointer_button(&desc.seat, button_event.time(), match button_event.button() {
                                    0x110 => MouseButton::Left,
                                    0x111 => MouseButton::Right,
                                    0x112 => MouseButton::Middle,
                                    x => MouseButton::Other(x as u8),
                                }, button_event.button_state().into());
                            }
                        }
                    }
                },
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

impl From<::input::event::pointer::AxisSource> for ::backend::input::AxisSource {
    fn from(libinput: ::input::event::pointer::AxisSource) -> Self {
        match libinput {
            ::input::event::pointer::AxisSource::Finger => ::backend::input::AxisSource::Finger,
            ::input::event::pointer::AxisSource::Continuous => ::backend::input::AxisSource::Continuous,
            ::input::event::pointer::AxisSource::Wheel => ::backend::input::AxisSource::Wheel,
            ::input::event::pointer::AxisSource::WheelTilt => ::backend::input::AxisSource::WheelTilt,
        }
    }
}

impl From<::input::event::pointer::ButtonState> for ::backend::input::MouseButtonState {
    fn from(libinput: ::input::event::pointer::ButtonState) -> Self {
        match libinput {
            ::input::event::pointer::ButtonState::Pressed => ::backend::input::MouseButtonState::Pressed,
            ::input::event::pointer::ButtonState::Released => ::backend::input::MouseButtonState::Released,
        }
    }
}

impl From<::input::event::touch::TouchEvent> for ::backend::input::TouchEvent {
    fn from(libinput: ::input::event::touch::TouchEvent) -> Self {
        use ::input::event::touch::{TouchEventSlot, TouchEventPosition};
        use ::backend::TouchSlotInternal;

        match libinput {
            ::input::event::touch::TouchEvent::Down(down_event) => ::backend::input::TouchEvent::Down {
                slot: down_event.slot().map(|x| ::backend::input::TouchSlot::new(x)),
                x: down_event.x_transformed(/*FIXME: global.get_focused_output_for_seat(&desc.seat).width() or something like that*/ 1280),
                y: down_event.x_transformed(/*FIXME: global.get_focused_output_for_seat(&desc.seat).height() or something like that*/ 800),
            },
            ::input::event::touch::TouchEvent::Motion(motion_event) => ::backend::input::TouchEvent::Motion {
                slot: motion_event.slot().map(|x| ::backend::input::TouchSlot::new(x)),
                x: motion_event.x_transformed(/*FIXME: global.get_focused_output_for_seat(&desc.seat).width() or something like that*/ 1280),
                y: motion_event.x_transformed(/*FIXME: global.get_focused_output_for_seat(&desc.seat).height() or something like that*/ 800),
            },
            ::input::event::touch::TouchEvent::Up(up_event) => ::backend::input::TouchEvent::Up {
                slot: up_event.slot().map(|x| ::backend::input::TouchSlot::new(x)),
            },
            ::input::event::touch::TouchEvent::Cancel(cancel_event) => ::backend::input::TouchEvent::Cancel {
                slot: cancel_event.slot().map(|x| ::backend::input::TouchSlot::new(x)),
            },
            ::input::event::touch::TouchEvent::Frame(_) => ::backend::input::TouchEvent::Frame,
        }
    }
}
