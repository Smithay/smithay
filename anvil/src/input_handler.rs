use std::{process::Command, sync::atomic::Ordering};

use crate::{shell::FullscreenSurface, AnvilState};

#[cfg(feature = "udev")]
use crate::udev::UdevData;

use smithay::{
    backend::input::{
        self, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
        PointerButtonEvent,
    },
    desktop::{layer_map_for_output, WindowSurfaceType},
    reexports::wayland_server::{
        protocol::{wl_pointer, wl_surface::WlSurface},
        DisplayHandle,
    },
    utils::{Logical, Point},
    wayland::{
        compositor::with_states,
        input_method_manager::InputMethodSeatTrait,
        output::Scale,
        seat::{keysyms as xkb, AxisFrame, ButtonEvent, FilterResult, Keysym, ModifiersState, MotionEvent},
        shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer, LayerSurfaceCachedState},
        Serial, SERIAL_COUNTER as SCOUNTER,
    },
};

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
use smithay::backend::input::PointerMotionAbsoluteEvent;

#[cfg(any(feature = "winit", feature = "x11"))]
use smithay::wayland::output::Output;

#[cfg(feature = "udev")]
use crate::state::Backend;
#[cfg(feature = "udev")]
use smithay::{
    backend::{
        input::{
            Device, DeviceCapability, PointerMotionEvent, ProximityState, TabletToolButtonEvent,
            TabletToolEvent, TabletToolProximityEvent, TabletToolTipEvent, TabletToolTipState,
        },
        session::Session,
    },
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

    fn keyboard_key_to_action<B: InputBackend>(
        &mut self,
        dh: &DisplayHandle,
        evt: B::KeyboardKeyEvent,
    ) -> KeyAction {
        let keycode = evt.key_code();
        let state = evt.state();
        debug!(self.log, "key"; "keycode" => keycode, "state" => format!("{:?}", state));
        let serial = SCOUNTER.next_serial();
        let log = &self.log;
        let time = Event::time(&evt);
        let suppressed_keys = &mut self.suppressed_keys;
        let keyboard = self.seat.get_keyboard().unwrap();

        for layer in self.layer_shell_state.layer_surfaces().rev() {
            let data = with_states(layer.wl_surface(), |states| {
                *states.cached_state.current::<LayerSurfaceCachedState>()
            });
            if data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay)
            {
                keyboard.set_focus(dh, Some(layer.wl_surface()), serial);
                keyboard.input::<(), _>(dh, keycode, state, serial, time, |_, _| FilterResult::Forward);
                return KeyAction::None;
            }
        }

        keyboard
            .input(dh, keycode, state, serial, time, |modifiers, handle| {
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

    fn on_pointer_button<B: InputBackend>(&mut self, dh: &DisplayHandle, evt: B::PointerButtonEvent) {
        let serial = SCOUNTER.next_serial();
        let button = evt.button_code();

        let state = wl_pointer::ButtonState::from(evt.state());

        if wl_pointer::ButtonState::Pressed == state {
            self.update_keyboard_focus(dh, serial);
        };
        self.seat.get_pointer().unwrap().button(
            self,
            dh,
            &ButtonEvent {
                button,
                state,
                serial,
                time: evt.time(),
            },
        );
    }

    fn update_keyboard_focus(&mut self, dh: &DisplayHandle, serial: Serial) {
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        let input_method = self.seat.input_method();
        // change the keyboard focus unless the pointer or keyboard is grabbed
        // We test for any matching surface type here but always use the root
        // (in case of a window the toplevel) surface for the focus.
        // So for example if a user clicks on a subsurface or popup the toplevel
        // will receive the keyboard focus. Directly assigning the focus to the
        // matching surface leads to issues with clients dismissing popups and
        // subsurface menus (for example firefox-wayland).
        // see here for a discussion about that issue:
        // https://gitlab.freedesktop.org/wayland/wayland/-/issues/294
        if !pointer.is_grabbed() && (!keyboard.is_grabbed() || input_method.keyboard_grabbed()) {
            if let Some(output) = self.space.output_under(self.pointer_location).next() {
                let output_geo = self.space.output_geometry(output).unwrap();
                if let Some(window) = output
                    .user_data()
                    .get::<FullscreenSurface>()
                    .and_then(|f| f.get())
                {
                    if window
                        .surface_under(
                            self.pointer_location - output_geo.loc.to_f64(),
                            WindowSurfaceType::ALL,
                        )
                        .is_some()
                    {
                        keyboard.set_focus(dh, Some(window.toplevel().wl_surface()), serial);
                        return;
                    }
                }

                let layers = layer_map_for_output(output);
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Overlay, self.pointer_location)
                    .or_else(|| layers.layer_under(WlrLayer::Top, self.pointer_location))
                {
                    if layer.can_receive_keyboard_focus()
                        && layer
                            .surface_under(
                                self.pointer_location
                                    - output_geo.loc.to_f64()
                                    - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                                WindowSurfaceType::ALL,
                            )
                            .is_some()
                    {
                        keyboard.set_focus(dh, Some(layer.wl_surface()), serial);
                        return;
                    }
                }
            }

            if let Some((window, _, _)) = self
                .space
                .surface_under(self.pointer_location, WindowSurfaceType::ALL)
            {
                self.space.raise_window(&window, true);
                keyboard.set_focus(dh, Some(window.toplevel().wl_surface()), serial);
                return;
            }

            if let Some(output) = self.space.output_under(self.pointer_location).next() {
                let output_geo = self.space.output_geometry(output).unwrap();
                let layers = layer_map_for_output(output);
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Bottom, self.pointer_location)
                    .or_else(|| layers.layer_under(WlrLayer::Background, self.pointer_location))
                {
                    if layer.can_receive_keyboard_focus()
                        && layer
                            .surface_under(
                                self.pointer_location
                                    - output_geo.loc.to_f64()
                                    - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                                WindowSurfaceType::ALL,
                            )
                            .is_some()
                    {
                        keyboard.set_focus(dh, Some(layer.wl_surface()), serial);
                    }
                }
            };
        }
    }

    pub fn surface_under(&self) -> Option<(WlSurface, Point<i32, Logical>)> {
        let pos = self.pointer_location;
        let output = self.space.outputs().find(|o| {
            let geometry = self.space.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = self.space.output_geometry(output).unwrap();
        let layers = layer_map_for_output(output);

        let mut under = None;
        if let Some(window) = output
            .user_data()
            .get::<FullscreenSurface>()
            .and_then(|f| f.get())
        {
            under = window.surface_under(pos - output_geo.loc.to_f64(), WindowSurfaceType::ALL);
        } else if let Some(layer) = layers
            .layer_under(WlrLayer::Overlay, pos)
            .or_else(|| layers.layer_under(WlrLayer::Top, pos))
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;
            under = layer
                .surface_under(
                    pos - output_geo.loc.to_f64() - layer_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(s, loc)| (s, loc + layer_loc));
        } else if let Some((_, surface, location)) = self.space.surface_under(pos, WindowSurfaceType::ALL) {
            under = Some((surface, location));
        } else if let Some(layer) = layers
            .layer_under(WlrLayer::Bottom, pos)
            .or_else(|| layers.layer_under(WlrLayer::Background, pos))
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;
            under = layer
                .surface_under(
                    pos - output_geo.loc.to_f64() - layer_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(s, loc)| (s, loc + layer_loc));
        };
        let mut input_method = self.seat.input_method();
        if let Some((_, point)) = under {
            input_method.set_point(&point);
        }
        under
    }

    fn on_pointer_axis<B: InputBackend>(&mut self, dh: &DisplayHandle, evt: B::PointerAxisEvent) {
        let source = wl_pointer::AxisSource::from(evt.source());

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
            self.seat.get_pointer().unwrap().axis(self, dh, frame);
        }
    }
}

#[cfg(any(feature = "winit", feature = "x11"))]
impl<Backend: crate::state::Backend> AnvilState<Backend> {
    pub fn process_input_event_windowed<B: InputBackend>(
        &mut self,
        dh: &DisplayHandle,
        event: InputEvent<B>,
        output_name: &str,
    ) {
        match event {
            InputEvent::Keyboard { event } => match self.keyboard_key_to_action::<B>(dh, event) {
                KeyAction::ScaleUp => {
                    let output = self
                        .space
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = current_scale + 0.25;
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    crate::shell::fixup_positions(dh, &mut self.space);
                    self.backend_data.reset_buffers(&output);
                }

                KeyAction::ScaleDown => {
                    let output = self
                        .space
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = f64::max(1.0, current_scale - 0.25);
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    crate::shell::fixup_positions(dh, &mut self.space);
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
                    .outputs()
                    .find(|o| o.name() == output_name)
                    .unwrap()
                    .clone();
                self.on_pointer_move_absolute_windowed::<B>(dh, event, &output)
            }
            InputEvent::PointerButton { event } => self.on_pointer_button::<B>(dh, event),
            InputEvent::PointerAxis { event } => self.on_pointer_axis::<B>(dh, event),
            _ => (), // other events are not handled in anvil (yet)
        }
    }

    fn on_pointer_move_absolute_windowed<B: InputBackend>(
        &mut self,
        dh: &DisplayHandle,
        evt: B::PointerMotionAbsoluteEvent,
        output: &Output,
    ) {
        let output_geo = self.space.output_geometry(output).unwrap();

        let pos = evt.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        self.pointer_location = pos;
        let serial = SCOUNTER.next_serial();

        let under = self.surface_under();
        self.seat.get_pointer().unwrap().motion(
            self,
            dh,
            &MotionEvent {
                location: pos,
                focus: under,
                serial,
                time: evt.time(),
            },
        );
    }
}

#[cfg(feature = "udev")]
impl AnvilState<UdevData> {
    pub fn process_input_event<B: InputBackend>(&mut self, dh: &DisplayHandle, event: InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event, .. } => match self.keyboard_key_to_action::<B>(dh, event) {
                #[cfg(feature = "udev")]
                KeyAction::VtSwitch(vt) => {
                    info!(self.log, "Trying to switch to vt {}", vt);
                    if let Err(err) = self.backend_data.session.change_vt(vt) {
                        error!(self.log, "Error switching to vt {}: {}", vt, err);
                    }
                }
                KeyAction::Screen(num) => {
                    let geometry = self
                        .space
                        .outputs()
                        .nth(num)
                        .map(|o| self.space.output_geometry(o).unwrap());

                    if let Some(geometry) = geometry {
                        let x = geometry.loc.x as f64 + geometry.size.w as f64 / 2.0;
                        let y = geometry.size.h as f64 / 2.0;
                        self.pointer_location = (x, y).into()
                    }
                }
                KeyAction::ScaleUp => {
                    let pos = self.pointer_location.to_i32_round();
                    let output = self
                        .space
                        .outputs()
                        .find(|o| self.space.output_geometry(o).unwrap().contains(pos))
                        .cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (
                            self.space.output_geometry(&output).unwrap().loc,
                            output.current_scale().fractional_scale(),
                        );
                        let new_scale = scale + 0.25;
                        output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                        let rescale = scale as f64 / new_scale as f64;
                        let output_location = output_location.to_f64();
                        let mut pointer_output_location = self.pointer_location - output_location;
                        pointer_output_location.x *= rescale;
                        pointer_output_location.y *= rescale;
                        self.pointer_location = output_location + pointer_output_location;

                        crate::shell::fixup_positions(dh, &mut self.space);
                        let under = self.surface_under();
                        if let Some(ptr) = self.seat.get_pointer() {
                            ptr.motion(
                                self,
                                dh,
                                &MotionEvent {
                                    location: self.pointer_location,
                                    focus: under,
                                    serial: SCOUNTER.next_serial(),
                                    time: 0,
                                },
                            );
                        }
                        self.backend_data.reset_buffers(&output);
                    }
                }
                KeyAction::ScaleDown => {
                    let pos = self.pointer_location.to_i32_round();
                    let output = self
                        .space
                        .outputs()
                        .find(|o| self.space.output_geometry(o).unwrap().contains(pos))
                        .cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (
                            self.space.output_geometry(&output).unwrap().loc,
                            output.current_scale().fractional_scale(),
                        );
                        let new_scale = f64::max(1.0, scale - 0.25);
                        output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                        let rescale = scale as f64 / new_scale as f64;
                        let output_location = output_location.to_f64();
                        let mut pointer_output_location = self.pointer_location - output_location;
                        pointer_output_location.x *= rescale;
                        pointer_output_location.y *= rescale;
                        self.pointer_location = output_location + pointer_output_location;

                        crate::shell::fixup_positions(dh, &mut self.space);
                        let under = self.surface_under();
                        if let Some(ptr) = self.seat.get_pointer() {
                            ptr.motion(
                                self,
                                dh,
                                &MotionEvent {
                                    location: self.pointer_location,
                                    focus: under,
                                    serial: SCOUNTER.next_serial(),
                                    time: 0,
                                },
                            );
                        }
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
            InputEvent::PointerMotion { event, .. } => self.on_pointer_move::<B>(dh, event),
            InputEvent::PointerMotionAbsolute { event, .. } => self.on_pointer_move_absolute::<B>(dh, event),
            InputEvent::PointerButton { event, .. } => self.on_pointer_button::<B>(dh, event),
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis::<B>(dh, event),
            InputEvent::TabletToolAxis { event, .. } => self.on_tablet_tool_axis::<B>(event),
            InputEvent::TabletToolProximity { event, .. } => self.on_tablet_tool_proximity::<B>(dh, event),
            InputEvent::TabletToolTip { event, .. } => self.on_tablet_tool_tip::<B>(dh, event),
            InputEvent::TabletToolButton { event, .. } => self.on_tablet_button::<B>(event),
            InputEvent::DeviceAdded { device } => {
                if device.has_capability(DeviceCapability::TabletTool) {
                    self.seat
                        .tablet_seat()
                        .add_tablet::<Self>(dh, &TabletDescriptor::from(&device));
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

    fn on_pointer_move<B: InputBackend>(&mut self, dh: &DisplayHandle, evt: B::PointerMotionEvent) {
        let serial = SCOUNTER.next_serial();
        self.pointer_location += evt.delta();

        // clamp to screen limits
        // this event is never generated by winit
        self.pointer_location = self.clamp_coords(self.pointer_location);

        let under = self.surface_under();
        if let Some(ptr) = self.seat.get_pointer() {
            ptr.motion(
                self,
                dh,
                &MotionEvent {
                    location: self.pointer_location,
                    focus: under,
                    serial,
                    time: evt.time(),
                },
            );
        }
    }

    fn on_pointer_move_absolute<B: InputBackend>(
        &mut self,
        dh: &DisplayHandle,
        evt: B::PointerMotionAbsoluteEvent,
    ) {
        let serial = SCOUNTER.next_serial();

        let max_x = self
            .space
            .outputs()
            .fold(0, |acc, o| acc + self.space.output_geometry(o).unwrap().size.w);

        let max_h_output = self
            .space
            .outputs()
            .max_by_key(|o| self.space.output_geometry(o).unwrap().size.h)
            .unwrap();

        let max_y = self.space.output_geometry(max_h_output).unwrap().size.h;

        self.pointer_location.x = evt.x_transformed(max_x);
        self.pointer_location.y = evt.y_transformed(max_y);

        // clamp to screen limits
        self.pointer_location = self.clamp_coords(self.pointer_location);

        let under = self.surface_under();
        if let Some(ptr) = self.seat.get_pointer() {
            ptr.motion(
                self,
                dh,
                &MotionEvent {
                    location: self.pointer_location,
                    focus: under,
                    serial,
                    time: evt.time(),
                },
            );
        }
    }

    fn on_tablet_tool_axis<B: InputBackend>(&mut self, evt: B::TabletToolAxisEvent) {
        let tablet_seat = self.seat.tablet_seat();

        let output_geometry = self
            .space
            .outputs()
            .next()
            .map(|o| self.space.output_geometry(o).unwrap());

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

    fn on_tablet_tool_proximity<B: InputBackend>(
        &mut self,
        dh: &DisplayHandle,
        evt: B::TabletToolProximityEvent,
    ) {
        let tablet_seat = self.seat.tablet_seat();

        let output_geometry = self
            .space
            .outputs()
            .next()
            .map(|o| self.space.output_geometry(o).unwrap());

        if let Some(rect) = output_geometry {
            let tool = evt.tool();
            tablet_seat.add_tool::<Self>(dh, &tool);

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

    fn on_tablet_tool_tip<B: InputBackend>(&mut self, dh: &DisplayHandle, evt: B::TabletToolTipEvent) {
        let tool = self.seat.tablet_seat().get_tool(&evt.tool());

        if let Some(tool) = tool {
            match evt.tip_state() {
                TabletToolTipState::Down => {
                    let serial = SCOUNTER.next_serial();
                    tool.tip_down(serial, evt.time());

                    // change the keyboard focus
                    self.update_keyboard_focus(dh, serial);
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
        if self.space.outputs().next().is_none() {
            return pos;
        }

        let (pos_x, pos_y) = pos.into();
        let max_x = self
            .space
            .outputs()
            .fold(0, |acc, o| acc + self.space.output_geometry(o).unwrap().size.w);
        let clamped_x = pos_x.max(0.0).min(max_x as f64);
        let max_y = self
            .space
            .outputs()
            .find(|o| {
                let geo = self.space.output_geometry(o).unwrap();
                geo.contains((clamped_x as i32, 0))
            })
            .map(|o| self.space.output_geometry(o).unwrap().size.h);

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
        // VTSwitch
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
