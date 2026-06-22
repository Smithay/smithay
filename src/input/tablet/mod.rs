//! Graphic tablet related types for smithay's input abstraction
//!
//! This module provides some types loosely resembling instances of wayland tablet seat, tablet and
//! tablet tool. It is however not directly tied to wayland and can be used to multiplex various
//! tablet-related operations between different handlers.
//!
//! If the `wayland_frontend`-feature is enabled, the `smithay::wayland::tablet_manager`-module
//! provides additional functionality for the provided types of this module to map them to
//! advertised wayland globals and objects.
//!
//! ## How to use it
//!
//! To start using this module, you need to create a [`TabletSeat`] from an existing [`Seat`].
//! Additionally, you want to implement the [`TabletSeatHandler`] trait.
//!
//! ### Initialization
//!
//! ```
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! use smithay::input::tablet::{TabletSeatHandler, TabletSeatTrait};
//! use smithay::backend::input::TabletToolDescriptor;
//! # use smithay::backend::input::KeyState;
//! # use smithay::input::{
//! #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent, RelativeMotionEvent,
//! #             GestureSwipeBeginEvent, GestureSwipeUpdateEvent, GestureSwipeEndEvent,
//! #             GesturePinchBeginEvent, GesturePinchUpdateEvent, GesturePinchEndEvent,
//! #             GestureHoldBeginEvent, GestureHoldEndEvent},
//! #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
//! #   touch::{DownEvent, UpEvent, MotionEvent as TouchMotionEvent, ShapeEvent, OrientationEvent, TouchTarget, FrameMarker},
//! #   tablet::{Tablet, tool::{self, TabletToolTarget}},
//! # };
//! # use smithay::utils::{IsAlive, Serial};
//!
//! struct State {
//!     seat_state: SeatState<Self>,
//!     // ...
//! };
//!
//! let mut seat_state = SeatState::<State>::new();
//!
//! // create the seat
//! let seat = seat_state.new_seat(
//!     "seat-0",  // the name of the seat, will be advertized to clients
//! );
//! // create the associated tablet seat.
//! let tablet_seat = seat.tablet_seat();
//!
//! # #[derive(Debug, Clone, PartialEq)]
//! # struct Target;
//! # impl IsAlive for Target {
//! #   fn alive(&self) -> bool { true }
//! # }
//! # impl PointerTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {}
//! #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
//! #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
//! #   fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {}
//! #   fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {}
//! #   fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {}
//! #   fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {}
//! #   fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {}
//! #   fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {}
//! #   fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {}
//! #   fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {}
//! # }
//! # impl KeyboardTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {}
//! #   fn key(
//! #       &self,
//! #       seat: &Seat<State>,
//! #       data: &mut State,
//! #       key: KeysymHandle<'_>,
//! #       state: KeyState,
//! #       serial: Serial,
//! #       time: u32,
//! #   ) {}
//! #   fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {}
//! # }
//! # impl TouchTarget<State> for Target {
//! #   fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent) {}
//! #   fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &TouchMotionEvent) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State, marker: FrameMarker) {}
//! #   fn cancel(&self, seat: &Seat<State>, data: &mut State, marker: FrameMarker) {}
//! #   fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent) {}
//! #   fn orientation(&self, seat: &Seat<State>, data: &mut State, event: &OrientationEvent) {}
//! #   fn last_frame(&self, seat: &Seat<State>, data: &mut State) -> Option<FrameMarker> { unimplemented!() }
//! # }
//! # impl TabletToolTarget<State> for Target {
//! #   fn proximity_in(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor, tablet: &Tablet, serial: Serial) {}
//! #   fn proximity_out(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor) {}
//! #   fn down(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor, event: &tool::DownEvent) {}
//! #   fn up(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor, event: &tool::UpEvent) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor, event: &tool::MotionEvent) {}
//! #   fn axis(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor, frame: tool::AxisFrame) {}
//! #   fn button(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor, event: &tool::ButtonEvent) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State, tool_descriptor: &TabletToolDescriptor, time: u32) {}
//! # }
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = Target;
//!     type PointerFocus = Target;
//!     type TouchFocus = Target;
//!
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Target>) {
//!         // handle focus changes, if you need to ...
//!     }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
//!         // handle new images for the cursor ...
//!     }
//! }
//!
//! impl TabletSeatHandler for State {
//!     type ToolFocus = Target;
//!
//!     fn tablet_tool_image(&mut self, tool: &TabletToolDescriptor, image: CursorImageStatus) {
//!         // handle new image for the given tool.
//!     }
//! }
//! ```
//!
//! ### Run usage
//!
//! Once the tablet seat is initialized, you can add tablet and tools to it.
//!
//! Currently, pads are unsupported by this module.
//!
//! You can add tablet and tools via methods of the [`TabletSeat`] struct:
//! [`TabletSeat::add_tablet`] and [`TabletSeat::add_tool`].
//! These method return handles that can be cloned and sent across thread, so you can keep them
//! around in you event-handling code to forward inputs to your clients.
//!

use std::{
    collections::HashMap,
    fmt,
    hash::Hash,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};

use crate::{
    backend::input::{Device, TabletToolDescriptor},
    input::{
        Seat, SeatHandler,
        pointer::CursorImageStatus,
        tablet::tool::{TabletToolGrab, TabletToolHandle, TabletToolTarget},
    },
};

#[cfg(feature = "wayland_frontend")]
use wayland_server::Weak as WlWeak;

pub mod tool;

/// Description of graphics tablet device
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct TabletDescriptor {
    /// Tablet device name
    pub name: String,
    /// Tablet device USB (product,vendor) id
    pub usb_id: Option<(u32, u32)>,
    /// Path to the device
    pub syspath: Option<PathBuf>,
}

impl<D: Device> From<&D> for TabletDescriptor {
    #[inline]
    fn from(device: &D) -> Self {
        TabletDescriptor {
            name: device.name(),
            syspath: device.syspath(),
            usb_id: device.usb_id(),
        }
    }
}

pub(crate) struct TabletRc {
    pub(crate) descriptor: TabletDescriptor,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) wp_tablet: crate::wayland::tablet_manager::tablet::WpTabletHandle,
}

#[cfg(not(feature = "wayland_frontend"))]
impl fmt::Debug for TabletRc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabletRc")
            .field("descriptor", &self.descriptor)
            .finish()
    }
}

#[cfg(feature = "wayland_frontend")]
impl fmt::Debug for TabletRc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabletRc")
            .field("descriptor", &self.descriptor)
            .field("wp_tablet", &self.wp_tablet)
            .finish()
    }
}

/// Handle to a tablet device
///
/// Tablet represents one graphics tablet device
pub struct Tablet {
    pub(crate) arc: Arc<TabletRc>,
}

impl Tablet {
    pub(super) fn new(descriptor: TabletDescriptor) -> Self {
        Self {
            arc: Arc::new(TabletRc {
                descriptor,
                #[cfg(feature = "wayland_frontend")]
                wp_tablet: Default::default(),
            }),
        }
    }

    /// Get a descriptor for this tablet
    pub fn descriptor(&self) -> &TabletDescriptor {
        &self.arc.descriptor
    }
}

impl fmt::Debug for Tablet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tablet").field("arc", &self.arc).finish()
    }
}

impl Clone for Tablet {
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl PartialEq for Tablet {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}

impl Eq for Tablet {}

impl Hash for Tablet {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.arc).hash(state)
    }
}

/// Weak variant of a [`Tablet`]
///
/// Does not keep associated data alive, and can be used to refer to a potentially already destroyed
/// tablet.
#[derive(Debug)]
pub struct WeakTablet(Weak<TabletRc>);

impl Clone for WeakTablet {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl WeakTablet {
    /// Try to retrieve the original `Tablet` if it still exists
    pub fn upgrade(&self) -> Option<Tablet> {
        self.0.upgrade().map(|arc| Tablet { arc })
    }

    /// Check if this tablet is still alive
    pub fn is_alive(&self) -> bool {
        self.0.strong_count() != 0
    }
}

impl Tablet {
    /// Create a weak reference to this tablet
    pub fn downgrade(&self) -> WeakTablet {
        WeakTablet(Arc::downgrade(&self.arc))
    }
}

/// Extends [`Seat`] with graphic tablet specific functionality
pub trait TabletSeatTrait<D: TabletSeatHandler> {
    /// Get tablet seat associated with this seat
    fn tablet_seat(&self) -> TabletSeat<D>;
}

/// Extends [`Seat`] with graphic tablet specific functionality.
impl<D: SeatHandler + TabletSeatHandler + 'static> TabletSeatTrait<D> for Seat<D> {
    fn tablet_seat(&self) -> TabletSeat<D> {
        let user_data = self.user_data();
        user_data.get_or_insert(TabletSeat::default).clone()
    }
}

/// Handler trait for Tablet Seats
pub trait TabletSeatHandler: SeatHandler + Sized {
    /// Type used to represent the target currently holding the tablet's tool focus
    type ToolFocus: TabletToolTarget<Self> + PartialEq + Clone + 'static;

    /// Callback that will be notified whenever a client requests to set a custom tool image.
    fn tablet_tool_image(&mut self, tool: &TabletToolDescriptor, image: CursorImageStatus) {
        let _ = tool;
        let _ = image;
    }
}

/// Handle to a tablet seat
///
/// TabletSeat extends [`Seat`] with graphic tablet specific functionality. They can be used to
/// advertise available graphics tablets and tools.
pub struct TabletSeat<D: TabletSeatHandler> {
    pub(crate) arc: Arc<Mutex<Inner<D>>>,
}

impl<D: TabletSeatHandler> Default for TabletSeat<D> {
    fn default() -> Self {
        Self {
            arc: Default::default(),
        }
    }
}

impl<D: TabletSeatHandler> Clone for TabletSeat<D> {
    fn clone(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }
}

impl<D: TabletSeatHandler> fmt::Debug for TabletSeat<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabletSeat").field("arc", &self.arc).finish()
    }
}

impl<D: TabletSeatHandler> PartialEq for TabletSeat<D> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.arc, &other.arc)
    }
}

impl<D: TabletSeatHandler> Eq for TabletSeat<D> {}

impl<D: TabletSeatHandler> Hash for TabletSeat<D> {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.arc).hash(state)
    }
}

/// Weak variant of a [`TabletSeat`].
///
/// Does not keep associated user data alive, and can be used to refer to a potentially already
/// destroyed seat.
#[derive(Debug)]
pub struct WeakTabletSeat<D: TabletSeatHandler>(Weak<Mutex<Inner<D>>>);

impl<D: TabletSeatHandler> Clone for WeakTabletSeat<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: TabletSeatHandler> WeakTabletSeat<D> {
    /// Try to retrieve the original `TabletSeat`, if it still exists
    pub fn upgrade(&self) -> Option<TabletSeat<D>> {
        self.0.upgrade().map(|arc| TabletSeat { arc })
    }

    /// Check if the tablet seat is still alive
    pub fn is_alive(&self) -> bool {
        self.0.strong_count() != 0
    }
}

pub(crate) struct Inner<D: TabletSeatHandler> {
    pub(crate) tablets: HashMap<TabletDescriptor, Tablet>,
    pub(crate) tools: HashMap<TabletToolDescriptor, TabletToolHandle<D>>,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) instances:
        Vec<WlWeak<wayland_protocols::wp::tablet::zv2::server::zwp_tablet_seat_v2::ZwpTabletSeatV2>>,
}

#[cfg(not(feature = "wayland_frontend"))]
impl<D: TabletSeatHandler> fmt::Debug for Inner<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("tablets", &self.tablets)
            .field("tools", &self.tools)
            .finish()
    }
}

#[cfg(feature = "wayland_frontend")]
impl<D: TabletSeatHandler> fmt::Debug for Inner<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("tablets", &self.tablets)
            .field("tools", &self.tools)
            .field("instances", &self.instances)
            .finish()
    }
}

impl<D: TabletSeatHandler> Default for Inner<D> {
    fn default() -> Self {
        Self {
            tablets: HashMap::default(),
            tools: HashMap::default(),
            #[cfg(feature = "wayland_frontend")]
            instances: Vec::default(),
        }
    }
}

impl<D: TabletSeatHandler + 'static> Inner<D> {
    pub(crate) fn add_tablet(
        &mut self,
        tablet_desc: &TabletDescriptor,
        default: impl FnOnce(TabletDescriptor) -> Tablet,
    ) -> Tablet {
        self.tablets.remove(tablet_desc);

        self.tablets
            .entry(tablet_desc.clone())
            .or_insert_with(|| default(tablet_desc.clone()))
            .clone()
    }

    pub(crate) fn add_tool<F>(
        &mut self,
        tool_desc: &TabletToolDescriptor,
        default_grab: F,
        default: impl FnOnce(TabletToolDescriptor, F) -> TabletToolHandle<D>,
    ) -> TabletToolHandle<D>
    where
        F: Fn() -> Box<dyn TabletToolGrab<D>> + Send + 'static,
    {
        self.tools.remove(tool_desc);

        self.tools
            .entry(tool_desc.clone())
            .or_insert_with(|| default(tool_desc.clone(), default_grab))
            .clone()
    }
}

impl<D: TabletSeatHandler + 'static> TabletSeat<D> {
    /// Add a new tablet to a seat.
    ///
    /// You can either add tablet on [DeviceAdded] event, or you can add tablet based
    /// on tool event, then clients will not know about devices that are not being used.
    ///
    /// If the tablet was already known it removes it and recreate a new handle. Because
    /// [`TabletToolHandle`] will keep a handle to the [`Tablet`] while in proximity, it may appears
    /// to clients that the tablet hasn't been removed until the tool leave its proximity.
    ///
    /// [DeviceAdded]: crate::backend::input::InputEvent::DeviceAdded
    pub fn add_tablet(&self, tablet_desc: &TabletDescriptor) -> Tablet {
        self.arc.lock().unwrap().add_tablet(tablet_desc, Tablet::new)
    }

    /// Get a handler to a tablet
    pub fn get_tablet(&self, tablet_desc: &TabletDescriptor) -> Option<Tablet> {
        self.arc.lock().unwrap().tablets.get(tablet_desc).cloned()
    }

    /// Count all tablet devices
    pub fn count_tablets(&self) -> usize {
        self.arc.lock().unwrap().tablets.len()
    }

    /// Remove tablet device
    ///
    /// Called when tablet is no longer available, for example on [DeviceRemoved] event.
    ///
    /// [DeviceRemoved]: crate::backend::input::InputEvent::DeviceRemoved
    pub fn remove_tablet(&self, tablet_desc: &TabletDescriptor) {
        self.arc.lock().unwrap().tablets.remove(tablet_desc);
    }

    /// Remove all tablet devices
    pub fn clear_tablets(&self) {
        self.arc.lock().unwrap().tablets.clear();
    }

    /// Add a new tool to a seat.
    ///
    /// Tool are usually added on [TabletToolProximityEvent] event.
    ///
    /// Calling this method on a seat that already has the same tool will overwrite it, and will be
    /// seen by clients as if the tool was removed and a new one was added.
    ///
    /// [TabletToolProximityEvent]: crate::backend::input::InputEvent::TabletToolProximity
    pub fn add_tool(&self, tool_desc: &TabletToolDescriptor) -> TabletToolHandle<D> {
        self.add_tool_with_default_grab(tool_desc, || Box::new(tool::DefaultGrab))
    }

    /// Add a new tool to a seat and allows the use of a custom default [`TabletToolGrab`]
    ///
    /// The default ghrab is used in case no other grab is currently active. When using
    /// [`TabletSeat::add_tool`], it will use [`tool::DefaultGrab`] which will install
    /// [`tool::DownGrab`] on a down event. [`tool::DownGrab`] makes sure all further event will use
    /// the same target until an up or physical proximity out event.
    ///
    /// See [`TabletSeat::add_tool`] for more information.
    pub fn add_tool_with_default_grab<F>(
        &self,
        tool_desc: &TabletToolDescriptor,
        default_grab: F,
    ) -> TabletToolHandle<D>
    where
        F: Fn() -> Box<dyn TabletToolGrab<D>> + Send + 'static,
    {
        self.arc
            .lock()
            .unwrap()
            .add_tool(tool_desc, default_grab, TabletToolHandle::new)
    }

    /// Get a handle to a tablet tool.
    pub fn get_tool(&self, tool_desc: &TabletToolDescriptor) -> Option<TabletToolHandle<D>> {
        self.arc.lock().unwrap().tools.get(tool_desc).cloned()
    }

    /// Count all tablet tool devices
    pub fn count_tools(&self) -> usize {
        self.arc.lock().unwrap().tools.len()
    }

    /// Run a callback on all available tablet tools
    pub fn with_tools<T>(
        &self,
        callback: impl FnOnce(&HashMap<TabletToolDescriptor, TabletToolHandle<D>>) -> T,
    ) -> T {
        let guard = self.arc.lock().unwrap();

        callback(&guard.tools)
    }

    /// Remove tablet tool device
    ///
    /// Policy of tool removal is a compositor-specific.
    ///
    /// One possible policy would be to remove a tool when all tablets the tool was used on are removed.
    pub fn remove_tool(&self, tool_desc: &TabletToolDescriptor) {
        self.arc.lock().unwrap().tools.remove(tool_desc);
    }

    /// Remove all tablet tool devices
    pub fn clear_tools(&self) {
        self.arc.lock().unwrap().tools.clear();
    }
}
