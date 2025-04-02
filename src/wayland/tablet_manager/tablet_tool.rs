use std::fmt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::backend::input::{ButtonState, TabletToolCapabilities, TabletToolDescriptor, TabletToolType};
use crate::input::pointer::{CursorImageAttributes, CursorImageStatus};
use crate::utils::{Client as ClientCoords, Logical, Point};
use crate::wayland::compositor::CompositorHandler;
use crate::wayland::seat::CURSOR_IMAGE_ROLE;
use atomic_float::AtomicF64;
use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_seat_v2::ZwpTabletSeatV2,
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::Weak;
use wayland_server::{backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::{utils::Serial, wayland::compositor};

use super::tablet::TabletHandle;
use super::tablet_seat::TabletSeatHandler;
use super::TabletManagerState;

#[derive(Debug, Default)]
pub(crate) struct TabletTool {
    instances: Vec<Weak<ZwpTabletToolV2>>,
    pub(crate) focus: Option<WlSurface>,

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
        (focus, sloc): (WlSurface, Point<f64, Logical>),
        tablet: &TabletHandle,
        serial: Serial,
        time: u32,
    ) {
        let wl_tool = self.instances.iter().find(|i| i.id().same_client_as(&focus.id()));

        if let Some(wl_tool) = wl_tool.and_then(|tool| tool.upgrade().ok()) {
            tablet.with_focused_tablet(&focus, |wl_tablet| {
                wl_tool.proximity_in(serial.into(), wl_tablet, &focus);
                // proximity_in has to be followed by motion event (required by protocol)
                let client_scale = wl_tool
                    .data::<TabletToolUserData>()
                    .unwrap()
                    .client_scale
                    .load(Ordering::Acquire);
                let srel_loc = (loc - sloc).to_client(client_scale);
                wl_tool.motion(srel_loc.x, srel_loc.y);
                wl_tool.frame(time);
            });
        }

        self.focus = Some(focus.clone());
    }

    fn proximity_out(&mut self, time: u32) {
        if let Some(ref focus) = self.focus {
            let wl_tool = self.instances.iter().find(|i| i.id().same_client_as(&focus.id()));

            if let Some(wl_tool) = wl_tool.and_then(|tool| tool.upgrade().ok()) {
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
                .find(|i| i.id().same_client_as(&focus.id()))
                .and_then(|tool| tool.upgrade().ok())
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
                .find(|i| i.id().same_client_as(&focus.id()))
                .and_then(|tool| tool.upgrade().ok())
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
        focus: Option<(WlSurface, Point<f64, Logical>)>,
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
                        .find(|i| i.id().same_client_as(&focus.0.id()))
                        .and_then(|tool| tool.upgrade().ok())
                    {
                        let client_scale = wl_tool
                            .data::<TabletToolUserData>()
                            .unwrap()
                            .client_scale
                            .load(Ordering::Acquire);
                        let srel_loc = (pos - focus.1).to_client(client_scale);
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
                .find(|i| i.id().same_client_as(&focus.id()))
                .and_then(|tool| tool.upgrade().ok())
            {
                wl_tool.button(serial.into(), button, state.into());
                wl_tool.frame(time);
            }
        }
    }
}

impl Drop for TabletTool {
    fn drop(&mut self) {
        for instance in self.instances.iter().filter_map(|tool| tool.upgrade().ok()) {
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
    pub(crate) inner: Arc<Mutex<TabletTool>>,
}

impl TabletToolHandle {
    pub(super) fn new_instance<D>(
        &mut self,
        state: &mut D,
        client: &Client,
        dh: &DisplayHandle,
        seat: &ZwpTabletSeatV2,
        tool: &TabletToolDescriptor,
    ) where
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData>,
        D: TabletSeatHandler + 'static,
        D: CompositorHandler,
    {
        let desc = tool.clone();

        let client_scale = state.client_compositor_state(client).clone_client_scale();
        let wl_tool = client
            .create_resource::<ZwpTabletToolV2, _, D>(
                dh,
                seat.version(),
                TabletToolUserData {
                    handle: self.clone(),
                    desc,
                    client_scale,
                },
            )
            .unwrap();

        seat.tool_added(&wl_tool);

        wl_tool._type(tool.tool_type.into());

        let high: u32 = (tool.hardware_serial >> 16) as u32;
        let low: u32 = tool.hardware_serial as u32;

        wl_tool.hardware_serial(high, low);

        let high: u32 = (tool.hardware_id_wacom >> 16) as u32;
        let low: u32 = tool.hardware_id_wacom as u32;
        wl_tool.hardware_id_wacom(high, low);

        if tool.capabilities.contains(TabletToolCapabilities::PRESSURE) {
            wl_tool.capability(zwp_tablet_tool_v2::Capability::Pressure);
        }

        if tool.capabilities.contains(TabletToolCapabilities::DISTANCE) {
            wl_tool.capability(zwp_tablet_tool_v2::Capability::Distance);
        }

        if tool.capabilities.contains(TabletToolCapabilities::TILT) {
            wl_tool.capability(zwp_tablet_tool_v2::Capability::Tilt);
        }

        if tool.capabilities.contains(TabletToolCapabilities::SLIDER) {
            wl_tool.capability(zwp_tablet_tool_v2::Capability::Slider);
        }

        if tool.capabilities.contains(TabletToolCapabilities::ROTATION) {
            wl_tool.capability(zwp_tablet_tool_v2::Capability::Rotation);
        }

        if tool.capabilities.contains(TabletToolCapabilities::WHEEL) {
            wl_tool.capability(zwp_tablet_tool_v2::Capability::Wheel);
        }

        wl_tool.done();
        self.inner.lock().unwrap().instances.push(wl_tool.downgrade());
    }

    /// Notify that this tool is focused on a certain surface.
    ///
    /// You provide the location of the tool, in the form of:
    ///
    /// - The coordinates of the tool in the global compositor space
    /// - The surface on top of which the tool is, and the coordinates of its
    ///   origin in the global compositor space.
    pub fn proximity_in(
        &self,
        pos: Point<f64, Logical>,
        focus: (WlSurface, Point<f64, Logical>),
        tablet: &TabletHandle,
        serial: Serial,
        time: u32,
    ) {
        self.inner
            .lock()
            .unwrap()
            .proximity_in(pos, focus, tablet, serial, time)
    }

    /// Notify that this tool has left proximity.
    pub fn proximity_out(&self, time: u32) {
        self.inner.lock().unwrap().proximity_out(time);
    }

    /// Tablet tool is making contact
    pub fn tip_down(&self, serial: Serial, time: u32) {
        self.inner.lock().unwrap().tip_down(serial, time);
    }

    /// Tablet tool is no longer making contact
    pub fn tip_up(&self, time: u32) {
        self.inner.lock().unwrap().tip_up(time);
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
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        tablet: &TabletHandle,
        serial: Serial,
        time: u32,
    ) {
        self.inner
            .lock()
            .unwrap()
            .motion(pos, focus, tablet, serial, time)
    }

    /// Queue tool pressure update
    ///
    /// It will be sent alongside next motion event
    pub fn pressure(&self, pressure: f64) {
        self.inner.lock().unwrap().pressure(pressure);
    }

    /// Queue tool distance update
    ///
    /// It will be sent alongside next motion event
    pub fn distance(&self, distance: f64) {
        self.inner.lock().unwrap().distance(distance);
    }

    /// Queue tool tilt update
    ///
    /// It will be sent alongside next motion event
    pub fn tilt(&self, tilt: (f64, f64)) {
        self.inner.lock().unwrap().tilt(tilt);
    }

    /// Queue tool rotation update
    ///
    /// It will be sent alongside next motion event
    pub fn rotation(&self, rotation: f64) {
        self.inner.lock().unwrap().rotation(rotation);
    }

    /// Queue tool slider update
    ///
    /// It will be sent alongside next motion event
    pub fn slider_position(&self, slider: f64) {
        self.inner.lock().unwrap().slider_position(slider);
    }

    /// Queue tool wheel update
    ///
    /// It will be sent alongside next motion event
    pub fn wheel(&self, degrees: f64, clicks: i32) {
        self.inner.lock().unwrap().wheel(degrees, clicks);
    }

    /// Button on the tool was pressed or released
    pub fn button(&self, button: u32, state: ButtonState, serial: Serial, time: u32) {
        self.inner.lock().unwrap().button(button, state, serial, time);
    }
}

impl From<TabletToolType> for zwp_tablet_tool_v2::Type {
    #[inline]
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
    #[inline]
    fn from(from: ButtonState) -> zwp_tablet_tool_v2::ButtonState {
        match from {
            ButtonState::Pressed => zwp_tablet_tool_v2::ButtonState::Pressed,
            ButtonState::Released => zwp_tablet_tool_v2::ButtonState::Released,
        }
    }
}

/// User data of ZwpTabletToolV2 object
pub struct TabletToolUserData {
    pub(crate) handle: TabletToolHandle,
    pub(crate) desc: TabletToolDescriptor,
    client_scale: Arc<AtomicF64>,
}

impl fmt::Debug for TabletToolUserData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabletToolUserData")
            .field("handle", &self.handle)
            .field("desc", &self.desc)
            .field("client_scale", &self.client_scale)
            .finish()
    }
}

impl<D> Dispatch<ZwpTabletToolV2, TabletToolUserData, D> for TabletManagerState
where
    D: Dispatch<ZwpTabletToolV2, TabletToolUserData>,
    D: TabletSeatHandler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        tool: &ZwpTabletToolV2,
        request: zwp_tablet_tool_v2::Request,
        data: &TabletToolUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_tablet_tool_v2::Request::SetCursor {
                surface,
                hotspot_x,
                hotspot_y,
                ..
            } => {
                let focus = data.handle.inner.lock().unwrap().focus.clone();

                if let Some(focus) = focus {
                    if focus.id().same_client_as(&tool.id()) {
                        if let Some(surface) = surface {
                            // tolerate re-using the same surface
                            if compositor::give_role(&surface, CURSOR_IMAGE_ROLE).is_err()
                                && compositor::get_role(&surface) != Some(CURSOR_IMAGE_ROLE)
                            {
                                tool.post_error(
                                    zwp_tablet_tool_v2::Error::Role,
                                    "Given wl_surface has another role.",
                                );
                                return;
                            }

                            compositor::with_states(&surface, |states| {
                                states.data_map.insert_if_missing_threadsafe(|| {
                                    Mutex::new(CursorImageAttributes {
                                        hotspot: (0, 0).into(),
                                    })
                                });
                                let client_scale = tool
                                    .data::<TabletToolUserData>()
                                    .unwrap()
                                    .client_scale
                                    .load(Ordering::Acquire);
                                let hotspot = Point::<_, ClientCoords>::from((hotspot_x, hotspot_y))
                                    .to_f64()
                                    .to_logical(client_scale)
                                    .to_i32_round();
                                states
                                    .data_map
                                    .get::<Mutex<CursorImageAttributes>>()
                                    .unwrap()
                                    .lock()
                                    .unwrap()
                                    .hotspot = hotspot;
                            });

                            state.tablet_tool_image(&data.desc, CursorImageStatus::Surface(surface));
                        } else {
                            state.tablet_tool_image(&data.desc, CursorImageStatus::Hidden);
                        };
                    }
                }
            }
            zwp_tablet_tool_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, resource: &ZwpTabletToolV2, data: &TabletToolUserData) {
        data.handle
            .inner
            .lock()
            .unwrap()
            .instances
            .retain(|i| i.id() != resource.id());
    }
}
