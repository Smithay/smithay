use std::{process::Command, sync::atomic::Ordering};

use crate::{shell::FullscreenSurface, state::Backend, AnvilState};

#[cfg(feature = "udev")]
use crate::udev::UdevData;

use smithay::{
    backend::input::{
        self, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
        PointerButtonEvent,
    },
    desktop::layer_map_for_output,
    reexports::wayland_server::protocol::{wl_pointer, wl_surface::WlSurface},
    wayland::{
        compositor::with_states,
        seat::{keysyms as xkb, AxisFrame, FilterResult, Keysym, ModifiersState},
        shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer, LayerSurfaceCachedState},
        Serial, SERIAL_COUNTER as SCOUNTER,
    },
};

#[cfg(any(feature = "winit", feature = "x11"))]
use smithay::{backend::input::PointerMotionAbsoluteEvent, wayland::output::Output};

#[cfg(feature = "udev")]
use smithay::{
    backend::{
        input::{
            Device, DeviceCapability, PointerMotionEvent, ProximityState, TabletToolButtonEvent,
            TabletToolEvent, TabletToolProximityEvent, TabletToolTipEvent, TabletToolTipState,
        },
        session::Session,
    },
    utils::{Logical, Point},
    wayland::tablet_manager::{TabletDescriptor, TabletSeatTrait},
};

impl<Backend> AnvilState<Backend> {
    fn process_common_key_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::None => (),

            KeyAction::Quit => {
                info!(self.log, "Quitting.");
                self.running.store(false, Ordering::SeqCst);
            }

            KeyAction::Run(cmd) => {
                info!(self.log, "Starting program"; "cmd" => cmd.clone());

                if let Err(e) = Command::new(&cmd).spawn() {
                    error!(self.log,
                        "Failed to start program";
                        "cmd" => cmd,
                        "err" => format!("{:?}", e)
                    );
                }
            }

            _ => unreachable!(
                "Common key action handler encountered backend specific action {:?}",
                action
            ),
        }
    }

    fn keyboard_key_to_action<B: InputBackend>(&mut self, evt: B::KeyboardKeyEvent) -> KeyAction {
        let keycode = evt.key_code();
        let state = evt.state();
        debug!(self.log, "key"; "keycode" => keycode, "state" => format!("{:?}", state));
        let serial = SCOUNTER.next_serial();
        let log = &self.log;
        let time = Event::time(&evt);
        let suppressed_keys = &mut self.suppressed_keys;

        for layer in self
            .shells
            .layer_state
            .lock()
            .unwrap()
            .layer_surfaces()
            .iter()
            .rev()
        {
            if let Some(data) = layer.get_surface().map(|surface| {
                with_states(surface, |states| {
                    *states.cached_state.current::<LayerSurfaceCachedState>()
                })
                .unwrap()
            }) {
                if data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay)
                {
                    self.keyboard
                        .set_focus(Some(layer.get_surface().unwrap()), serial);
                    self.keyboard
                        .input::<(), _>(keycode, state, serial, time, |_, _| FilterResult::Forward);
                    return KeyAction::None;
                }
            }
        }

        self.keyboard
            .input(keycode, state, serial, time, |modifiers, handle| {
                let keysym = handle.modified_sym();

                debug!(log, "keysym";
                    "state" => format!("{:?}", state),
                    "mods" => format!("{:?}", modifiers),
                    "keysym" => ::xkbcommon::xkb::keysym_get_name(keysym)
                );

                // If the key is pressed and triggered a action
                // we will not forward the key to the client.
                // Additionally add the key to the suppressed keys
                // so that we can decide on a release if the key
                // should be forwarded to the client or not.
                if let KeyState::Pressed = state {
                    let action = process_keyboard_shortcut(*modifiers, keysym);

                    if action.is_some() {
                        suppressed_keys.push(keysym);
                    }

                    action
                        .map(FilterResult::Intercept)
                        .unwrap_or(FilterResult::Forward)
                } else {
                    let suppressed = suppressed_keys.contains(&keysym);
                    if suppressed {
                        suppressed_keys.retain(|k| *k != keysym);
                        FilterResult::Intercept(KeyAction::None)
                    } else {
                        FilterResult::Forward
                    }
                }
            })
            .unwrap_or(KeyAction::None)
    }

    fn on_pointer_button<B: InputBackend>(&mut self, evt: B::PointerButtonEvent) {
        let serial = SCOUNTER.next_serial();
        let button = evt.button_code();
        let state = match evt.state() {
            input::ButtonState::Pressed => wl_pointer::ButtonState::Pressed,
            input::ButtonState::Released => wl_pointer::ButtonState::Released,
        };

        if wl_pointer::ButtonState::Pressed == state {
            self.update_keyboard_focus(serial);
        };
        self.pointer.button(button, state, serial, evt.time());
    }

    fn update_keyboard_focus(&mut self, serial: Serial) {
        // change the keyboard focus unless the pointer is grabbed
        if !self.pointer.is_grabbed() {
            let mut space = self.space.borrow_mut();

            if let Some(output) = space.output_under(self.pointer_location) {
                let output_geo = space.output_geometry(output).unwrap();
                if let Some(window) = output
                    .user_data()
                    .get::<FullscreenSurface>()
                    .and_then(|f| f.get())
                {
                    let surface = window
                        .surface_under(self.pointer_location - output_geo.loc.to_f64())
                        .map(|(s, _)| s);
                    self.keyboard.set_focus(surface.as_ref(), serial);
                    return;
                }

                let layers = layer_map_for_output(output);
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Overlay, self.pointer_location)
                    .or_else(|| layers.layer_under(WlrLayer::Top, self.pointer_location))
                {
                    if layer.can_receive_keyboard_focus() {
                        let surface = layer
                            .surface_under(
                                self.pointer_location
                                    - output_geo.loc.to_f64()
                                    - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                            )
                            .map(|(s, _)| s);
                        self.keyboard.set_focus(surface.as_ref(), serial);
                        return;
                    }
                }
            }

            if let Some(window) = space.window_under(self.pointer_location).cloned() {
                space.raise_window(&window, true);
                let window_loc = space.window_geometry(&window).unwrap().loc;
                let surface = window
                    .surface_under(self.pointer_location - window_loc.to_f64())
                    .map(|(s, _)| s);
                self.keyboard.set_focus(surface.as_ref(), serial);
                return;
            }

            if let Some(output) = space.output_under(self.pointer_location) {
                let output_geo = space.output_geometry(output).unwrap();
                let layers = layer_map_for_output(output);
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Bottom, self.pointer_location)
                    .or_else(|| layers.layer_under(WlrLayer::Background, self.pointer_location))
                {
                    if layer.can_receive_keyboard_focus() {
                        let surface = layer
                            .surface_under(
                                self.pointer_location
                                    - output_geo.loc.to_f64()
                                    - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                            )
                            .map(|(s, _)| s);
                        self.keyboard.set_focus(surface.as_ref(), serial);
                    }
                }
            }
        }
    }

    fn surface_under(&self) -> Option<(WlSurface, Point<i32, Logical>)> {
        let pos = self.pointer_location;
        let space = self.space.borrow();
        let output = space.outputs().find(|o| {
            let geometry = space.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = space.output_geometry(output).unwrap();
        let layers = layer_map_for_output(output);

        let mut under = None;
        if let Some(window) = output
            .user_data()
            .get::<FullscreenSurface>()
            .and_then(|f| f.get())
        {
            under = window.surface_under(pos - output_geo.loc.to_f64());
        } else if let Some(layer) = layers
            .layer_under(WlrLayer::Overlay, pos)
            .or_else(|| layers.layer_under(WlrLayer::Top, pos))
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;
            under = layer
                .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64())
                .map(|(s, loc)| (s, loc + layer_loc));
        } else if let Some(window) = space.window_under(pos) {
            let window_loc = space.window_geometry(window).unwrap().loc;
            under = window
                .surface_under(pos - window_loc.to_f64())
                .map(|(s, loc)| (s, loc + window_loc));
        } else if let Some(layer) = layers
            .layer_under(WlrLayer::Bottom, pos)
            .or_else(|| layers.layer_under(WlrLayer::Background, pos))
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;
            under = layer
                .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64())
                .map(|(s, loc)| (s, loc + layer_loc));
        };
        under
    }

    fn on_pointer_axis<B: InputBackend>(&mut self, evt: B::PointerAxisEvent) {
        let source = match evt.source() {
            input::AxisSource::Continuous => wl_pointer::AxisSource::Continuous,
            input::AxisSource::Finger => wl_pointer::AxisSource::Finger,
            input::AxisSource::Wheel | input::AxisSource::WheelTilt => wl_pointer::AxisSource::Wheel,
        };
        let horizontal_amount = evt
            .amount(input::Axis::Horizontal)
            .unwrap_or_else(|| evt.amount_discrete(input::Axis::Horizontal).unwrap() * 3.0);
        let vertical_amount = evt
            .amount(input::Axis::Vertical)
            .unwrap_or_else(|| evt.amount_discrete(input::Axis::Vertical).unwrap() * 3.0);
        let horizontal_amount_discrete = evt.amount_discrete(input::Axis::Horizontal);
        let vertical_amount_discrete = evt.amount_discrete(input::Axis::Vertical);

        {
            let mut frame = AxisFrame::new(evt.time()).source(source);
            if horizontal_amount != 0.0 {
                frame = frame.value(wl_pointer::Axis::HorizontalScroll, horizontal_amount);
                if let Some(discrete) = horizontal_amount_discrete {
                    frame = frame.discrete(wl_pointer::Axis::HorizontalScroll, discrete as i32);
                }
            } else if source == wl_pointer::AxisSource::Finger {
                frame = frame.stop(wl_pointer::Axis::HorizontalScroll);
            }
            if vertical_amount != 0.0 {
                frame = frame.value(wl_pointer::Axis::VerticalScroll, vertical_amount);
                if let Some(discrete) = vertical_amount_discrete {
                    frame = frame.discrete(wl_pointer::Axis::VerticalScroll, discrete as i32);
                }
            } else if source == wl_pointer::AxisSource::Finger {
                frame = frame.stop(wl_pointer::Axis::VerticalScroll);
            }
            self.pointer.axis(frame);
        }
    }
}

#[cfg(any(feature = "winit", feature = "x11"))]
impl<Backend: crate::state::Backend> AnvilState<Backend> {
    pub fn process_input_event_windowed<B: InputBackend>(&mut self, event: InputEvent<B>, output_name: &str) {
        match event {
            InputEvent::Keyboard { event } => match self.keyboard_key_to_action::<B>(event) {
                KeyAction::ScaleUp => {
                    let mut space = self.space.borrow_mut();
                    let output = space.outputs().find(|o| o.name() == output_name).unwrap().clone();

                    let geometry = space.output_geometry(&output).unwrap();
                    let current_scale = space.output_scale(&output).unwrap();
                    let new_scale = current_scale + 0.25;
                    output.change_current_state(None, None, Some(new_scale.ceil() as i32), None);
                    space.map_output(&output, new_scale, geometry.loc);
                    self.backend_data.reset_buffers(&output);
                }

                KeyAction::ScaleDown => {
                    let mut space = self.space.borrow_mut();
                    let output = space.outputs().find(|o| o.name() == output_name).unwrap().clone();

                    let geometry = space.output_geometry(&output).unwrap();
                    let current_scale = space.output_scale(&output).unwrap();
                    let new_scale = current_scale - 0.25;
                    output.change_current_state(None, None, Some(new_scale.ceil() as i32), None);
                    space.map_output(&output, new_scale, geometry.loc);
                    self.backend_data.reset_buffers(&output);
                }

                action => match action {
                    KeyAction::None | KeyAction::Quit | KeyAction::Run(_) => {
                        self.process_common_key_action(action)
                    }

                    _ => warn!(
                        self.log,
                        "Key action {:?} unsupported on on output {} backend.", action, output_name
                    ),
                },
            },

            InputEvent::PointerMotionAbsolute { event } => {
                let output = self
                    .space
                    .borrow()
                    .outputs()
                    .find(|o| o.name() == output_name)
                    .unwrap()
                    .clone();
                self.on_pointer_move_absolute_windowed::<B>(event, &output)
            }
            InputEvent::PointerButton { event } => self.on_pointer_button::<B>(event),
            InputEvent::PointerAxis { event } => self.on_pointer_axis::<B>(event),
            _ => (), // other events are not handled in anvil (yet)
        }
    }

    fn on_pointer_move_absolute_windowed<B: InputBackend>(
        &mut self,
        evt: B::PointerMotionAbsoluteEvent,
        output: &Output,
    ) {
        let output_geo = self.space.borrow().output_geometry(output).unwrap();

        let pos = evt.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        self.pointer_location = pos;
        let serial = SCOUNTER.next_serial();

        let under = self.surface_under();
        self.pointer.motion(pos, under, serial, evt.time());
    }
}

#[cfg(feature = "udev")]
impl AnvilState<UdevData> {
    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event, .. } => match self.keyboard_key_to_action::<B>(event) {
                #[cfg(feature = "udev")]
                KeyAction::VtSwitch(vt) => {
                    info!(self.log, "Trying to switch to vt {}", vt);
                    if let Err(err) = self.backend_data.session.change_vt(vt) {
                        error!(self.log, "Error switching to vt {}: {}", vt, err);
                    }
                }
                KeyAction::Screen(num) => {
                    let space = self.space.borrow();
                    let geometry = space
                        .outputs()
                        .skip(num)
                        .next()
                        .map(|o| space.output_geometry(o).unwrap());

                    if let Some(geometry) = geometry {
                        let x = geometry.loc.x as f64 + geometry.size.w as f64 / 2.0;
                        let y = geometry.size.h as f64 / 2.0;
                        self.pointer_location = (x, y).into()
                    }
                }
                KeyAction::ScaleUp => {
                    let mut space = self.space.borrow_mut();

                    let pos = self.pointer_location.to_i32_round();
                    let output = space
                        .outputs()
                        .find(|o| space.output_geometry(o).unwrap().contains(pos))
                        .cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (
                            space.output_geometry(&output).unwrap().loc,
                            space.output_scale(&output).unwrap(),
                        );
                        let new_scale = scale + 0.25;
                        output.change_current_state(None, None, Some(new_scale.ceil() as i32), None);
                        // TODO: this might cause underlap... (so we need the code from output_map::update)
                        space.map_output(&output, new_scale, output_location);
                        layer_map_for_output(&output).arrange();

                        let rescale = scale as f64 / new_scale as f64;
                        let output_location = output_location.to_f64();
                        let mut pointer_output_location = self.pointer_location - output_location;
                        pointer_output_location.x *= rescale;
                        pointer_output_location.y *= rescale;
                        self.pointer_location = output_location + pointer_output_location;

                        std::mem::drop(space);
                        let under = self.surface_under();
                        self.pointer
                            .motion(self.pointer_location, under, SCOUNTER.next_serial(), 0);
                        self.backend_data.reset_buffers(&output);
                    }
                }
                KeyAction::ScaleDown => {
                    let mut space = self.space.borrow_mut();

                    let pos = self.pointer_location.to_i32_round();
                    let output = space
                        .outputs()
                        .find(|o| space.output_geometry(o).unwrap().contains(pos))
                        .cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (
                            space.output_geometry(&output).unwrap().loc,
                            space.output_scale(&output).unwrap(),
                        );
                        let new_scale = f64::max(1.0, scale - 0.25);
                        output.change_current_state(None, None, Some(new_scale.ceil() as i32), None);
                        // TODO: this might cause underlap... (so we need the code from output_map::update)
                        space.map_output(&output, new_scale, output_location);
                        layer_map_for_output(&output).arrange();

                        let rescale = scale as f64 / new_scale as f64;
                        let output_location = output_location.to_f64();
                        let mut pointer_output_location = self.pointer_location - output_location;
                        pointer_output_location.x *= rescale;
                        pointer_output_location.y *= rescale;
                        self.pointer_location = output_location + pointer_output_location;

                        std::mem::drop(space);
                        let under = self.surface_under();
                        self.pointer
                            .motion(self.pointer_location, under, SCOUNTER.next_serial(), 0);
                        self.backend_data.reset_buffers(&output);
                    }
                }

                action => match action {
                    KeyAction::None | KeyAction::Quit | KeyAction::Run(_) => {
                        self.process_common_key_action(action)
                    }

                    _ => unreachable!(),
                },
            },
            InputEvent::PointerMotion { event, .. } => self.on_pointer_move::<B>(event),
            InputEvent::PointerButton { event, .. } => self.on_pointer_button::<B>(event),
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis::<B>(event),
            InputEvent::TabletToolAxis { event, .. } => self.on_tablet_tool_axis::<B>(event),
            InputEvent::TabletToolProximity { event, .. } => self.on_tablet_tool_proximity::<B>(event),
            InputEvent::TabletToolTip { event, .. } => self.on_tablet_tool_tip::<B>(event),
            InputEvent::TabletToolButton { event, .. } => self.on_tablet_button::<B>(event),
            InputEvent::DeviceAdded { device } => {
                if device.has_capability(DeviceCapability::TabletTool) {
                    self.seat
                        .tablet_seat()
                        .add_tablet(&TabletDescriptor::from(&device));
                }
            }
            InputEvent::DeviceRemoved { device } => {
                if device.has_capability(DeviceCapability::TabletTool) {
                    let tablet_seat = self.seat.tablet_seat();

                    tablet_seat.remove_tablet(&TabletDescriptor::from(&device));

                    // If there are no tablets in seat we can remove all tools
                    if tablet_seat.count_tablets() == 0 {
                        tablet_seat.clear_tools();
                    }
                }
            }
            _ => {
                // other events are not handled in anvil (yet)
            }
        }
    }

    fn on_pointer_move<B: InputBackend>(&mut self, evt: B::PointerMotionEvent) {
        let serial = SCOUNTER.next_serial();
        self.pointer_location += evt.delta();

        // clamp to screen limits
        // this event is never generated by winit
        self.pointer_location = self.clamp_coords(self.pointer_location);

        let under = self.surface_under();
        self.pointer
            .motion(self.pointer_location, under, serial, evt.time());
    }

    fn on_tablet_tool_axis<B: InputBackend>(&mut self, evt: B::TabletToolAxisEvent) {
        let tablet_seat = self.seat.tablet_seat();

        let space = self.space.borrow();
        let output_geometry = space.outputs().next().map(|o| space.output_geometry(o).unwrap());

        if let Some(rect) = output_geometry {
            self.pointer_location = evt.position_transformed(rect.size) + rect.loc.to_f64();

            let under = self.surface_under();
            let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&evt.device()));
            let tool = tablet_seat.get_tool(&evt.tool());

            if let (Some(tablet), Some(tool)) = (tablet, tool) {
                if evt.pressure_has_changed() {
                    tool.pressure(evt.pressure());
                }
                if evt.distance_has_changed() {
                    tool.distance(evt.distance());
                }
                if evt.tilt_has_changed() {
                    tool.tilt(evt.tilt());
                }
                if evt.slider_has_changed() {
                    tool.slider_position(evt.slider_position());
                }
                if evt.rotation_has_changed() {
                    tool.rotation(evt.rotation());
                }
                if evt.wheel_has_changed() {
                    tool.wheel(evt.wheel_delta(), evt.wheel_delta_discrete());
                }

                tool.motion(
                    self.pointer_location,
                    under,
                    &tablet,
                    SCOUNTER.next_serial(),
                    evt.time(),
                );
            }
        }
    }

    fn on_tablet_tool_proximity<B: InputBackend>(&mut self, evt: B::TabletToolProximityEvent) {
        let space = self.space.borrow();
        let tablet_seat = self.seat.tablet_seat();

        let output_geometry = space.outputs().next().map(|o| space.output_geometry(o).unwrap());

        if let Some(rect) = output_geometry {
            let tool = evt.tool();
            tablet_seat.add_tool(&tool);

            self.pointer_location = evt.position_transformed(rect.size) + rect.loc.to_f64();

            let under = self.surface_under();
            let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&evt.device()));
            let tool = tablet_seat.get_tool(&tool);

            if let (Some(under), Some(tablet), Some(tool)) = (under, tablet, tool) {
                match evt.state() {
                    ProximityState::In => tool.proximity_in(
                        self.pointer_location,
                        under,
                        &tablet,
                        SCOUNTER.next_serial(),
                        evt.time(),
                    ),
                    ProximityState::Out => tool.proximity_out(evt.time()),
                }
            }
        }
    }

    fn on_tablet_tool_tip<B: InputBackend>(&mut self, evt: B::TabletToolTipEvent) {
        let tool = self.seat.tablet_seat().get_tool(&evt.tool());

        if let Some(tool) = tool {
            match evt.tip_state() {
                TabletToolTipState::Down => {
                    let serial = SCOUNTER.next_serial();
                    tool.tip_down(serial, evt.time());

                    // change the keyboard focus
                    self.update_keyboard_focus(serial);
                }
                TabletToolTipState::Up => {
                    tool.tip_up(evt.time());
                }
            }
        }
    }

    fn on_tablet_button<B: InputBackend>(&mut self, evt: B::TabletToolButtonEvent) {
        let tool = self.seat.tablet_seat().get_tool(&evt.tool());

        if let Some(tool) = tool {
            tool.button(
                evt.button(),
                evt.button_state(),
                SCOUNTER.next_serial(),
                evt.time(),
            );
        }
    }

    fn clamp_coords(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let space = self.space.borrow();
        if space.outputs().next().is_none() {
            return pos;
        }

        let (pos_x, pos_y) = pos.into();
        let max_x = space
            .outputs()
            .fold(0, |acc, o| acc + space.output_geometry(o).unwrap().size.w);
        let clamped_x = pos_x.max(0.0).min(max_x as f64);
        let max_y = space
            .outputs()
            .find(|o| {
                let geo = space.output_geometry(o).unwrap();
                geo.contains((clamped_x as i32, 0))
            })
            .map(|o| space.output_geometry(o).unwrap().size.h);

        if let Some(max_y) = max_y {
            let clamped_y = pos_y.max(0.0).min(max_y as f64);
            (clamped_x, clamped_y).into()
        } else {
            (clamped_x, pos_y).into()
        }
    }
}

/// Possible results of a keyboard action
#[derive(Debug)]
enum KeyAction {
    /// Quit the compositor
    Quit,
    /// Trigger a vt-switch
    VtSwitch(i32),
    /// run a command
    Run(String),
    /// Switch the current screen
    Screen(usize),
    ScaleUp,
    ScaleDown,
    /// Do nothing more
    None,
}

fn process_keyboard_shortcut(modifiers: ModifiersState, keysym: Keysym) -> Option<KeyAction> {
    if modifiers.ctrl && modifiers.alt && keysym == xkb::KEY_BackSpace
        || modifiers.logo && keysym == xkb::KEY_q
    {
        // ctrl+alt+backspace = quit
        // logo + q = quit
        Some(KeyAction::Quit)
    } else if (xkb::KEY_XF86Switch_VT_1..=xkb::KEY_XF86Switch_VT_12).contains(&keysym) {
        // VTSwicth
        Some(KeyAction::VtSwitch(
            (keysym - xkb::KEY_XF86Switch_VT_1 + 1) as i32,
        ))
    } else if modifiers.logo && keysym == xkb::KEY_Return {
        // run terminal
        Some(KeyAction::Run("weston-terminal".into()))
    } else if modifiers.logo && keysym >= xkb::KEY_1 && keysym <= xkb::KEY_9 {
        Some(KeyAction::Screen((keysym - xkb::KEY_1) as usize))
    } else if modifiers.logo && modifiers.shift && keysym == xkb::KEY_M {
        Some(KeyAction::ScaleDown)
    } else if modifiers.logo && modifiers.shift && keysym == xkb::KEY_P {
        Some(KeyAction::ScaleUp)
    } else {
        None
    }
}
