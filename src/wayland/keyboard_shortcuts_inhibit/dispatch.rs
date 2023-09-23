use std::{
    collections::hash_map::Entry,
    sync::atomic::{self, AtomicBool},
};

use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::{
    zwp_keyboard_shortcuts_inhibit_manager_v1::{self, ZwpKeyboardShortcutsInhibitManagerV1},
    zwp_keyboard_shortcuts_inhibitor_v1::{self, ZwpKeyboardShortcutsInhibitorV1},
};
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::wl_surface::WlSurface,
    Dispatch, GlobalDispatch, Resource,
};

use crate::input::{Seat, SeatHandler};

use super::{KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState};

/// User data of [ZwpKeyboardShortcutsInhibitorV1] object
#[derive(Debug)]
pub struct KeyboardShortcutsInhibitorUserData {
    /// Seat that is beeing inhibited
    pub(crate) seat: ObjectId,
    /// Surface that is inhibiting shortcuts
    pub(crate) surface: WlSurface,
    pub(crate) is_active: AtomicBool,
}

impl<D> GlobalDispatch<ZwpKeyboardShortcutsInhibitManagerV1, (), D> for KeyboardShortcutsInhibitState
where
    D: KeyboardShortcutsInhibitHandler,
    D: Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()>,
    D: Dispatch<ZwpKeyboardShortcutsInhibitorV1, KeyboardShortcutsInhibitorUserData>,
{
    fn bind(
        _state: &mut D,
        _handle: &wayland_server::DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<ZwpKeyboardShortcutsInhibitManagerV1>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, (), D> for KeyboardShortcutsInhibitState
where
    D: KeyboardShortcutsInhibitHandler,
    D: SeatHandler,
    D: Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()>,
    D: Dispatch<ZwpKeyboardShortcutsInhibitorV1, KeyboardShortcutsInhibitorUserData>,
{
    fn request(
        handler: &mut D,
        _client: &wayland_server::Client,
        resource: &ZwpKeyboardShortcutsInhibitManagerV1,
        request: zwp_keyboard_shortcuts_inhibit_manager_v1::Request,
        _data: &(),
        _dhandle: &wayland_server::DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_keyboard_shortcuts_inhibit_manager_v1::Request::InhibitShortcuts { id, surface, seat } => {
                let seat_id = seat.id();

                if handler
                    .keyboard_shortcuts_inhibit_state()
                    .inhibitors
                    .get(&seat_id)
                    .map(|list| list.borrow().surface_has_inhibitor(&surface))
                    .unwrap_or(false)
                {
                    resource.post_error(
                        zwp_keyboard_shortcuts_inhibit_manager_v1::Error::AlreadyInhibited,
                        "Keyboare shortcuts for this surface are already inhibited",
                    );
                    return;
                }

                let seat = Seat::<D>::from_resource(&seat).unwrap();
                let seat_data = super::SeatData::get(&seat);

                let inhibitor = data_init.init(
                    id,
                    KeyboardShortcutsInhibitorUserData {
                        seat: seat_id.clone(),
                        surface,
                        is_active: AtomicBool::new(false),
                    },
                );

                let inhibitor = super::KeyboardShortcutsInhibitor { inhibitor };

                let list = handler
                    .keyboard_shortcuts_inhibit_state()
                    .inhibitors
                    .entry(seat_id)
                    .or_insert_with(|| seat_data.borrow().inhibitors.clone());

                list.borrow_mut().push(inhibitor.clone());

                handler.new_inhibitor(inhibitor);
            }
            zwp_keyboard_shortcuts_inhibit_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<ZwpKeyboardShortcutsInhibitorV1, KeyboardShortcutsInhibitorUserData, D>
    for KeyboardShortcutsInhibitState
where
    D: KeyboardShortcutsInhibitHandler,
{
    fn request(
        _handler: &mut D,
        _client: &wayland_server::Client,
        _inhibitor: &ZwpKeyboardShortcutsInhibitorV1,
        request: zwp_keyboard_shortcuts_inhibitor_v1::Request,
        _data: &KeyboardShortcutsInhibitorUserData,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_keyboard_shortcuts_inhibitor_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        handler: &mut D,
        _client: ClientId,
        wl_inhibitor: &ZwpKeyboardShortcutsInhibitorV1,
        data: &KeyboardShortcutsInhibitorUserData,
    ) {
        data.is_active.store(false, atomic::Ordering::Release);

        let state = handler.keyboard_shortcuts_inhibit_state();

        if let Entry::Occupied(mut entry) = state.inhibitors.entry(data.seat.clone()) {
            let inhibitor = entry.get_mut().borrow_mut().remove(wl_inhibitor.id());

            if entry.get().borrow().is_empty() {
                entry.remove();
            }

            if let Some(inhibitor) = inhibitor {
                handler.inhibitor_destroyed(inhibitor)
            }
        }
    }
}
