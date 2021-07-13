use std::ops::Deref as _;
use std::sync::Mutex;
use std::{cell::RefCell, rc::Rc};

use crate::backend::input::{ButtonState, TabletToolCapabilitys, TabletToolDescriptor, TabletToolType};
use crate::utils::{Logical, Point};
use crate::wayland::seat::{CursorImageAttributes, CursorImageStatus};
use wayland_protocols::unstable::tablet::v2::server::{
    zwp_tablet_seat_v2::ZwpTabletSeatV2,
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::Filter;

use crate::wayland::{compositor, Serial};

use super::tablet::TabletHandle;

static CURSOR_IMAGE_ROLE: &str = "cursor_image";

#[derive(Debug, Default)]
struct TabletTool {
    instances: Vec<ZwpTabletToolV2>,
    focus: Option<WlSurface>,

    is_down: bool,

    pending_pressure: Option<f64>,
    pending_distance: Option<f64>,
    pending_tilt: Option<(f64, f64)>,
    pending_slider: Option<f64>,
    pending_rotation: Option<f64>,
    pending_wheel: Option<(f64, i32)>,
}

impl TabletTool {
    fn proximity_in(
        &mut self,
        loc: Point<f64, Logical>,
        (focus, sloc): (WlSurface, Point<i32, Logical>),
        tablet: &TabletHandle,
        serial: Serial,
        time: u32,
    ) {
        let wl_tool = self
            .instances
            .iter()
            .find(|i| i.as_ref().same_client_as(focus.as_ref()));

        if let Some(wl_tool) = wl_tool {
            tablet.with_focused_tablet(&focus, |wl_tablet| {
                wl_tool.proximity_in(serial.into(), wl_tablet, &focus);
                // proximity_in has to be followed by motion event (required by protocol)
                let srel_loc = loc - sloc.to_f64();
                wl_tool.motion(srel_loc.x, srel_loc.y);
                wl_tool.frame(time);
            });
        }

        self.focus = Some(focus.clone());
    }

    fn proximity_out(&mut self, time: u32) {
        if let Some(ref focus) = self.focus {
            let wl_tool = self
                .instances
                .iter()
                .find(|i| i.as_ref().same_client_as(focus.as_ref()));

            if let Some(wl_tool) = wl_tool {
                if self.is_down {
                    wl_tool.up();
                    self.is_down = false;
                }
                wl_tool.proximity_out();
                wl_tool.frame(time);
            }
        }

        self.focus = None;
    }

    fn tip_down(&mut self, serial: Serial, time: u32) {
        if let Some(ref focus) = self.focus {
            if let Some(wl_tool) = self
                .instances
                .iter()
                .find(|i| i.as_ref().same_client_as(focus.as_ref()))
            {
                if !self.is_down {
                    wl_tool.down(serial.into());
                    wl_tool.frame(time);
                }
            }
        }

        self.is_down = true;
    }

    fn tip_up(&mut self, time: u32) {
        if let Some(ref focus) = self.focus {
            if let Some(wl_tool) = self
                .instances
                .iter()
                .find(|i| i.as_ref().same_client_as(focus.as_ref()))
            {
                if self.is_down {
                    wl_tool.up();
                    wl_tool.frame(time);
                }
            }
        }

        self.is_down = false;
    }

    fn motion(
        &mut self,
        pos: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        tablet: &TabletHandle,
        serial: Serial,
        time: u32,
    ) {
        match (focus, self.focus.as_ref()) {
            (Some(focus), Some(prev_focus)) => {
                if &focus.0 == prev_focus {
                    if let Some(wl_tool) = self
                        .instances
                        .iter()
                        .find(|i| i.as_ref().same_client_as(focus.0.as_ref()))
                    {
                        let srel_loc = pos - focus.1.to_f64();
                        wl_tool.motion(srel_loc.x, srel_loc.y);

                        if let Some(pressure) = self.pending_pressure.take() {
                            wl_tool.pressure((pressure * 65535.0).round() as u32);
                        }

                        if let Some(distance) = self.pending_distance.take() {
                            wl_tool.distance((distance * 65535.0).round() as u32);
                        }

                        if let Some((x, y)) = self.pending_tilt.take() {
                            wl_tool.tilt(x, y);
                        }

                        if let Some(slider) = self.pending_slider.take() {
                            wl_tool.slider((slider * 65535.0).round() as i32);
                        }

                        if let Some(rotation) = self.pending_rotation.take() {
                            wl_tool.rotation(rotation);
                        }

                        if let Some((degrees, clicks)) = self.pending_wheel.take() {
                            wl_tool.wheel(degrees, clicks)
                        }

                        wl_tool.frame(time);
                    }
                } else {
                    // If surface has changed

                    // Unfocus previous surface
                    self.proximity_out(time);
                    // Focuss a new one
                    self.proximity_in(pos, focus, tablet, serial, time)
                }
            }
            // New surface in focus
            (Some(focus), None) => self.proximity_in(pos, focus, tablet, serial, time),
            // No surface in focus
            (None, _) => self.proximity_out(time),
        }
    }

    fn pressure(&mut self, pressure: f64) {
        self.pending_pressure = Some(pressure);
    }

    fn distance(&mut self, distance: f64) {
        self.pending_distance = Some(distance);
    }

    fn tilt(&mut self, tilt: (f64, f64)) {
        self.pending_tilt = Some(tilt);
    }

    fn rotation(&mut self, rotation: f64) {
        self.pending_rotation = Some(rotation);
    }

    fn slider_position(&mut self, slider: f64) {
        self.pending_slider = Some(slider);
    }

    fn wheel(&mut self, degrees: f64, clicks: i32) {
        self.pending_wheel = Some((degrees, clicks));
    }

    /// Sent whenever a button on the tool is pressed or released.
    fn button(&self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        if let Some(ref focus) = self.focus {
            if let Some(wl_tool) = self
                .instances
                .iter()
                .find(|i| i.as_ref().same_client_as(focus.as_ref()))
            {
                wl_tool.button(serial.into(), button, state.into());
                wl_tool.frame(time);
            }
        }
    }
}

impl Drop for TabletTool {
    fn drop(&mut self) {
        for instance in self.instances.iter() {
            // This event is sent when the tool is removed from the system and will send no further events.
            instance.removed();
        }
    }
}

/// Handle to a tablet tool device
///
/// TabletTool represents a physical tool that has been, or is currently in use with a tablet in seat.
///
/// A TabletTool relation to a physical tool depends on the tablet's ability to report serial numbers. If the tablet supports this capability, then the object represents a specific physical tool and can be identified even when used on multiple tablets.
#[derive(Debug, Default, Clone)]
pub struct TabletToolHandle {
    inner: Rc<RefCell<TabletTool>>,
}

impl TabletToolHandle {
    pub(super) fn new_instance<F>(&mut self, seat: &ZwpTabletSeatV2, tool: &TabletToolDescriptor, mut cb: F)
    where
        F: FnMut(&TabletToolDescriptor, CursorImageStatus) + 'static,
    {
        if let Some(client) = seat.as_ref().client() {
            let wl_tool = client
                .create_resource::<ZwpTabletToolV2>(seat.as_ref().version())
                .unwrap();

            let desc = tool.clone();
            let inner = self.inner.clone();
            wl_tool.quick_assign(move |tool, req, _| {
                use wayland_protocols::unstable::tablet::v2::server::zwp_tablet_tool_v2::Request;
                match req {
                    Request::SetCursor {
                        surface,
                        hotspot_x,
                        hotspot_y,
                        ..
                    } => {
                        let inner = inner.borrow();

                        if let Some(ref focus) = inner.focus {
                            if focus.as_ref().same_client_as(&tool.as_ref()) {
                                if let Some(surface) = surface {
                                    // tolerate re-using the same surface
                                    if compositor::give_role(&surface, CURSOR_IMAGE_ROLE).is_err()
                                        && compositor::get_role(&surface) != Some(CURSOR_IMAGE_ROLE)
                                    {
                                        tool.as_ref().post_error(
                                            zwp_tablet_tool_v2::Error::Role as u32,
                                            "Given wl_surface has another role.".into(),
                                        );
                                        return;
                                    }

                                    compositor::with_states(&surface, |states| {
                                        states.data_map.insert_if_missing_threadsafe(|| {
                                            Mutex::new(CursorImageAttributes {
                                                hotspot: (0, 0).into(),
                                            })
                                        });
                                        states
                                            .data_map
                                            .get::<Mutex<CursorImageAttributes>>()
                                            .unwrap()
                                            .lock()
                                            .unwrap()
                                            .hotspot = (hotspot_x, hotspot_y).into();
                                    })
                                    .unwrap();

                                    cb(&desc, CursorImageStatus::Image(surface));
                                } else {
                                    cb(&desc, CursorImageStatus::Hidden);
                                };
                            }
                        }
                    }
                    Request::Destroy => {
                        // Handled by our destructor
                    }
                    _ => {}
                }
            });

            let inner = self.inner.clone();
            wl_tool.assign_destructor(Filter::new(move |instance: ZwpTabletToolV2, _, _| {
                inner
                    .borrow_mut()
                    .instances
                    .retain(|i| !i.as_ref().equals(&instance.as_ref()));
            }));

            seat.tool_added(&wl_tool);

            wl_tool._type(tool.tool_type.into());

            let high: u32 = (tool.hardware_serial >> 16) as u32;
            let low: u32 = tool.hardware_serial as u32;

            wl_tool.hardware_serial(high, low);

            let high: u32 = (tool.hardware_id_wacom >> 16) as u32;
            let low: u32 = tool.hardware_id_wacom as u32;
            wl_tool.hardware_id_wacom(high, low);

            if tool.capabilitys.contains(TabletToolCapabilitys::PRESSURE) {
                wl_tool.capability(zwp_tablet_tool_v2::Capability::Pressure);
            }

            if tool.capabilitys.contains(TabletToolCapabilitys::DISTANCE) {
                wl_tool.capability(zwp_tablet_tool_v2::Capability::Distance);
            }

            if tool.capabilitys.contains(TabletToolCapabilitys::TILT) {
                wl_tool.capability(zwp_tablet_tool_v2::Capability::Tilt);
            }

            if tool.capabilitys.contains(TabletToolCapabilitys::SLIDER) {
                wl_tool.capability(zwp_tablet_tool_v2::Capability::Slider);
            }

            if tool.capabilitys.contains(TabletToolCapabilitys::ROTATION) {
                wl_tool.capability(zwp_tablet_tool_v2::Capability::Rotation);
            }

            if tool.capabilitys.contains(TabletToolCapabilitys::WHEEL) {
                wl_tool.capability(zwp_tablet_tool_v2::Capability::Wheel);
            }

            wl_tool.done();
            self.inner.borrow_mut().instances.push(wl_tool.deref().clone());
        }
    }

    /// Notifify that this tool is focused on a certain surface.
    ///
    /// You provide the location of the tool, in the form of:
    ///
    /// - The coordinates of the tool in the global compositor space
    /// - The surface on top of which the tool is, and the coordinates of its
    ///   origin in the global compositor space.
    pub fn proximity_in(
        &self,
        pos: Point<f64, Logical>,
        focus: (WlSurface, Point<i32, Logical>),
        tablet: &TabletHandle,
        serial: Serial,
        time: u32,
    ) {
        self.inner
            .borrow_mut()
            .proximity_in(pos, focus, tablet, serial, time)
    }

    /// Notifify that this tool has left proximity.
    pub fn proximity_out(&self, time: u32) {
        self.inner.borrow_mut().proximity_out(time);
    }

    /// Tablet tool is making contact
    pub fn tip_down(&self, serial: Serial, time: u32) {
        self.inner.borrow_mut().tip_down(serial, time);
    }

    /// Tablet tool is no longer making contact
    pub fn tip_up(&self, time: u32) {
        self.inner.borrow_mut().tip_up(time);
    }

    /// Notify that the tool moved
    ///
    /// You provide the new location of the tool, in the form of:
    ///
    /// - The coordinates of the tool in the global compositor space
    /// - The surface on top of which the tool is, and the coordinates of its
    ///   origin in the global compositor space (or `None` of the pointer is not
    ///   on top of a client surface).
    ///
    /// This will internally take care of notifying the appropriate client objects
    /// of proximity_in/proximity_out events.
    pub fn motion(
        &self,
        pos: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<i32, Logical>)>,
        tablet: &TabletHandle,
        serial: Serial,
        time: u32,
    ) {
        self.inner.borrow_mut().motion(pos, focus, tablet, serial, time)
    }

    /// Queue tool pressure update
    ///
    /// It will be sent alongside next motion event
    pub fn pressure(&self, pressure: f64) {
        self.inner.borrow_mut().pressure(pressure);
    }

    /// Queue tool distance update
    ///
    /// It will be sent alongside next motion event
    pub fn distance(&self, distance: f64) {
        self.inner.borrow_mut().distance(distance);
    }

    /// Queue tool tilt update
    ///
    /// It will be sent alongside next motion event
    pub fn tilt(&self, tilt: (f64, f64)) {
        self.inner.borrow_mut().tilt(tilt);
    }

    /// Queue tool rotation update
    ///
    /// It will be sent alongside next motion event
    pub fn rotation(&self, rotation: f64) {
        self.inner.borrow_mut().rotation(rotation);
    }

    /// Queue tool slider update
    ///
    /// It will be sent alongside next motion event
    pub fn slider_position(&self, slider: f64) {
        self.inner.borrow_mut().slider_position(slider);
    }

    /// Queue tool wheel update
    ///
    /// It will be sent alongside next motion event
    pub fn wheel(&self, degrees: f64, clicks: i32) {
        self.inner.borrow_mut().wheel(degrees, clicks);
    }

    /// Button on the tool was pressed or released
    pub fn button(&self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        self.inner.borrow().button(button, state, serial, time);
    }
}

impl From<TabletToolType> for zwp_tablet_tool_v2::Type {
    fn from(from: TabletToolType) -> zwp_tablet_tool_v2::Type {
        match from {
            TabletToolType::Pen => zwp_tablet_tool_v2::Type::Pen,
            TabletToolType::Eraser => zwp_tablet_tool_v2::Type::Eraser,
            TabletToolType::Brush => zwp_tablet_tool_v2::Type::Brush,
            TabletToolType::Pencil => zwp_tablet_tool_v2::Type::Pencil,
            TabletToolType::Airbrush => zwp_tablet_tool_v2::Type::Airbrush,
            TabletToolType::Mouse => zwp_tablet_tool_v2::Type::Mouse,
            TabletToolType::Lens => zwp_tablet_tool_v2::Type::Lens,
            _ => zwp_tablet_tool_v2::Type::Pen,
        }
    }
}

impl From<ButtonState> for zwp_tablet_tool_v2::ButtonState {
    fn from(from: ButtonState) -> zwp_tablet_tool_v2::ButtonState {
        match from {
            ButtonState::Pressed => zwp_tablet_tool_v2::ButtonState::Pressed,
            ButtonState::Released => zwp_tablet_tool_v2::ButtonState::Released,
        }
    }
}
