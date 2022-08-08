use std::{
    fmt,
    sync::{Arc, Mutex},
};

use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle, Resource};

use crate::{
    backend::input::{ButtonState, KeyState},
    input::{
        keyboard::{
            GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle, KeyboardHandler,
            KeyboardInnerHandle, ModifiersState,
        },
        pointer::{
            AxisFrame, ButtonEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
            PointerHandler, PointerInnerHandle,
        },
        SeatHandler,
    },
    utils::{DeadResource, IsAlive, Logical, Point},
    wayland::{compositor::get_role, shell::xdg::XDG_POPUP_ROLE, Serial},
};

use thiserror::Error;

use super::{PopupKind, PopupManager};

/// Defines the possible errors that
/// can be returned from [`PopupManager::grab_popup`]
#[derive(Debug, Error)]
pub enum PopupGrabError {
    /// This resource has been destroyed and can no longer be used.
    #[error(transparent)]
    DeadResource(#[from] DeadResource),
    /// The client tried to grab a popup after it's parent has been dismissed
    #[error("the parent of the popup has been already dismissed")]
    ParentDismissed,
    /// The client tried to grab a popup after it has been mapped
    #[error("tried to grab after being mapped")]
    InvalidGrab,
    /// The client tried to grab a popup which is not the topmost
    #[error("popup was not created on the topmost popup")]
    NotTheTopmostPopup,
}

/// Defines the possibly strategies
/// for the [`PopupGrab::ungrab`] operation
#[derive(Debug, Copy, Clone)]
pub enum PopupUngrabStrategy {
    /// Only ungrab the topmost popup
    Topmost,
    /// Ungrab all active popups
    All,
}

#[derive(Debug, Default)]
struct PopupGrabInternal {
    serial: Option<Serial>,
    active_grabs: Vec<(WlSurface, PopupKind)>,
    dismissed_grabs: Vec<(WlSurface, PopupKind)>,
}

impl PopupGrabInternal {
    fn active(&self) -> bool {
        !self.active_grabs.is_empty() || !self.dismissed_grabs.is_empty()
    }

    fn current_grab(&self) -> Option<&WlSurface> {
        self.active_grabs
            .iter()
            .rev()
            .find(|(_, p)| p.alive())
            .map(|(s, _)| s)
    }

    fn is_dismissed(&self, surface: &WlSurface) -> bool {
        self.dismissed_grabs.iter().any(|(s, _)| s == surface)
    }

    fn append_grab(&mut self, popup: &PopupKind) {
        let surface = popup.wl_surface();
        self.active_grabs.push((surface.clone(), popup.clone()));
    }

    fn cleanup(&mut self) {
        let mut i = 0;
        while i < self.active_grabs.len() {
            if !self.active_grabs[i].1.alive() {
                let grab = self.active_grabs.remove(i);
                self.dismissed_grabs.push(grab);
            } else {
                i += 1;
            }
        }

        self.dismissed_grabs.retain(|(s, _)| s.alive());
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct PopupGrabInner {
    internal: Arc<Mutex<PopupGrabInternal>>,
}

impl PopupGrabInner {
    pub(super) fn active(&self) -> bool {
        let guard = self.internal.lock().unwrap();
        guard.active()
    }

    fn current_grab(&self) -> Option<WlSurface> {
        let guard = self.internal.lock().unwrap();
        guard
            .active_grabs
            .iter()
            .rev()
            .find(|(_, p)| p.alive())
            .map(|(s, _)| s)
            .cloned()
    }

    pub(super) fn cleanup(&self) {
        let mut guard = self.internal.lock().unwrap();
        guard.cleanup();
    }

    pub(super) fn grab(&self, popup: &PopupKind, serial: Serial) -> Result<Option<Serial>, PopupGrabError> {
        let parent = popup.parent().ok_or(DeadResource)?;
        let parent_role = get_role(&parent);

        self.cleanup();

        let mut guard = self.internal.lock().unwrap();

        match guard.current_grab() {
            Some(grab) => {
                if grab != &parent {
                    // If the parent is a grabbing popup which has already been dismissed, this popup will be immediately dismissed.
                    if guard.is_dismissed(&parent) {
                        return Err(PopupGrabError::ParentDismissed);
                    }

                    // If the parent is a popup that did not take an explicit grab, an error will be raised.
                    return Err(PopupGrabError::NotTheTopmostPopup);
                }
            }
            None => {
                if parent_role == Some(XDG_POPUP_ROLE) {
                    return Err(PopupGrabError::NotTheTopmostPopup);
                }
            }
        }

        guard.append_grab(popup);

        Ok(guard.serial.replace(serial))
    }

    fn ungrab(
        &self,
        dh: &DisplayHandle,
        root: &WlSurface,
        strategy: PopupUngrabStrategy,
    ) -> Option<WlSurface> {
        let mut guard = self.internal.lock().unwrap();
        let dismissed = match strategy {
            PopupUngrabStrategy::Topmost => {
                if let Some(grab) = guard.active_grabs.pop() {
                    let dismissed = PopupManager::dismiss_popup(dh, root, &grab.1);

                    if dismissed.is_ok() {
                        guard.dismissed_grabs.push(grab);
                    }

                    dismissed
                } else {
                    Ok(())
                }
            }
            PopupUngrabStrategy::All => {
                let grabs = guard.active_grabs.drain(..).collect::<Vec<_>>();

                if let Some(grab) = grabs.first() {
                    let dismissed = PopupManager::dismiss_popup(dh, root, &grab.1);

                    if dismissed.is_ok() {
                        guard.dismissed_grabs.push(grab.clone());
                        guard.dismissed_grabs.extend(grabs);
                    }

                    dismissed
                } else {
                    Ok(())
                }
            }
        };

        if dismissed.is_err() {
            // If dismiss_popup returns Err(DeadResource) there is not much what
            // can do about it here, we just remove all our grabs as they are dead now
            // anyway. The pointer/keyboard grab will be unset automatically so we
            // should be fine.
            guard.active_grabs.drain(..);
        }

        guard.current_grab().cloned()
    }
}

/// Represents the explicit grab a client requested for a popup
///
/// An explicit grab can be used by a client to redirect all keyboard
/// input to a single popup. The focus of the keyboard will stay on
/// the popup for as long as the grab is valid, that is as long as the
/// compositor did not call [`ungrab`](PopupGrab::ungrab) or the client
/// did not destroy the popup. A grab can be nested by requesting a grab
/// on a popup who's parent is the currently grabbed popup. The grab will
/// be returned to the parent after the popup has been dismissed.
///
/// This module also provides default implementations for [`KeyboardGrab`] and
/// [`PointerGrab`] that implement the behavior described in the [`xdg-shell`](https://wayland.app/protocols/xdg-shell#xdg_popup:request:grab)
/// specification. See [`PopupKeyboardGrab`] and [`PopupPointerGrab`] for more
/// information on the default implementations.
///
/// In case the implemented behavior is not suited for your use-case the grab can be
/// either decorated or a custom [`KeyboardGrab`]/[`PointerGrab`] can use the methods
/// on the [`PopupGrab`] to implement a custom behavior.
///
/// One example would be to use a timer to automatically dismiss the popup after some
/// timeout.
///
/// The grab is obtained by calling [`PopupManager::grap_popup`](super::PopupManager::grab_popup).
pub struct PopupGrab<D: SeatHandler + 'static> {
    dh: DisplayHandle,
    root: WlSurface,
    serial: Serial,
    previous_serial: Option<Serial>,
    toplevel_grab: PopupGrabInner,
    keyboard_handle: Option<KeyboardHandle<D>>,
    keyboard_grab_start_data: KeyboardGrabStartData<D>,
    pointer_grab_start_data: PointerGrabStartData<D>,
}

impl<D: SeatHandler + 'static> Clone for PopupGrab<D> {
    fn clone(&self) -> Self {
        PopupGrab {
            dh: self.dh.clone(),
            root: self.root.clone(),
            serial: self.serial.clone(),
            previous_serial: self.previous_serial.clone(),
            toplevel_grab: self.toplevel_grab.clone(),
            keyboard_handle: self.keyboard_handle.clone(),
            keyboard_grab_start_data: self.keyboard_grab_start_data.clone(),
            pointer_grab_start_data: self.pointer_grab_start_data.clone(),
        }
    }
}

impl<D: SeatHandler + 'static> fmt::Debug for PopupGrab<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PopupGrab")
            .field("dh", &self.dh)
            .field("root", &self.root)
            .field("serial", &self.serial)
            .field("previous_serial", &self.previous_serial)
            .field("toplevel_grab", &self.toplevel_grab)
            .field("keyboard_handle", &self.keyboard_handle.as_ref().map(|_| "..."))
            .field("keyboard_grab_start_data", &"...")
            .field("pointer_grab_start_data", &"...")
            .finish()
    }
}

impl<D: SeatHandler + 'static> PopupGrab<D> {
    pub(super) fn new(
        dh: &DisplayHandle,
        toplevel_popups: PopupGrabInner,
        root: WlSurface,
        serial: Serial,
        previous_serial: Option<Serial>,
        keyboard_handle: Option<KeyboardHandle<D>>,
    ) -> Self {
        PopupGrab {
            dh: dh.clone(),
            root: root.clone(),
            serial,
            previous_serial,
            toplevel_grab: toplevel_popups,
            keyboard_handle,
            keyboard_grab_start_data: KeyboardGrabStartData {
                // We set the focus to root as this will make
                // sure the grab will stay alive until the
                // toplevel is destroyed or the grab is unset
                focus: Some(Box::new(root.clone())),
            },
            pointer_grab_start_data: PointerGrabStartData {
                button: 0,
                // We set the focus to root as this will make
                // sure the grab will stay alive until the
                // toplevel is destroyed or the grab is unset
                focus: Some((Box::new(root), (0, 0).into())),
                location: (0f64, 0f64).into(),
            },
        }
    }

    /// Returns the serial that was used to grab the popup
    pub fn serial(&self) -> Serial {
        self.serial
    }

    /// Returns the previous serial that was used to grab
    /// the parent popup in case of nested grabs
    pub fn previous_serial(&self) -> Option<Serial> {
        self.previous_serial
    }

    /// Check if this grab has ended
    ///
    /// A grab has ended if either all popups
    /// associated with the grab have been dismissed
    /// by the server with [`PopupGrab::ungrab`] or by the client
    /// by destroying the popup.
    ///
    /// This will also return [`false`] if the root
    /// of the grab has been destroyed.
    pub fn has_ended(&self) -> bool {
        !self.root.alive() || !self.toplevel_grab.active()
    }

    /// Returns the current grabbed [`WlSurface`].
    ///
    /// If the grab has ended this will return the root surface
    /// so that the client expected focus can be restored
    pub fn current_grab(&self) -> Option<WlSurface> {
        self.toplevel_grab
            .current_grab()
            .or_else(|| Some(self.root.clone()))
    }

    /// Ungrab and dismiss a popup
    ///
    /// This will dismiss either the topmost or all popups
    /// according to the specified [`PopupUngrabStrategy`]
    ///
    /// Returns the new topmost popup in case of nested popups
    /// or if the grab has ended the root surface
    pub fn ungrab(&mut self, strategy: PopupUngrabStrategy) -> Option<WlSurface> {
        self.toplevel_grab
            .ungrab(&self.dh, &self.root, strategy)
            .or_else(|| Some(self.root.clone()))
    }

    /// Convenience method for getting a [`KeyboardGrabStartData`] for this grab.
    ///
    /// The focus of the [`KeyboardGrabStartData`] will always be the root
    /// of the popup grab, e.g. the surface of the toplevel, to make sure
    /// the grab is not automatically unset.
    pub fn keyboard_grab_start_data(&self) -> &KeyboardGrabStartData<D> {
        &self.keyboard_grab_start_data
    }

    /// Convenience method for getting a [`PointerGrabStartData`] for this grab.
    ///
    /// The focus of the [`PointerGrabStartData`] will always be the root
    /// of the popup grab, e.g. the surface of the toplevel, to make sure
    /// the grab is not automatically unset.
    pub fn pointer_grab_start_data(&self) -> &PointerGrabStartData<D> {
        &self.pointer_grab_start_data
    }

    fn unset_keyboard_grab(&self, data: &mut D, serial: Serial) {
        if let Some(keyboard) = self.keyboard_handle.as_ref() {
            if keyboard.is_grabbed()
                && (keyboard.has_grab(self.serial)
                    || keyboard.has_grab(self.previous_serial.unwrap_or(self.serial)))
            {
                keyboard.unset_grab();
                keyboard.set_focus(data, Some(self.root.clone()), serial);
            }
        }
    }
}

/// Default implementation of a [`KeyboardGrab`] for [`PopupGrab`]
///
/// The [`PopupKeyboardGrab`] will keep the focus of the keyboard
/// on the topmost popup until the grab has ended. If the
/// grab has ended it will restore the focus on the root of the grab
/// and unset the [`KeyboardGrab`]
#[derive(Debug)]
pub struct PopupKeyboardGrab<D: SeatHandler + 'static> {
    popup_grab: PopupGrab<D>,
}

impl<D: SeatHandler + 'static> PopupKeyboardGrab<D> {
    /// Create a [`PopupKeyboardGrab`] for the provided [`PopupGrab`]
    pub fn new(popup_grab: &PopupGrab<D>) -> Self {
        let grab: PopupGrab<D> = PopupGrab::<D>::clone(popup_grab);
        PopupKeyboardGrab { popup_grab: grab }
    }
}

impl<D: SeatHandler> KeyboardGrab<D> for PopupKeyboardGrab<D> {
    fn input(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: u32,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        // Check if the grab changed and update the focus
        // If the grab has ended this will return the root
        // surface to restore the client expected focus.
        if let Some(surface) = self.popup_grab.current_grab() {
            handle.set_focus(data, Some(Box::new(surface)), serial);
        }

        if self.popup_grab.has_ended() {
            handle.unset_grab(data, serial, false);
        }

        handle.input(data, keycode, state, modifiers, serial, time)
    }

    fn set_focus(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<Box<dyn KeyboardHandler<D>>>,
        serial: Serial,
    ) {
        // Ignore focus changes unless the grab has ended
        if self.popup_grab.has_ended() {
            handle.set_focus(data, focus, serial);
            handle.unset_grab(data, serial, false);
            return;
        }

        // Allow to set the focus to the current grab, this can
        // happen if the user initially sets the focus to
        // popup instead of relying on the grab behavior
        if focus
            .as_ref()
            .and_then(|h| h.as_any().downcast_ref::<WlSurface>())
            == self.popup_grab.current_grab().as_ref()
        {
            handle.set_focus(data, focus, serial);
        }
    }

    fn start_data(&self) -> &KeyboardGrabStartData<D> {
        self.popup_grab.keyboard_grab_start_data()
    }
}

/// Default implementation of a [`PointerGrab`] for [`PopupGrab`]
///
/// The [`PopupPointerGrab`] will make sure that the pointer focus
/// stays on the same client as the grabbed popup (similar to an
/// "owner-events" grab in X11 parlance). If an input event happens
/// outside of the grabbed [`WlSurface`] the popup will be dismissed
/// and the grab ends. In case of a nested grab all parent grabs will
/// also be dismissed.
///
/// If the grab has ended the pointer focus is restored and the
/// [`PointerGrab`] is unset. Additional it will unset an active
/// [`KeyboardGrab`] that matches the [`Serial`] of this grab and
/// restore the keyboard focus like described in [`PopupKeyboardGrab`]
#[derive(Debug)]
pub struct PopupPointerGrab<D: SeatHandler + 'static> {
    popup_grab: PopupGrab<D>,
}

impl<D: SeatHandler + 'static> PopupPointerGrab<D> {
    /// Create a [`PopupPointerGrab`] for the provided [`PopupGrab`]
    pub fn new(popup_grab: &PopupGrab<D>) -> Self {
        PopupPointerGrab {
            popup_grab: popup_grab.clone(),
        }
    }

    fn focus_client_equals(&self, focus: Option<&dyn PointerHandler<D>>) -> bool {
        match (focus, self.popup_grab.current_grab()) {
            (Some(a), Some(b)) => a
                .as_any()
                .downcast_ref::<WlSurface>()
                .map(|s| s.id().same_client_as(&b.id()))
                .unwrap_or(false),
            (None, Some(_)) | (Some(_), None) => false,
            (None, None) => true,
        }
    }
}

impl<D: SeatHandler> PointerGrab<D> for PopupPointerGrab<D> {
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(Box<dyn PointerHandler<D>>, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        if self.popup_grab.has_ended() {
            handle.unset_grab(data, event.serial, event.time);
            self.popup_grab.unset_keyboard_grab(data, event.serial);
            return;
        }

        // Check that the focus is of the same client as the grab
        // If yes allow it, if not unset the focus.
        if self.focus_client_equals(focus.as_ref().map(|(a, _)| &**a)) {
            handle.motion(data, focus, event);
        } else {
            handle.motion(data, focus, event);
        }
    }

    fn button(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, event: &ButtonEvent) {
        let serial = event.serial;
        let time = event.time;
        let state = event.state;

        if self.popup_grab.has_ended() {
            handle.unset_grab(data, serial, time);
            handle.button(data, event);
            self.popup_grab.unset_keyboard_grab(data, serial);
            return;
        }

        // Check if the the client of the focused surface is still equal to the grabbed surface client
        // if not the popup will be dismissed
        if state == ButtonState::Pressed && !self.focus_client_equals(handle.current_focus().map(|(a, _)| a))
        {
            let _ = self.popup_grab.ungrab(PopupUngrabStrategy::All);
            handle.unset_grab(data, serial, time);
            handle.button(data, event);
            self.popup_grab.unset_keyboard_grab(data, serial);
            return;
        }

        handle.button(data, event);
    }

    fn axis(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn start_data(&self) -> &PointerGrabStartData<D> {
        self.popup_grab.pointer_grab_start_data()
    }
}
