//! Tablet Tool related types for smithay's input abstraction

use core::fmt;
use std::sync::{Arc, Mutex};
use std::{hash::Hash, sync::Weak};

use crate::{
    backend::input::{ButtonState, TabletToolDescriptor},
    input::{
        GrabStatus, Seat, SeatHandler,
        pointer::Focus,
        tablet::{Tablet, TabletSeat, TabletSeatHandler},
    },
    utils::{IsAlive, Logical, Point, Serial},
};

pub(crate) mod grab;
pub use grab::{DefaultGrab, DownGrab, GrabStartData, GrabTrigger, TabletToolGrab};

pub(crate) struct TabletToolRc<D: TabletSeatHandler> {
    pub(crate) descriptor: TabletToolDescriptor,
    // We want to drop the wp_tablet_tool before the tablet stored in inner, so clients receive
    // tool's destroy events before tablet's destroy.
    #[cfg(feature = "wayland_frontend")]
    pub(crate) wp_tablet_tool: crate::wayland::tablet_manager::tablet_tool::WpTabletToolHandle,
    pub(crate) inner: Mutex<TabletToolInternal<D>>,
}

#[cfg(not(feature = "wayland_frontend"))]
impl<D: TabletSeatHandler> fmt::Debug for TabletToolRc<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabletToolRc")
            .field("descriptor", &self.descriptor)
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D: TabletSeatHandler> fmt::Debug for TabletToolRc<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabletToolRc")
            .field("descriptor", &self.descriptor)
            .field("wp_tablet_tool", &self.wp_tablet_tool)
            .field("inner", &self.inner)
            .finish()
    }
}

/// Handle to a tablet tool
///
/// It can be cloned and all clones manipulate the same internal state.
///
/// This handle gives you access to an interface to sent tablet tool events to your clients. When
/// sending events using this handle, they will be intercepted by a tablet tool grab if any is
/// active. See [`TabletToolGrab`] trait for details.
///
/// As a tablet tool must be in proximity to a tablet, most methods on this object expect a prior
/// call to [`TabletToolHandle::proximity_in`].
pub struct TabletToolHandle<D: TabletSeatHandler> {
    pub(crate) arc: Arc<TabletToolRc<D>>,
}

impl<D: TabletSeatHandler> fmt::Debug for TabletToolHandle<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabletToolHandle")
            .field("arc", &self.arc)
            .finish()
    }
}

impl<D: TabletSeatHandler> Clone for TabletToolHandle<D> {
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl<D: TabletSeatHandler> PartialEq for TabletToolHandle<D> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}

impl<D: TabletSeatHandler> Eq for TabletToolHandle<D> {}

impl<D: TabletSeatHandler> Hash for TabletToolHandle<D> {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.arc).hash(state);
    }
}

impl<D: TabletSeatHandler + 'static> TabletToolHandle<D> {
    pub(super) fn new<F>(descriptor: TabletToolDescriptor, default_grab: F) -> Self
    where
        F: Fn() -> Box<dyn TabletToolGrab<D>> + Send + 'static,
    {
        Self {
            arc: Arc::new(TabletToolRc {
                descriptor,
                inner: Mutex::new(TabletToolInternal::new(default_grab)),
                #[cfg(feature = "wayland_frontend")]
                wp_tablet_tool: Default::default(),
            }),
        }
    }

    /// Change the current grab on this tablet tool to the provided grab.
    ///
    /// If focus is set to [`Focus::Clear`], any currently focused client object will be unfocused.
    ///
    /// Overwrites any current grab.
    ///
    /// Calling this function prior to calling proximity_in will not set the grab.
    pub fn set_grab<G: TabletToolGrab<D> + 'static>(
        &self,
        data: &mut D,
        grab: G,
        time: u32,
        serial: Serial,
        focus: Focus,
    ) {
        let mut inner = self.arc.inner.lock().unwrap();
        let seat = self.get_seat(data);

        inner.set_grab(data, &seat, &self.arc.descriptor, grab, time, serial, focus);
    }

    /// Remove any current grab on this tablet tool, resetting it to the default behavior.
    pub fn unset_grab(&self, data: &mut D, serial: Serial, time: u32) {
        let mut inner = self.arc.inner.lock().unwrap();
        let seat = self.get_seat(data);

        inner.unset_grab(data, &seat, &self.arc.descriptor, serial, time, true);
    }

    /// Check if this tablet tool is currently grabbed with this serial
    pub fn has_grab(&self, serial: Serial) -> bool {
        let guard = self.arc.inner.lock().unwrap();

        match guard.grab {
            GrabStatus::Active(s, _) => s == serial,
            _ => false,
        }
    }

    /// Check if this tablet tool is currently being grabbed.
    pub fn is_grabbed(&self) -> bool {
        let guard = self.arc.inner.lock().unwrap();
        !matches!(guard.grab, GrabStatus::None)
    }

    /// Return the start data for the grab, if any.
    pub fn grab_start_data(&self) -> Option<GrabStartData<D>> {
        let guard = self.arc.inner.lock().unwrap();
        match &guard.grab {
            GrabStatus::Active(_, g) => Some(g.start_data().clone()),
            _ => None,
        }
    }

    /// Call `f` with the active grab, if any.
    pub fn with_grab<T>(&self, f: impl FnOnce(Serial, &dyn TabletToolGrab<D>) -> T) -> Option<T> {
        let guard = self.arc.inner.lock().unwrap();
        if let GrabStatus::Active(s, g) = &guard.grab {
            Some(f(*s, &**g))
        } else {
            None
        }
    }

    /// Notify that a tool entered physical proximity with a tablet.
    ///
    /// You must invoke this method before any other events. Failing to do so will make the
    /// compositor ignore all other calls.
    ///
    /// You provide the location of the tool in the form of:
    /// - The coordinates of the tool in the global compositor space
    /// - The surface on top of which the tool is, and the coordinates of its origin in the global
    ///   compositor space (or `None` if the tool is not over a client surface).
    ///
    /// This will internally generate the following event to the client object, if any:
    /// - ProximityIn
    /// - Axis, if an axis frame is available
    /// - Motion
    /// - Button press, if any button were held down.
    ///
    /// The [`Tablet`] will be stored until a call to `TabletToolHandle::proximity_out``.
    ///
    /// If this event is invoked while the tool is already in proximity with a tablet, a synthetic
    /// ProximityOut will be generated to reset the instance to a sane state.
    pub fn proximity_in(
        &self,
        data: &mut D,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        tablet: Tablet,
        event: &ProximityInEvent,
    ) {
        let mut inner = self.arc.inner.lock().unwrap();
        let seat = self.get_seat(data);

        if inner.in_proximity() {
            tracing::warn!("proximity_in was called, but the tool is already in proximity with a tablet.");
            inner.proximity_out_impl(
                data,
                &seat,
                &self.arc.descriptor,
                &ProximityOutEvent {
                    serial: event.serial,
                    time: event.time,
                },
            );
        }

        inner.tablet = Some(tablet);
        inner.location = Some(event.location);
        inner.pending_focus.clone_from(&focus);

        inner.with_grab(data, &seat, &self.arc.descriptor, |data, handle, grab| {
            grab.proximity_in(data, handle, focus, event);
        });
    }

    /// Notify that a tool left physical proximity with a tablet.
    ///
    /// This will internally generate the following events:
    /// - Button release, for any pressed button
    /// - Up, if the tool tip was down
    /// - ProximityOut
    pub fn proximity_out(&self, data: &mut D, event: &ProximityOutEvent) {
        let mut inner = self.arc.inner.lock().unwrap();
        if !inner.in_proximity() {
            tracing::warn!("proximity_out was called, but the tool hasn't entered tablet proximity.");
            return;
        }

        let seat = self.get_seat(data);

        inner.proximity_out_impl(data, &seat, &self.arc.descriptor, event);
    }

    /// Notify that a tool moved above a tablet.
    ///
    /// You provide the new location of the tool, in the form of:
    /// - The coordinates of the tool in the global compositor space
    /// - The target on top of which the tool is, and the coordinates of its origin in the global
    ///   compositor space (or `None` if the tool is not above a client's target.).
    ///
    /// This will internally take care of notifying the appropriate client objects of
    /// proximity_in/motion/proximity_out events.
    pub fn motion(
        &self,
        data: &mut D,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        let mut inner = self.arc.inner.lock().unwrap();
        if !inner.in_proximity() {
            tracing::warn!("motion was called, but the tool hasn't entered tablet proximity.");
            return;
        }

        inner.pending_focus.clone_from(&focus);
        let seat = self.get_seat(data);

        inner.with_grab(data, &seat, &self.arc.descriptor, |data, handle, grab| {
            grab.motion(data, handle, focus, event);
        });
    }

    /// Notify that a tool start touching the tablet surface.
    ///
    /// This will internally send the appropriate down event to the client objects matching the
    /// currently focused target.
    pub fn down(&self, data: &mut D, event: &DownEvent) {
        let mut inner = self.arc.inner.lock().unwrap();
        if !inner.in_proximity() {
            tracing::warn!("down was called, but the tool hasn't entered tablet proximity.");
            return;
        }

        let seat = self.get_seat(data);

        inner.with_grab(data, &seat, &self.arc.descriptor, |data, handle, grab| {
            grab.down(data, handle, event);
        });
    }

    /// Notify that a tool sopped touching the tablet surface.
    ///
    /// This will internally send the appropriate up event to the client objects matching the
    /// currently focused target.
    pub fn up(&self, data: &mut D, event: &UpEvent) {
        let mut inner = self.arc.inner.lock().unwrap();
        if !inner.in_proximity() {
            tracing::warn!("up was called, but the tool hasn't entered tablet proximity.");
            return;
        }

        let seat = self.get_seat(data);

        inner.with_grab(data, &seat, &self.arc.descriptor, |data, handle, grab| {
            grab.up(data, handle, event);
        });
    }

    /// Notify that a button state changed.
    ///
    /// This will internally send the appropriate button event to the client objects matching with
    /// the currently focused target.
    pub fn button(&self, data: &mut D, event: &ButtonEvent) {
        let mut inner = self.arc.inner.lock().unwrap();
        if !inner.in_proximity() {
            tracing::warn!("button was called, but the tool hasn't entered tablet proximity.");
            return;
        }

        let seat = self.get_seat(data);

        if matches!(event.state, ButtonState::Pressed) {
            inner.current_buttons.push(event.button);
        } else {
            inner.current_buttons.retain(|b| *b != event.button);
        }

        inner.with_grab(data, &seat, &self.arc.descriptor, |data, handle, grab| {
            grab.button(data, handle, event);
        });
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple axis events as if they happened in the same instance.
    pub fn axis(&self, data: &mut D, frame: AxisFrame) {
        let mut inner = self.arc.inner.lock().unwrap();
        if !inner.in_proximity() {
            tracing::warn!("axis was called, but the tool hasn't entered tablet proximity.");
            return;
        }

        let seat = self.get_seat(data);

        inner.with_grab(data, &seat, &self.arc.descriptor, |data, handle, grab| {
            grab.axis(data, handle, frame);
        });
    }

    /// End of a tool frame.
    ///
    /// A frame groups associated events. This terminates the frame.
    pub fn frame(&self, data: &mut D, time: u32) {
        let mut inner = self.arc.inner.lock().unwrap();
        if !inner.in_proximity() && !inner.frame_pending {
            tracing::warn!("frame was called, but the tool hasn't entered tablet proximity.");
            return;
        }

        // If we don't have a pending frame, skip.
        if !inner.frame_pending {
            return;
        }

        let seat = self.get_seat(data);

        inner.with_grab(data, &seat, &self.arc.descriptor, |data, handle, grab| {
            grab.frame(data, handle, time);
        });
    }

    /// Access the current location of this tablet tool in the global space
    ///
    /// This function will return None if the tool isn't above any tablet.
    pub fn current_location(&self) -> Option<Point<f64, Logical>> {
        self.arc.inner.lock().unwrap().location
    }

    /// Access the current tablet this tool is hovering over.
    ///
    /// This function will return None if the tool isn't above any tablet.
    pub fn current_tablet(&self) -> Option<Tablet> {
        self.arc.inner.lock().unwrap().tablet.clone()
    }

    /// Return the tablet tool descriptor.
    pub fn descriptor(&self) -> &TabletToolDescriptor {
        &self.arc.descriptor
    }

    fn get_seat(&self, data: &mut D) -> Seat<D> {
        let seat_state = data.seat_state();
        seat_state
            .seats
            .iter()
            .find(|seat| {
                let Some(tablet_seat) = seat.user_data().get::<TabletSeat<D>>() else {
                    return false;
                };

                tablet_seat.get_tool(&self.arc.descriptor).as_ref() == Some(self)
            })
            .cloned()
            .unwrap()
    }
}

/// Weak variant of a [`TabletToolHandle`]
///
/// Does not keep associated data alive, and can be used to refer to a potentially already destroyed
/// tablet tool.
#[derive(Debug)]
pub struct WeakTabletToolHandle<D: TabletSeatHandler>(Weak<TabletToolRc<D>>);

impl<D: TabletSeatHandler> Clone for WeakTabletToolHandle<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: TabletSeatHandler> WeakTabletToolHandle<D> {
    /// Try to retrieve the original `TabletToolHandle` if it still exists
    pub fn upgrade(&self) -> Option<TabletToolHandle<D>> {
        self.0.upgrade().map(|arc| TabletToolHandle { arc })
    }

    /// Check if this tablet tool is still alive
    pub fn is_alive(&self) -> bool {
        self.0.strong_count() != 0
    }
}

impl<D: TabletSeatHandler> TabletToolHandle<D> {
    /// Create a weak reference to this tablet tool
    pub fn downgrade(&self) -> WeakTabletToolHandle<D> {
        WeakTabletToolHandle(Arc::downgrade(&self.arc))
    }
}

/// Trait representing object that can receive tablet tool interactions
pub trait TabletToolTarget<D>: IsAlive + fmt::Debug + Send
where
    D: TabletSeatHandler + SeatHandler,
{
    /// A tool has entered proximity above this target.
    ///
    /// This event can be received when the tool has moved from one target to another, or when the
    /// tool has come back into proximity above the target.
    fn proximity_in(
        &self,
        seat: &Seat<D>,
        data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        tablet: &Tablet,
        serial: Serial,
    );

    /// A tool has left this target.
    fn proximity_out(&self, seat: &Seat<D>, data: &mut D, tool_descriptor: &TabletToolDescriptor);

    /// The tool is in contact with the surface of the tablet.
    fn down(&self, seat: &Seat<D>, data: &mut D, tool_descriptor: &TabletToolDescriptor, event: &DownEvent);
    /// The tool is no longer in contact with the surface of the tablet.
    fn up(&self, seat: &Seat<D>, data: &mut D, tool_descriptor: &TabletToolDescriptor, event: &UpEvent);

    /// A tool has moved over this target.
    fn motion(
        &self,
        seat: &Seat<D>,
        data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        event: &MotionEvent,
    );

    /// One or more of the tool axis has changed.
    fn axis(&self, seat: &Seat<D>, data: &mut D, tool_descriptor: &TabletToolDescriptor, frame: AxisFrame);

    /// One of the tool button has changed state.
    fn button(
        &self,
        seat: &Seat<D>,
        data: &mut D,
        tool_descriptor: &TabletToolDescriptor,
        event: &ButtonEvent,
    );

    /// End of a tablet tool frame.
    fn frame(&self, seat: &Seat<D>, data: &mut D, tool_descriptor: &TabletToolDescriptor, time: u32);
}

/// This inner handle is accessed from inside a tablet tool grab logic, and directly sends event to
/// the client.
pub struct TabletToolInnerHandle<'a, D: TabletSeatHandler> {
    inner: &'a mut TabletToolInternal<D>,
    descriptor: &'a TabletToolDescriptor,
    seat: &'a Seat<D>,
}

impl<D: TabletSeatHandler> fmt::Debug for TabletToolInnerHandle<'_, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabletToolInnerHandle")
            .field("inner", &self.inner)
            .field("descriptor", &self.descriptor)
            .field("seat", &self.seat)
            .finish()
    }
}

impl<D: TabletSeatHandler + 'static> TabletToolInnerHandle<'_, D> {
    /// Change the current grab on this tool to the provided grab
    ///
    /// Overwrites any current grab.
    pub fn set_grab<G: TabletToolGrab<D> + 'static>(
        &mut self,
        handler: &mut dyn TabletToolGrab<D>,
        data: &mut D,
        time: u32,
        serial: Serial,
        focus: Focus,
        grab: G,
    ) {
        handler.unset(data);
        self.inner
            .set_grab(data, self.seat, self.descriptor, grab, time, serial, focus);
    }

    /// Remove any current grab on this tool, resetting it to the default behavior
    ///
    /// This will also restore the focus of the tool if `restore_focus` is true.
    pub fn unset_grab(
        &mut self,
        handler: &mut dyn TabletToolGrab<D>,
        data: &mut D,
        serial: Serial,
        time: u32,
        restore_focus: bool,
    ) {
        handler.unset(data);
        self.inner
            .unset_grab(data, self.seat, self.descriptor, serial, time, restore_focus);
    }

    /// Access the current focus of this pointer
    pub fn current_focus(&self) -> Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)> {
        self.inner.focus.clone()
    }

    /// Access the current location of this tablet tool in the global space
    pub fn current_location(&self) -> Point<f64, Logical> {
        self.inner.location.unwrap()
    }

    /// Access the current tablet this tool is hovering over.
    pub fn current_tablet(&self) -> Tablet {
        self.inner.tablet.clone().unwrap()
    }

    /// A list of the currently physically pressed buttons
    ///
    /// This still includes buttons that your grab have intercepted and not sent to the target.
    pub fn current_pressed(&self) -> &[u32] {
        &self.inner.current_buttons
    }

    /// Notify that a tool entered proximity with a tablet.
    ///
    /// You provide the location of the tool in the form of:
    /// - The coordinates of the tool in the global compositor space
    /// - The surface on top of which the tool is, and the coordinates of its origin in the global
    ///   compositor space (or `None` if the tool is not over a client surface).
    ///
    /// This will internally generate the following event to the client object, if any:
    /// - ProximityIn
    /// - Axis, if an axis frame is available
    /// - Motion
    pub fn proximity_in(
        &mut self,
        data: &mut D,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &ProximityInEvent,
    ) {
        self.inner
            .proximity_in(data, self.seat, self.descriptor, focus, event);
    }

    /// Notify that a tool left physical proximity with a tablet.
    ///
    /// This will internally generate the following events:
    /// - Button release, for any pressed button
    /// - Up, if the tool tip was down
    /// - ProximityOut
    pub fn proximity_out(&mut self, data: &mut D, event: &ProximityOutEvent) {
        self.inner.proximity_out(data, self.seat, self.descriptor, event);
    }

    /// Notify that a tool moved above a tablet.
    ///
    /// You provide the new location of the tool, in the form of:
    /// - The coordinates of the tool in the global compositor space
    /// - The target on top of which the tool is, and the coordinates of its origin in the global
    ///   compositor space (or `None` if the tool is not above a client's target.).
    ///
    /// This will internally take care of notifying the appropriate client objects of
    /// proximity_in/motion/proximity_out events.
    pub fn motion(
        &mut self,
        data: &mut D,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        self.inner.motion(data, self.seat, self.descriptor, focus, event);
    }

    /// Notify that a tool start touching the tablet surface.
    ///
    /// This will internally send the appropriate down event to the client objects matching the
    /// currently focused target.
    pub fn down(&mut self, data: &mut D, event: &DownEvent) {
        self.inner.down(data, self.seat, self.descriptor, event);
    }

    /// Notify that a tool sopped touching the tablet surface.
    ///
    /// This will internally send the appropriate up event to the client objects matching the
    /// currently focused target.
    pub fn up(&mut self, data: &mut D, event: &UpEvent) {
        self.inner.up(data, self.seat, self.descriptor, event);
    }

    /// Notify that a button state changed.
    ///
    /// This will internally send the appropriate button event to the client objects matching with
    /// the currently focused target.
    pub fn button(&mut self, data: &mut D, event: &ButtonEvent) {
        self.inner.button(data, self.seat, self.descriptor, event);
    }

    /// Start an axis frame
    ///
    /// A single frame will group multiple axis events as if they happened in the same instance.
    pub fn axis(&mut self, data: &mut D, frame: AxisFrame) {
        self.inner.axis(data, self.seat, self.descriptor, frame);
    }

    /// End of a tool frame.
    ///
    /// A frame groups associated events. This terminates the frame.
    pub fn frame(&mut self, data: &mut D, time: u32) {
        self.inner.frame(data, self.seat, self.descriptor, time);
    }

    /// Return the tablet tool descriptor.
    pub fn descriptor(&self) -> &TabletToolDescriptor {
        self.descriptor
    }
}

pub(crate) struct TabletToolInternal<D: TabletSeatHandler> {
    pub(crate) focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
    pending_focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
    previous_focus: Option<<D as TabletSeatHandler>::ToolFocus>,

    location: Option<Point<f64, Logical>>,
    tablet: Option<Tablet>,
    current_buttons: Vec<u32>,
    pressed_buttons: Vec<u32>,
    tip_down: bool,
    frame_pending: bool,

    default_grab: Box<dyn Fn() -> Box<dyn TabletToolGrab<D>> + Send + 'static>,
    grab: GrabStatus<dyn TabletToolGrab<D>>,
}

impl<D: TabletSeatHandler> fmt::Debug for TabletToolInternal<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabletToolHandle")
            .field("focus", &self.focus)
            .field("grab", &self.grab)
            .finish()
    }
}

impl<D: TabletSeatHandler + 'static> TabletToolInternal<D> {
    pub(crate) fn new<F>(default_grab: F) -> Self
    where
        F: Fn() -> Box<dyn TabletToolGrab<D>> + Send + 'static,
    {
        Self {
            focus: Default::default(),
            pending_focus: Default::default(),
            previous_focus: Default::default(),

            location: None,
            tablet: None,
            current_buttons: Default::default(),
            pressed_buttons: Default::default(),
            tip_down: false,
            frame_pending: false,

            default_grab: Box::new(default_grab),
            grab: GrabStatus::None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn set_grab<G: TabletToolGrab<D> + 'static>(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        descriptor: &TabletToolDescriptor,
        grab: G,
        time: u32,
        serial: Serial,
        focus: Focus,
    ) {
        if let GrabStatus::Active(_, handler) = &mut self.grab {
            handler.unset(data)
        }

        if self.tablet.is_none() || self.location.is_none() {
            tracing::warn!("set_grab called with an out of proximity tool.");
            return;
        }

        self.grab = GrabStatus::Active(serial, Box::new(grab));

        if matches!(focus, Focus::Clear) {
            let location = self.location.unwrap();

            self.motion(
                data,
                seat,
                descriptor,
                None,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            );
        }
    }

    fn unset_grab(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        descriptor: &TabletToolDescriptor,
        serial: Serial,
        time: u32,
        restore_focus: bool,
    ) {
        if let GrabStatus::Active(_, handler) = &mut self.grab {
            handler.unset(data)
        }
        self.grab = GrabStatus::None;

        if let Some(location) = self.location {
            if restore_focus {
                let focus = self.pending_focus.clone();

                self.motion(
                    data,
                    seat,
                    descriptor,
                    focus,
                    &MotionEvent {
                        location,
                        serial,
                        time,
                    },
                );
            }
        }
    }

    fn with_grab<F>(&mut self, data: &mut D, seat: &Seat<D>, descriptor: &TabletToolDescriptor, f: F)
    where
        F: FnOnce(&mut D, &mut TabletToolInnerHandle<'_, D>, &mut dyn TabletToolGrab<D>),
    {
        let mut grab = std::mem::replace(&mut self.grab, GrabStatus::Borrowed);
        match grab {
            GrabStatus::Borrowed => {
                panic!("Accessed a tablet tool grab from within a tablet tool grab access.")
            }
            GrabStatus::Active(_, ref mut handler) => {
                if let Some((ref focus, _)) = handler.start_data().focus {
                    if !focus.alive() {
                        handler.unset(data);
                        self.grab = GrabStatus::None;
                        let mut default_grab = (self.default_grab)();
                        f(
                            data,
                            &mut TabletToolInnerHandle {
                                inner: self,
                                descriptor,
                                seat,
                            },
                            &mut *default_grab,
                        );
                        return;
                    }
                }

                f(
                    data,
                    &mut TabletToolInnerHandle {
                        inner: self,
                        descriptor,
                        seat,
                    },
                    &mut **handler,
                );
            }
            GrabStatus::None => {
                let mut default_grab = (self.default_grab)();
                f(
                    data,
                    &mut TabletToolInnerHandle {
                        inner: self,
                        descriptor,
                        seat,
                    },
                    &mut *default_grab,
                );
            }
        }

        if let GrabStatus::Borrowed = self.grab {
            self.grab = grab;
        }
    }

    fn in_proximity(&self) -> bool {
        self.tablet.is_some() && self.location.is_some()
    }

    fn proximity_in(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        descriptor: &TabletToolDescriptor,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &ProximityInEvent,
    ) {
        self.focus = focus;
        if let Some((focused, loc)) = self.focus.as_mut() {
            let location = event.location - *loc;
            let serial = event.serial;
            let time = event.time;

            focused.proximity_in(seat, data, descriptor, self.tablet.as_ref().unwrap(), serial);

            if let Some(axis) = event.axis.clone() {
                focused.axis(seat, data, descriptor, axis);
            }

            focused.motion(
                seat,
                data,
                descriptor,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            );

            if self.tip_down {
                focused.down(seat, data, descriptor, &DownEvent { serial, time });
            }

            for button in self.pressed_buttons.iter() {
                focused.button(
                    seat,
                    data,
                    descriptor,
                    &ButtonEvent {
                        serial: event.serial,
                        button: *button,
                        state: ButtonState::Pressed,
                        time: event.time,
                    },
                );
            }
        }

        self.frame_pending = true;
    }

    fn proximity_out(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        descriptor: &TabletToolDescriptor,
        event: &ProximityOutEvent,
    ) {
        if let Some((focused, _)) = self.focus.take() {
            let time = event.time;

            for button in self.pressed_buttons.iter() {
                focused.button(
                    seat,
                    data,
                    descriptor,
                    &ButtonEvent {
                        serial: event.serial,
                        button: *button,
                        state: ButtonState::Released,
                        time,
                    },
                );
            }

            if self.tip_down {
                focused.up(
                    seat,
                    data,
                    descriptor,
                    &UpEvent {
                        serial: event.serial,
                        time,
                    },
                );
            }

            focused.proximity_out(seat, data, descriptor);

            // This should not happen, but in case a previous proximity_out wasn't acknowledged by a
            // frame, we need to send one now.
            if let Some(old_focus) = self.previous_focus.take_if(|f| f != &focused) {
                old_focus.frame(seat, data, descriptor, time);
            }

            self.previous_focus = Some(focused);
        }

        self.frame_pending = true;
    }

    fn motion(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        descriptor: &TabletToolDescriptor,
        focus: Option<(<D as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        self.location = Some(event.location);

        let Some((focused, loc)) = focus else {
            self.proximity_out(
                data,
                seat,
                descriptor,
                &ProximityOutEvent {
                    serial: event.serial,
                    time: event.time,
                },
            );
            return;
        };

        if self
            .focus
            .as_ref()
            .is_some_and(|(old_focused, _)| &focused == old_focused)
        {
            let event = MotionEvent {
                location: event.location - loc,
                serial: event.serial,
                time: event.time,
            };
            // We were on top of a target and remained on it.
            focused.motion(seat, data, descriptor, &event);
        } else {
            self.proximity_out(
                data,
                seat,
                descriptor,
                &ProximityOutEvent {
                    serial: event.serial,
                    time: event.time,
                },
            );
            self.proximity_in(
                data,
                seat,
                descriptor,
                Some((focused, loc)),
                &ProximityInEvent {
                    location: event.location,
                    axis: None,
                    serial: event.serial,
                    time: event.time,
                },
            );
        }

        self.frame_pending = true;
    }

    fn down(&mut self, data: &mut D, seat: &Seat<D>, descriptor: &TabletToolDescriptor, event: &DownEvent) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.down(seat, data, descriptor, event);
        }

        self.tip_down = true;
        self.frame_pending = true;
    }

    fn up(&mut self, data: &mut D, seat: &Seat<D>, descriptor: &TabletToolDescriptor, event: &UpEvent) {
        if self.tip_down {
            if let Some((focused, _)) = self.focus.as_mut() {
                focused.up(seat, data, descriptor, event);
            }
        }

        self.tip_down = false;
        self.frame_pending = true;
    }

    fn button(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        descriptor: &TabletToolDescriptor,
        event: &ButtonEvent,
    ) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.button(seat, data, descriptor, event);
        }

        if matches!(event.state, ButtonState::Pressed) {
            self.pressed_buttons.push(event.button);
        } else {
            self.pressed_buttons.retain(|b| b != &event.button);
        }

        self.frame_pending = true;
    }

    fn axis(&mut self, data: &mut D, seat: &Seat<D>, descriptor: &TabletToolDescriptor, frame: AxisFrame) {
        if let Some((focused, _)) = self.focus.as_mut() {
            focused.axis(seat, data, descriptor, frame);
        }

        self.frame_pending = true;
    }

    fn frame(&mut self, data: &mut D, seat: &Seat<D>, descriptor: &TabletToolDescriptor, time: u32) {
        if self.frame_pending {
            if let Some((focused, _)) = self.focus.as_mut() {
                focused.frame(seat, data, descriptor, time);

                // We don't want to send the frame twice.
                self.previous_focus.take_if(|f| f == focused);
            }

            if let Some(focused) = self.previous_focus.take() {
                focused.frame(seat, data, descriptor, time);
            }
        }

        self.frame_pending = false;
    }

    fn proximity_out_impl(
        &mut self,
        data: &mut D,
        seat: &Seat<D>,
        descriptor: &TabletToolDescriptor,
        event: &ProximityOutEvent,
    ) {
        self.with_grab(data, seat, descriptor, |data, handle, grab| {
            grab.proximity_out(data, handle, event);
        });

        // Now, reset everything so we don't get stale data later.
        self.tip_down = false;
        self.pressed_buttons.clear();
        self.current_buttons.clear();
        self.focus = None;
        self.pending_focus = None;
        // We don't reset last_focus just yet as it's needed for the frame event.
        self.tablet = None;
        self.location = None;
    }
}

/// Proximity in event.
#[derive(Debug, Clone)]
pub struct ProximityInEvent {
    /// Location of the tool in compositor space
    pub location: Point<f64, Logical>,
    /// Axis frame to send along with the proximity in
    pub axis: Option<AxisFrame>,
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Proximity out event.
#[derive(Debug, Clone)]
pub struct ProximityOutEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Tablet tool motion event
#[derive(Debug, Clone)]
pub struct MotionEvent {
    /// Location of the tool in compositor space
    pub location: Point<f64, Logical>,
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Tablet tool button event
///
/// Tool button click and release event. The location of the click is given by the last motion or
/// proximity in event.
#[derive(Debug, Clone)]
pub struct ButtonEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Button that produced the event
    pub button: u32,
    /// Physical state of the button
    pub state: ButtonState,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Tip down event.
///
/// The location of the tool is given by the last motion or proximity in event.
#[derive(Debug, Clone)]
pub struct DownEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Tip up event.
///
/// The location of the tool is given by the last motion or proximity in event.
#[derive(Debug, Clone)]
pub struct UpEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// A frame of tablet tool axis events.
/// Frames of axis events should be considered as one logical action.
///
/// Can be used with the builder pattern, e.g.:
///
/// ```ignore
/// AxisFrame::new()
///     .pressure(0.5)
///     .distance(0.0)
///     .tilt(15.0, 25.0);
/// ```
#[must_use = "AxisFrame uses a builder-like pattern, so its result must be used"]
#[derive(Clone, Debug, Default)]
pub struct AxisFrame {
    /// Changes in the pressure axis of the tool
    pub pressure: Option<f64>,
    /// Changes in the distance axis
    pub distance: Option<f64>,
    /// Changes in the tool tilt
    pub tilt: Option<(f64, f64)>,
    /// Changes in the tool's z-rotation
    pub rotation: Option<f64>,
    /// Changes in the tool's slider position
    pub slider: Option<f64>,
    /// Changes in the tool's wheel position
    pub wheel: Option<(f64, i32)>,
}

impl AxisFrame {
    /// Create a new frame of axis events
    pub fn new() -> Self {
        Default::default()
    }

    /// Value of the pressure event.
    pub fn pressure(self, pressure: f64) -> Self {
        Self {
            pressure: Some(pressure),
            ..self
        }
    }

    /// Value of the distance event.
    pub fn distance(self, distance: f64) -> Self {
        Self {
            distance: Some(distance),
            ..self
        }
    }

    /// Value of the tool tilt
    pub fn tilt(self, tilt_x: f64, tilt_y: f64) -> Self {
        Self {
            tilt: Some((tilt_x, tilt_y)),
            ..self
        }
    }

    /// Value in the tool's z-rotation
    pub fn rotation(self, rotation: f64) -> Self {
        Self {
            rotation: Some(rotation),
            ..self
        }
    }

    /// Value of the tool's slider position
    pub fn slider(self, slider: f64) -> Self {
        Self {
            slider: Some(slider),
            ..self
        }
    }

    /// Value of the tool's wheel
    pub fn wheel(self, degrees: f64, clicks: i32) -> Self {
        Self {
            wheel: Some((degrees, clicks)),
            ..self
        }
    }
}
