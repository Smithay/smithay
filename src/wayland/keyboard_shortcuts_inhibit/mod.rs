//! Protocol for inhibiting the compositor keyboard shortcuts
//!
//! This protocol specifies a way for a client to request the compositor
//! to ignore its own keyboard shortcuts for a given seat,
//! so that all key events from that seat get forwarded to a surface.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::atomic};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::{
    zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
    zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
};
use wayland_server::{
    backend::{GlobalId, ObjectId},
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    Dispatch, DisplayHandle, GlobalDispatch, Resource,
};

mod dispatch;
pub use dispatch::KeyboardShortcutsInhibitorUserData;

use crate::input::{Seat, SeatHandler};

type SeatId = ObjectId;

/// List of inhibitors associated with WlSeat
#[derive(Debug, Default)]
struct SeatInhibitors(Vec<KeyboardShortcutsInhibitor>);

impl SeatInhibitors {
    fn push(&mut self, value: KeyboardShortcutsInhibitor) {
        self.0.push(value)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Check if any inhibitor is active
    fn is_inhibited(&self) -> bool {
        self.0.iter().any(|i| i.is_active())
    }

    fn remove(&mut self, id: ObjectId) -> Option<KeyboardShortcutsInhibitor> {
        self.0
            .iter()
            .position(|i| i.inhibitor.id() == id)
            .map(|id| self.0.remove(id))
    }

    fn surface_has_inhibitor(&self, surface: &WlSurface) -> bool {
        self.inhibitor_for_surface(surface).is_some()
    }

    /// Find inhibitor_for WlSurface
    fn inhibitor_for_surface(&self, surface: &WlSurface) -> Option<&KeyboardShortcutsInhibitor> {
        self.0.iter().find(|i| i.wl_surface() == surface)
    }
}

/// Delegate type for KeyboardShortcutsInhibit global.
#[derive(Debug)]
pub struct KeyboardShortcutsInhibitState {
    manager_global: GlobalId,
    inhibitors: HashMap<SeatId, Rc<RefCell<SeatInhibitors>>>,
}

impl KeyboardShortcutsInhibitState {
    /// Regiseter new [ZwpKeyboardShortcutsInhibitManagerV1] global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()>,
        D: Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()>,
        D: Dispatch<ZwpKeyboardShortcutsInhibitorV1, KeyboardShortcutsInhibitorUserData>,
        D: 'static,
    {
        let manager_global = display.create_global::<D, ZwpKeyboardShortcutsInhibitManagerV1, _>(1, ());
        Self {
            manager_global,
            inhibitors: HashMap::new(),
        }
    }

    /// [ZwpKeyboardShortcutsInhibitManagerV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.manager_global.clone()
    }
}

/// Context object for keyboard shortcuts inhibitor
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardShortcutsInhibitor {
    inhibitor: ZwpKeyboardShortcutsInhibitorV1,
}

impl KeyboardShortcutsInhibitor {
    fn data(&self) -> &KeyboardShortcutsInhibitorUserData {
        self.inhibitor.data().unwrap()
    }

    fn set_is_active(&self, v: bool) {
        self.data().is_active.store(v, atomic::Ordering::Release);
    }

    /// Seat that is beeing inhibited
    fn seat_id(&self) -> &SeatId {
        &self.data().seat
    }

    /// Seat that is beeing inhibited
    pub fn seat(&self, dh: &DisplayHandle) -> Option<WlSeat> {
        WlSeat::from_id(dh, self.seat_id().clone()).ok()
    }

    /// Seat that is beeing inhibited
    #[inline]
    pub fn wl_surface(&self) -> &WlSurface {
        &self.data().surface
    }

    /// Is inhibitor active
    pub fn is_active(&self) -> bool {
        self.data().is_active.load(atomic::Ordering::Acquire)
    }

    /// This method indicates that the shortcut inhibitor is active.
    ///
    /// When active, the client may receive input events normally reserved by the compositor.
    ///
    /// Typically this is called when user instructs the compositor to enable inhibitor using any mechanism offered by the compositor,
    /// either by accepting new inhibitor or re-enabling already existing inhibitor.
    pub fn activate(&self) {
        self.inhibitor.active();
        self.set_is_active(true);
    }

    /// This event indicates that the shortcuts inhibitor is inactive, normal shortcuts processing is restored by the compositor.
    pub fn inactivate(&self) {
        self.inhibitor.inactive();
        self.set_is_active(false);
    }
}

#[derive(Debug, Default)]
struct SeatData {
    inhibitors: Rc<RefCell<SeatInhibitors>>,
}

impl SeatData {
    fn get<D>(seat: &Seat<D>) -> &RefCell<Self>
    where
        D: SeatHandler,
        D: 'static,
    {
        seat.user_data()
            .insert_if_missing(|| RefCell::new(SeatData::default()));
        seat.user_data().get().unwrap()
    }

    /// Check if any inhibitor is active
    fn is_inhibited(&self) -> bool {
        self.inhibitors.borrow().is_inhibited()
    }

    /// Find inhibitor for WlSurface
    fn inhibitor_for_surface(&self, surface: &WlSurface) -> Option<KeyboardShortcutsInhibitor> {
        self.inhibitors.borrow().inhibitor_for_surface(surface).cloned()
    }
}

/// Seat extension used to check if shortcuts are inhibited
pub trait KeyboardShortcutsInhibitorSeat {
    /// Check if keyboard_shortcuts are inhibited
    fn keyboard_shortcuts_inhibited(&self) -> bool;

    /// Get inhibitors associated with given WlSurface
    ///
    /// Can be used to check if certain surface has inhibitor on it
    /// ```no_run
    /// use smithay::input::Seat;
    /// use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat;
    /// # use smithay::input::{SeatHandler, SeatState, pointer::CursorImageStatus};
    /// # use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
    /// # struct State;
    /// # impl SeatHandler for State {
    /// #     type KeyboardFocus = WlSurface;
    /// #     type PointerFocus = WlSurface;
    /// #     type TouchFocus = WlSurface;
    /// #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
    /// #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
    /// #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
    /// # }
    ///
    /// # let wl_surface: WlSurface = todo!();
    ///
    /// let seat: Seat<State> = todo!();
    ///
    /// if let Some(inhibitor) = seat.keyboard_shortcuts_inhibitor_for_surface(&wl_surface) {
    ///     dbg!(inhibitor.is_active());
    /// }
    /// ```
    fn keyboard_shortcuts_inhibitor_for_surface(
        &self,
        surface: &WlSurface,
    ) -> Option<KeyboardShortcutsInhibitor>;
}

impl<D> KeyboardShortcutsInhibitorSeat for Seat<D>
where
    D: SeatHandler,
    D: 'static,
{
    fn keyboard_shortcuts_inhibited(&self) -> bool {
        SeatData::get(self).borrow().is_inhibited()
    }

    fn keyboard_shortcuts_inhibitor_for_surface(
        &self,
        surface: &WlSurface,
    ) -> Option<KeyboardShortcutsInhibitor> {
        SeatData::get(self).borrow().inhibitor_for_surface(surface)
    }
}

/// WP Keyboard shortcuts inhibit handler
#[allow(unused_variables)]
pub trait KeyboardShortcutsInhibitHandler {
    /// [KeyboardShortcutsInhibitState] getter
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState;

    /// New keyboard shortcuts inhibitor got created by the client
    ///
    /// In response to this event compositor can decide if inhibitor should be activated or not, usually based on user decision.
    /// You may also postpone activation based on your compositor specific policy.
    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {}

    /// Inhibitor got destoryed
    fn inhibitor_destroyed(&mut self, inhibitor: KeyboardShortcutsInhibitor) {}
}

/// Macro to delegate implementation of the keyboard shortcuts inhibit protocol
///
/// You must also implement [`KeyboardShortcutsInhibitHandler`] to use this.
#[macro_export]
macro_rules! delegate_keyboard_shortcuts_inhibit {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1: ()
        ] => $crate::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1: ()
        ] => $crate::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1: $crate::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorUserData
        ] => $crate::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState);
    };
}
