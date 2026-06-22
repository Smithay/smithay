use std::sync::{Arc, Mutex, atomic::Ordering};

use portable_atomic::AtomicF64;
use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_seat_v2::ZwpTabletSeatV2,
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
};
use wayland_server::{
    Client, Dispatch, DisplayHandle, Resource, Weak,
    backend::{ClientId, ObjectId},
    protocol::wl_surface::WlSurface,
};

use crate::{
    backend::input::{ButtonState, TabletToolCapabilities, TabletToolDescriptor, TabletToolType},
    input::{
        pointer::{CursorImageAttributes, CursorImageStatus},
        tablet::{
            Tablet, TabletSeat, TabletSeatHandler, TabletSeatTrait,
            tool::{
                self, AxisFrame, ButtonEvent, DownEvent, MotionEvent, TabletToolGrab, TabletToolHandle,
                TabletToolInternal, TabletToolRc, TabletToolTarget, UpEvent, WeakTabletToolHandle,
            },
        },
    },
    utils::{Client as ClientCoords, Point, Serial, iter::new_locked_obj_iter_from_vec},
    wayland::{
        Dispatch2,
        compositor::{self, CompositorHandler},
        seat::{CURSOR_IMAGE_ROLE, WaylandFocus},
    },
};

impl<D: TabletSeatHandler + 'static> TabletToolHandle<D> {
    fn new_bound<F>(descriptor: TabletToolDescriptor, default_grab: F) -> Self
    where
        F: Fn() -> Box<dyn TabletToolGrab<D>> + Send + 'static,
    {
        Self {
            arc: Arc::new(TabletToolRc {
                descriptor,
                inner: Mutex::new(TabletToolInternal::new(default_grab)),
                wp_tablet_tool: WpTabletToolHandle {
                    bound: true,
                    last_proximity_in: Default::default(),
                    known_instances: Default::default(),
                },
            }),
        }
    }

    /// Attempt to retrieve a [`TabletToolHandle`] from an existing resource
    pub fn from_resource(tool: &ZwpTabletToolV2) -> Option<Self> {
        tool.data::<TabletToolUserData<D>>()?.handle.upgrade()
    }

    /// Return the raw [`ZwpTabletToolV2`] instance for a particular [`Client`]
    pub fn client_tools<'a>(&'a self, client: &Client) -> impl Iterator<Item = ZwpTabletToolV2> + 'a {
        let guard = self.arc.wp_tablet_tool.known_instances.lock().unwrap();
        new_locked_obj_iter_from_vec(guard, client.id())
    }
}

impl<D: TabletSeatHandler + 'static> TabletSeat<D> {
    /// Add a new tool to a seat and exposes it to wayland clients.
    ///
    /// Tool are usually added on [TabletToolProximityEvent] event.
    ///
    /// Calling this method on a seat that already has the same tool will overwrite it, and will be
    /// seen by clients as if the tool was removed and a new one was added.
    ///
    /// [TabletToolProximityEvent]: crate::backend::input::InputEvent::TabletToolProximity
    pub fn add_wp_tool(
        &self,
        state: &mut D,
        dh: &DisplayHandle,
        tool_desc: &TabletToolDescriptor,
    ) -> TabletToolHandle<D>
    where
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData<D>>,
        D: CompositorHandler,
    {
        self.add_wp_tool_with_grab(state, dh, tool_desc, || Box::new(tool::grab::DefaultGrab))
    }

    /// Add a new tool to a seat and allows the use of a custom default [`TabletToolGrab`]
    ///
    /// The default ghrab is used in case no other grab is currently active. When using
    /// [`TabletSeat::add_wp_tool`], it will use [`tool::DefaultGrab`] which will install
    /// [`tool::DownGrab`] on a down event. [`tool::DownGrab`] makes sure all further event will use
    /// the same target until an up or physical proximity out event.
    ///
    /// See [`TabletSeat::add_wp_tool`] for more information.
    pub fn add_wp_tool_with_grab<F>(
        &self,
        state: &mut D,
        dh: &DisplayHandle,
        tool_desc: &TabletToolDescriptor,
        default_grab: F,
    ) -> TabletToolHandle<D>
    where
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData<D>>,
        D: CompositorHandler,
        F: Fn() -> Box<dyn TabletToolGrab<D>> + Send + 'static,
    {
        let inner = &mut self.arc.lock().unwrap();

        let tool = inner.add_tool(tool_desc, default_grab, TabletToolHandle::new_bound);
        let instances = &mut inner.instances;

        for seat in instances.iter() {
            let Ok(seat) = seat.upgrade() else {
                continue;
            };

            if let Ok(client) = dh.get_client(seat.id()) {
                tool.arc
                    .wp_tablet_tool
                    .new_instance(state, &client, dh, &seat, tool.clone(), tool_desc);
            }
        }

        tool
    }
}

/// User data for ZwpTabletToolV2 object
#[derive(Debug)]
pub struct TabletToolUserData<D: TabletSeatHandler> {
    pub(crate) handle: WeakTabletToolHandle<D>,
    seat_id: ObjectId,
    client_scale: Arc<AtomicF64>,
}

#[derive(Default, Debug)]
pub(crate) struct WpTabletToolHandle {
    pub(crate) bound: bool,
    pub(crate) last_proximity_in: Mutex<Option<Serial>>,
    known_instances: Mutex<Vec<Weak<ZwpTabletToolV2>>>,
}

impl WpTabletToolHandle {
    pub(super) fn new_instance<D>(
        &self,
        state: &mut D,
        client: &Client,
        dh: &DisplayHandle,
        seat: &ZwpTabletSeatV2,
        handle: TabletToolHandle<D>,
        desc: &TabletToolDescriptor,
    ) where
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData<D>>,
        D: CompositorHandler,
        D: TabletSeatHandler,
        D: 'static,
    {
        if !self.bound {
            return;
        }

        let client_scale = state.client_compositor_state(client).clone_client_scale();
        let wp_tool = client
            .create_resource::<ZwpTabletToolV2, _, D>(
                dh,
                seat.version(),
                TabletToolUserData {
                    handle: handle.downgrade(),
                    seat_id: seat.id(),
                    client_scale,
                },
            )
            .unwrap();

        seat.tool_added(&wp_tool);

        wp_tool._type(desc.tool_type.into());

        let high: u32 = (desc.hardware_serial >> 32) as u32;
        let low: u32 = desc.hardware_serial as u32;
        wp_tool.hardware_serial(high, low);

        let high: u32 = (desc.hardware_id_wacom >> 32) as u32;
        let low: u32 = desc.hardware_id_wacom as u32;
        wp_tool.hardware_id_wacom(high, low);

        if desc.capabilities.contains(TabletToolCapabilities::PRESSURE) {
            wp_tool.capability(zwp_tablet_tool_v2::Capability::Pressure);
        }

        if desc.capabilities.contains(TabletToolCapabilities::DISTANCE) {
            wp_tool.capability(zwp_tablet_tool_v2::Capability::Distance);
        }

        if desc.capabilities.contains(TabletToolCapabilities::TILT) {
            wp_tool.capability(zwp_tablet_tool_v2::Capability::Tilt);
        }

        if desc.capabilities.contains(TabletToolCapabilities::SLIDER) {
            wp_tool.capability(zwp_tablet_tool_v2::Capability::Slider);
        }

        if desc.capabilities.contains(TabletToolCapabilities::ROTATION) {
            wp_tool.capability(zwp_tablet_tool_v2::Capability::Rotation);
        }

        if desc.capabilities.contains(TabletToolCapabilities::WHEEL) {
            wp_tool.capability(zwp_tablet_tool_v2::Capability::Wheel);
        }

        wp_tool.done();
        self.known_instances.lock().unwrap().push(wp_tool.downgrade());
    }

    fn proximity_in<D: TabletSeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        tablet: &Tablet,
        serial: Serial,
    ) {
        *self.last_proximity_in.lock().unwrap() = Some(serial);

        self.for_each_focused_tool(surface, |wp_tool| {
            let seat_id = &wp_tool.data::<TabletToolUserData<D>>().unwrap().seat_id;

            let Some(wp_tablet) = tablet.arc.wp_tablet.focused_tablet_for_seat(surface, seat_id) else {
                return;
            };

            wp_tool.proximity_in(serial.0, &wp_tablet, surface);
        });
    }

    fn proximity_out(&self, surface: &WlSurface) {
        self.for_each_focused_tool(surface, |wp_tool| {
            wp_tool.proximity_out();
        });

        *self.last_proximity_in.lock().unwrap() = None;
    }

    fn down(&self, surface: &WlSurface, event: &DownEvent) {
        self.for_each_focused_tool(surface, |wp_tool| {
            wp_tool.down(event.serial.0);
        });
    }

    fn up(&self, surface: &WlSurface) {
        self.for_each_focused_tool(surface, |wp_tool| {
            wp_tool.up();
        });
    }

    fn motion<D: TabletSeatHandler + 'static>(&self, surface: &WlSurface, event: &MotionEvent) {
        self.for_each_focused_tool(surface, |wp_tool| {
            let client_scale = wp_tool
                .data::<TabletToolUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);

            let Point { x, y, .. } = event.location.to_client(client_scale);
            wp_tool.motion(x, y);
        })
    }

    fn axis(&self, surface: &WlSurface, frame: AxisFrame) {
        const NORMALIZE: f64 = 65535.0;
        fn normalize(value: f64) -> u32 {
            (value * NORMALIZE) as u32
        }

        self.for_each_focused_tool(surface, |wp_tool| {
            let AxisFrame {
                pressure,
                distance,
                tilt,
                rotation,
                slider,
                wheel,
            } = frame;

            if let Some(pressure) = pressure.map(normalize) {
                wp_tool.pressure(pressure);
            }

            if let Some(distance) = distance.map(normalize) {
                wp_tool.distance(distance);
            }

            if let Some((tilt_x, tilt_y)) = tilt {
                wp_tool.tilt(tilt_x, tilt_y);
            }

            if let Some(degrees) = rotation {
                wp_tool.rotation(degrees);
            }

            if let Some(slider) = slider.map(|s| (s * NORMALIZE) as i32) {
                wp_tool.slider(slider);
            }

            if let Some((degrees, clicks)) = wheel {
                wp_tool.wheel(degrees, clicks);
            }
        });
    }

    fn button(&self, surface: &WlSurface, event: &ButtonEvent) {
        self.for_each_focused_tool(surface, |wp_tool| {
            let ButtonEvent {
                serial,
                button,
                state,
                ..
            } = *event;
            wp_tool.button(serial.0, button, state.into());
        });
    }

    fn frame(&self, surface: &WlSurface, time: u32) {
        self.for_each_focused_tool(surface, |wp_tool| {
            wp_tool.frame(time);
        });
    }

    fn for_each_focused_tool(&self, surface: &WlSurface, mut f: impl FnMut(ZwpTabletToolV2)) {
        let inner = self.known_instances.lock().unwrap();

        for tool in &*inner {
            let Ok(tool) = tool.upgrade() else {
                continue;
            };

            if tool.id().same_client_as(&surface.id()) {
                f(tool.clone())
            }
        }
    }
}

impl Drop for WpTabletToolHandle {
    fn drop(&mut self) {
        let mut guard = self.known_instances.lock().unwrap();

        for tool in guard.drain(..) {
            let Ok(wp_tool) = tool.upgrade() else { continue };

            wp_tool.removed();
        }
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

impl<D> Dispatch2<ZwpTabletToolV2, D> for TabletToolUserData<D>
where
    D: TabletSeatHandler,
    <D as TabletSeatHandler>::ToolFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &wayland_server::Client,
        tool: &ZwpTabletToolV2,
        request: <ZwpTabletToolV2 as wayland_server::Resource>::Request,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let Some(handle) = self.handle.upgrade() else {
            return;
        };

        match request {
            zwp_tablet_tool_v2::Request::SetCursor {
                serial,
                surface,
                hotspot_x,
                hotspot_y,
            } => {
                if !allow_setting_cursor(&handle, Serial(serial), &tool.id()) {
                    return;
                }

                let cursor_image = match surface {
                    Some(surface) => {
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
                                .data::<TabletToolUserData<D>>()
                                .unwrap()
                                .client_scale
                                .load(Ordering::Acquire);

                            let hotspot = Point::<i32, ClientCoords>::from((hotspot_x, hotspot_y))
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

                        CursorImageStatus::Surface(surface)
                    }
                    None => CursorImageStatus::Hidden,
                };

                state.tablet_tool_image(handle.descriptor(), cursor_image);
            }
            zwp_tablet_tool_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, _state: &mut D, _client: ClientId, tool: &ZwpTabletToolV2) {
        let Some(handle) = self.handle.upgrade() else {
            return;
        };

        handle
            .arc
            .wp_tablet_tool
            .known_instances
            .lock()
            .unwrap()
            .retain(|i| i.id() != tool.id())
    }
}

impl<D> TabletToolTarget<D> for WlSurface
where
    D: TabletSeatHandler + 'static,
{
    fn proximity_in(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        tablet: &Tablet,
        serial: Serial,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.proximity_in::<D>(self, tablet, serial);
        }
    }

    fn proximity_out(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.proximity_out(self);
        }
    }

    fn down(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        event: &DownEvent,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.down(self, event);
        }
    }

    fn up(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        _event: &UpEvent,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.up(self);
        }
    }

    fn motion(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        event: &MotionEvent,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.motion::<D>(self, event);
        }
    }

    fn axis(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        frame: AxisFrame,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.axis(self, frame);
        }
    }

    fn button(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        event: &ButtonEvent,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.button(self, event);
        }
    }

    fn frame(
        &self,
        seat: &crate::input::Seat<D>,
        _data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        time: u32,
    ) {
        let tablet_seat = seat.tablet_seat();

        if let Some(tool) = tablet_seat.get_tool(tool_descriptor) {
            tool.arc.wp_tablet_tool.frame(self, time);
        }
    }
}

pub(crate) fn allow_setting_cursor<D>(
    handle: &TabletToolHandle<D>,
    serial: Serial,
    object_id: &ObjectId,
) -> bool
where
    D: TabletSeatHandler + 'static,
    <D as TabletSeatHandler>::ToolFocus: WaylandFocus,
{
    // Allow client if there is a tool grab for that client. Like drag and drop.
    if handle
        .grab_start_data()
        .and_then(|data| data.focus)
        .is_some_and(|focus| focus.0.same_client_as(object_id))
    {
        return true;
    }

    if !handle
        .arc
        .wp_tablet_tool
        .last_proximity_in
        .lock()
        .unwrap()
        .is_some_and(|last_serial| last_serial == serial)
    {
        return false; // Ignore mismatches in serial
    }

    // Only allow setting the cursor icon if the current tool focus is of the same
    // client.
    handle
        .arc
        .inner
        .lock()
        .unwrap()
        .focus
        .as_ref()
        .is_some_and(|(focus, _)| focus.same_client_as(object_id))
}
