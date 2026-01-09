use std::{
    fmt, fs,
    sync::{Arc, Mutex},
};

use tracing::{info, warn};
use wayland_protocols_misc::zwp_input_method_v2::server::{
    zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
    zwp_input_method_v2::{self, ZwpInputMethodV2},
    zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
};
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::wl_surface::WlSurface,
};
use wayland_server::{
    protocol::wl_keyboard::KeymapFormat, Client, DataInit, Dispatch, DisplayHandle, Resource,
};

use crate::{
    input::{keyboard::KeyboardHandle, SeatHandler},
    utils::{alive_tracker::AliveTracker, Logical, Rectangle, SERIAL_COUNTER},
    wayland::{compositor, seat::WaylandFocus, text_input::TextInputHandle},
};

use super::{
    input_method_keyboard_grab::InputMethodKeyboardGrab,
    input_method_popup_surface::{PopupHandle, PopupParent, PopupSurface},
    InputMethodHandler, InputMethodKeyboardUserData, InputMethodManagerState,
    InputMethodPopupSurfaceUserData, INPUT_POPUP_SURFACE_ROLE,
};

#[derive(Default, Debug)]
pub(crate) struct InputMethod {
    pub instances: Vec<Instance>,
    pub active_input_method_id: Option<ObjectId>,
    pub popup_handle: PopupHandle,
    pub keyboard_grab: InputMethodKeyboardGrab,
}

#[derive(Debug)]
pub(crate) struct Instance {
    pub object: ZwpInputMethodV2,
    pub serial: u32,
    pub app_id: String,
    pub keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
}

impl Instance {
    /// Send the done incrementing the serial.
    pub(crate) fn done(&mut self) {
        self.object.done();
        self.serial += 1;
    }
}

/// Extract app_id from a client's PID by reading /proc/<pid>/comm
fn get_app_id_from_pid(pid: i32) -> String {
    // Try to read the process name from /proc/<pid>/comm
    let comm_path = format!("/proc/{}/comm", pid);
    if let Ok(comm) = fs::read_to_string(&comm_path) {
        return comm.trim().to_string();
    }

    // Fallback: try to get the executable name from /proc/<pid>/cmdline
    let cmdline_path = format!("/proc/{}/cmdline", pid);
    if let Ok(cmdline) = fs::read_to_string(&cmdline_path) {
        // cmdline is null-separated, take the first argument (the executable)
        if let Some(exe) = cmdline.split('\0').next() {
            // Extract just the filename from the path
            if let Some(name) = exe.rsplit('/').next() {
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }

    // Final fallback: use the PID as identifier
    format!("unknown-{}", pid)
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodHandle {
    pub(crate) inner: Arc<Mutex<InputMethod>>,
}

impl InputMethodHandle {
    pub(super) fn add_instance(
        &self,
        instance: &ZwpInputMethodV2,
        client: &Client,
        dh: &DisplayHandle,
    ) -> String {
        let app_id = client
            .get_credentials(dh)
            .ok()
            .map(|creds| get_app_id_from_pid(creds.pid))
            .unwrap_or_else(|| format!("unknown-client-{:?}", client.id()));

        let mut inner = self.inner.lock().unwrap();
        inner.instances.push(Instance {
            object: instance.clone(),
            serial: 0,
            app_id: app_id.clone(),
            keyboard_grab: None,
        });

        // Don't auto-activate - let the compositor decide when to activate
        app_id
    }

    /// Whether there's an active instance of input-method.
    pub(crate) fn has_instance(&self) -> bool {
        self.inner.lock().unwrap().active_input_method_id.is_some()
    }

    /// Callback function to access the active input method instance
    pub(crate) fn with_instance<F>(&self, mut f: F)
    where
        F: FnMut(&mut Instance),
    {
        let mut inner = self.inner.lock().unwrap();
        let active_id = match inner.active_input_method_id.clone() {
            Some(id) => id,
            None => return,
        };
        let index = inner.instances.iter().position(|i| i.object.id() == active_id);
        if let Some(idx) = index {
            f(&mut inner.instances[idx]);
        }
    }

    /// Callback function to access the input method.
    pub(crate) fn with_input_method<F>(&self, mut f: F)
    where
        F: FnMut(&mut InputMethod),
    {
        let mut inner = self.inner.lock().unwrap();
        f(&mut inner);
    }

    /// Get a list of all registered input method instances with their app_ids and active status.
    /// Returns a vector of tuples: (app_id, serial, is_active)
    pub fn list_instances(&self) -> Vec<(String, u32, bool)> {
        let inner = self.inner.lock().unwrap();
        inner
            .instances
            .iter()
            .map(|inst| {
                let is_active = inner.active_input_method_id.as_ref() == Some(&inst.object.id());
                (inst.app_id.clone(), inst.serial, is_active)
            })
            .collect()
    }

    /// Set which input method instance should be active by app_id.
    /// Returns true if an instance with the given app_id was found and set as active.
    pub fn set_active_instance(&self, app_id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();

        // Check if we're switching to a different input method
        let old_active_id = inner.active_input_method_id.clone();

        if let Some(instance) = inner.instances.iter().find(|i| i.app_id == app_id) {
            let object_id = instance.object.id();

            // Check if this is the same as the currently active one
            let is_same = old_active_id.as_ref() == Some(&object_id);

            info!(
                "InputMethod: set_active_instance - app_id: '{}', new_id: {:?}, old_id: {:?}, is_same: {}",
                app_id, object_id, old_active_id, is_same
            );

            if !is_same && old_active_id.is_some() {
                info!(
                    "InputMethod: set_active_instance - SWITCHING: deactivating old instance {:?}",
                    old_active_id
                );

                // Deactivate the old instance first
                if let Some(old_id) = old_active_id {
                    if let Some(old_instance) = inner.instances.iter_mut().find(|i| i.object.id() == old_id) {
                        info!(
                            "InputMethod: set_active_instance - sending deactivate() to old instance {:?} (app_id: '{}')",
                            old_id, old_instance.app_id
                        );
                        old_instance.object.deactivate();
                        old_instance.done();
                    }
                }

                // Clear the keyboard grab's active ID
                let mut keyboard_grab = inner.keyboard_grab.inner.lock().unwrap();
                keyboard_grab.active_input_method_id = None;
                info!("InputMethod: set_active_instance - cleared keyboard_grab.active_input_method_id");
                drop(keyboard_grab);
            }

            inner.active_input_method_id = Some(object_id.clone());
            info!(
                "InputMethod: set_active_instance - set inner.active_input_method_id to {:?}",
                object_id
            );

            // Log keyboard grab state
            let keyboard_grab = inner.keyboard_grab.inner.lock().unwrap();
            info!(
                "InputMethod: set_active_instance - AFTER: keyboard_grab.active_input_method_id: {:?}",
                keyboard_grab.active_input_method_id
            );
            drop(keyboard_grab);

            true
        } else {
            info!(
                "InputMethod: set_active_instance - app_id: '{}' not found",
                app_id
            );
            false
        }
    }

    /// Clear the active input method instance.
    /// This deactivates any currently active input method and releases any keyboard grab.
    pub fn clear_active_instance<D: SeatHandler + 'static>(&self, state: &mut D) {
        info!("InputMethod: clear_active_instance - START");

        // First deactivate the input method (this also releases keyboard grab)
        self.deactivate_input_method(state);

        // Then clear the active instance
        let mut inner = self.inner.lock().unwrap();
        inner.active_input_method_id = None;

        info!("InputMethod: clear_active_instance - COMPLETE - active_input_method_id is now None");
    }

    /// Indicates that an input method has grabbed a keyboard
    pub fn keyboard_grabbed(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        let keyboard = inner.keyboard_grab.inner.lock().unwrap();
        keyboard.grab.is_some()
    }

    /// Send keymap update to active input method keyboard grab
    ///
    /// This should be called when the keyboard layout changes to ensure
    /// the IME receives the new keymap and can update its virtual keyboard accordingly.
    pub fn send_keymap_to_grab<D: SeatHandler + 'static>(&self, keyboard_handle: &KeyboardHandle<D>) {
        let inner = self.inner.lock().unwrap();

        // Get the active instance's keyboard grab
        let active_id = match inner.active_input_method_id.as_ref() {
            Some(id) => id,
            None => {
                info!("InputMethod: send_keymap_to_grab - no active input method");
                return;
            }
        };

        let keyboard_grab_obj = inner
            .instances
            .iter()
            .find(|inst| inst.object.id() == *active_id)
            .and_then(|inst| inst.keyboard_grab.as_ref());

        let keyboard_grab = match keyboard_grab_obj {
            Some(grab) => grab,
            None => {
                info!("InputMethod: send_keymap_to_grab - active instance has no keyboard grab");
                return;
            }
        };

        // Send the current keymap to the grab
        let guard = keyboard_handle.arc.internal.lock().unwrap();
        let keymap_file = keyboard_handle.arc.keymap.lock().unwrap();
        let res = keymap_file.with_fd(false, |fd, size| {
            keyboard_grab.keymap(KeymapFormat::XkbV1, fd, size as u32);
        });

        if let Err(err) = res {
            warn!(err = ?err, "Failed to send keymap update to IME keyboard grab");
        } else {
            info!("InputMethod: send_keymap_to_grab - successfully sent keymap to active grab");

            // Also send current modifiers to keep them in sync
            let mods = guard.mods_state.serialized;
            keyboard_grab.modifiers(
                SERIAL_COUNTER.next_serial().into(),
                mods.depressed,
                mods.latched,
                mods.locked,
                mods.layout_effective,
            );
        }
    }

    pub(crate) fn set_text_input_rectangle<D: SeatHandler + 'static>(
        &self,
        state: &mut D,
        rect: Rectangle<i32, Logical>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        inner.popup_handle.rectangle = rect;

        let mut popup_surface = match inner.popup_handle.surface.clone() {
            Some(popup_surface) => popup_surface,
            None => return,
        };

        popup_surface.set_text_input_rectangle(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);

        let active_id = match inner.active_input_method_id.clone() {
            Some(id) => id,
            None => return,
        };

        if let Some(instance) = inner.instances.iter().find(|i| i.object.id() == active_id) {
            let data = instance.object.data::<InputMethodUserData<D>>().unwrap();
            (data.popup_repositioned)(state, popup_surface.clone());
        }
    }

    /// Activate input method on the given surface.
    pub fn activate_input_method<D: SeatHandler + 'static>(&self, state: &mut D, surface: &WlSurface) {
        info!("InputMethod: activate_input_method - CALLED");

        self.with_input_method(|im| {
            let active_id = match im.active_input_method_id.clone() {
                Some(id) => {
                    info!(
                        "InputMethod: activate_input_method - inner.active_input_method_id is {:?}",
                        id
                    );
                    id
                }
                None => {
                    info!(
                        "InputMethod: activate_input_method - no active_input_method_id set, returning early"
                    );
                    return;
                }
            };

            let instance = match im.instances.iter_mut().find(|i| i.object.id() == active_id) {
                Some(inst) => inst,
                None => {
                    info!(
                        "InputMethod: activate_input_method - instance not found for active_id: {:?}",
                        active_id
                    );
                    return;
                }
            };

            info!(
                "InputMethod: activate_input_method - sending activate() to instance {:?} (app_id: '{}')",
                instance.object.id(),
                instance.app_id
            );
            instance.object.activate();

            if let Some(popup) = im.popup_handle.surface.as_mut() {
                let data = instance.object.data::<InputMethodUserData<D>>().unwrap();
                let location = (data.popup_geometry_callback)(state, surface);
                // Remove old popup.
                (data.dismiss_popup)(state, popup.clone());

                // Add a new one with updated parent.
                let parent = PopupParent {
                    surface: surface.clone(),
                    location,
                };
                popup.set_parent(Some(parent));
                (data.new_popup)(state, popup.clone());
            }
        });

        // Set the active input method ID in the keyboard grab so it knows to forward events
        // Also update the grab object to point to the active IME's grab
        let inner = self.inner.lock().unwrap();
        let active_id = inner.active_input_method_id.clone();
        info!(
            "InputMethod: activate_input_method - BEFORE setting keyboard grab: inner.active_input_method_id: {:?}",
            active_id
        );
        if let Some(id) = active_id {
            // Find the active instance and get its grab object
            let active_instance_grab = inner
                .instances
                .iter()
                .find(|inst| inst.object.id() == id)
                .and_then(|inst| inst.keyboard_grab.clone());

            let mut keyboard_grab = inner.keyboard_grab.inner.lock().unwrap();
            keyboard_grab.active_input_method_id = Some(id.clone());

            // Update the grab object to point to the active IME's grab
            if let Some(grab) = active_instance_grab {
                keyboard_grab.grab = Some(grab.clone());
                info!(
                    "InputMethod: activate_input_method - COMPLETE - set keyboard_grab to active IME's grab object for {:?}",
                    id
                );
            } else {
                info!(
                    "InputMethod: activate_input_method - WARNING: active instance {:?} has no keyboard grab object",
                    id
                );
            }
        } else {
            info!("InputMethod: activate_input_method - WARNING: inner.active_input_method_id is None after activation");
        }
    }

    /// Deactivate the active input method.
    ///
    /// The `done` is always send when deactivating IME.
    pub fn deactivate_input_method<D: SeatHandler + 'static>(&self, state: &mut D) {
        info!("InputMethod: deactivate_input_method - START");

        self.with_input_method(|im| {
            let active_id = match im.active_input_method_id.clone() {
                Some(id) => {
                    info!(
                        "InputMethod: deactivate_input_method - deactivating instance {:?}",
                        id
                    );
                    id
                }
                None => {
                    info!("InputMethod: deactivate_input_method - no active instance to deactivate");
                    return;
                }
            };

            let instance = match im.instances.iter_mut().find(|i| i.object.id() == active_id) {
                Some(inst) => inst,
                None => return,
            };

            info!(
                "InputMethod: deactivate_input_method - sending deactivate() to instance {:?} (app_id: '{}')",
                instance.object.id(),
                instance.app_id
            );
            instance.object.deactivate();
            instance.done();

            if let Some(popup) = im.popup_handle.surface.as_mut() {
                let data = instance.object.data::<InputMethodUserData<D>>().unwrap();
                if popup.get_parent().is_some() {
                    (data.dismiss_popup)(state, popup.clone());
                }
                popup.set_parent(None);
            }
        });

        // Clear the active input method ID in the keyboard grab so it stops forwarding events
        let inner = self.inner.lock().unwrap();
        let mut keyboard_grab = inner.keyboard_grab.inner.lock().unwrap();
        keyboard_grab.active_input_method_id = None;
        info!("InputMethod: deactivate_input_method - COMPLETE - cleared active_input_method_id in keyboard grab");
    }
}

/// User data of ZwpInputMethodV2 object
pub struct InputMethodUserData<D: SeatHandler> {
    pub(super) handle: InputMethodHandle,
    pub(crate) text_input_handle: TextInputHandle,
    pub(crate) keyboard_handle: KeyboardHandle<D>,
    pub(crate) popup_geometry_callback: fn(&D, &WlSurface) -> Rectangle<i32, Logical>,
    pub(crate) new_popup: fn(&mut D, PopupSurface),
    pub(crate) popup_repositioned: fn(&mut D, PopupSurface),
    pub(crate) dismiss_popup: fn(&mut D, PopupSurface),
}

impl<D: SeatHandler> fmt::Debug for InputMethodUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputMethodUserData")
            .field("handle", &self.handle)
            .field("text_input_handle", &self.text_input_handle)
            .field("keyboard_handle", &self.keyboard_handle)
            .finish()
    }
}

impl<D> Dispatch<ZwpInputMethodV2, InputMethodUserData<D>, D> for InputMethodManagerState
where
    D: Dispatch<ZwpInputMethodV2, InputMethodUserData<D>>,
    D: Dispatch<ZwpInputPopupSurfaceV2, InputMethodPopupSurfaceUserData>,
    D: Dispatch<ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData<D>>,
    D: SeatHandler,
    D: InputMethodHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        seat: &ZwpInputMethodV2,
        request: zwp_input_method_v2::Request,
        data: &InputMethodUserData<D>,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_input_method_v2::Request::CommitString { text } => {
                data.text_input_handle.with_active_text_input(|ti, _surface| {
                    ti.commit_string(Some(text.clone()));
                });
            }
            zwp_input_method_v2::Request::SetPreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                data.text_input_handle.with_active_text_input(|ti, _surface| {
                    ti.preedit_string(Some(text.clone()), cursor_begin, cursor_end);
                });
            }
            zwp_input_method_v2::Request::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                data.text_input_handle.with_active_text_input(|ti, _surface| {
                    ti.delete_surrounding_text(before_length, after_length);
                });
            }
            zwp_input_method_v2::Request::Commit { serial } => {
                let inner = data.handle.inner.lock().unwrap();
                let current_serial = if let Some(active_id) = inner.active_input_method_id.clone() {
                    inner
                        .instances
                        .iter()
                        .find(|i| i.object.id() == active_id)
                        .map(|i| i.serial)
                        .unwrap_or(0)
                } else {
                    0
                };

                data.text_input_handle.done(serial != current_serial);
            }
            zwp_input_method_v2::Request::GetInputPopupSurface { id, surface } => {
                if compositor::give_role(&surface, INPUT_POPUP_SURFACE_ROLE).is_err()
                    && compositor::get_role(&surface) != Some(INPUT_POPUP_SURFACE_ROLE)
                {
                    // Protocol requires this raise an error, but doesn't define an error enum
                    seat.post_error(0u32, "Surface already has a role.");
                    return;
                }

                // Check if this input method is active before allowing popup creation
                let input_method = data.handle.inner.lock().unwrap();
                let requesting_id = seat.id();
                let is_active = input_method.active_input_method_id.as_ref() == Some(&requesting_id);

                if !is_active {
                    // Create the protocol object to satisfy the client, but don't track it
                    // This prevents protocol errors while still blocking the popup
                    let _instance = data_init.init(
                        id,
                        InputMethodPopupSurfaceUserData {
                            alive_tracker: AliveTracker::default(),
                        },
                    );
                    // Don't create PopupSurface or call state.new_popup() - popup remains invisible
                    drop(input_method);
                    return;
                }

                let parent = match data.text_input_handle.focus().clone() {
                    Some(parent) => {
                        let location = state.parent_geometry(&parent);
                        Some(PopupParent {
                            surface: parent,
                            location,
                        })
                    }
                    None => None,
                };
                drop(input_method);

                let instance = data_init.init(
                    id,
                    InputMethodPopupSurfaceUserData {
                        alive_tracker: AliveTracker::default(),
                    },
                );

                let mut input_method = data.handle.inner.lock().unwrap();
                let popup_rect = Arc::new(Mutex::new(input_method.popup_handle.rectangle));
                let popup = PopupSurface::new(instance, surface, popup_rect, parent);
                input_method.popup_handle.surface = Some(popup.clone());
                if popup.get_parent().is_some() {
                    state.new_popup(popup);
                }
            }
            zwp_input_method_v2::Request::GrabKeyboard { keyboard } => {
                let input_method = data.handle.inner.lock().unwrap();
                let keyboard_grab = input_method.keyboard_grab.clone();
                drop(input_method);

                // Check the current state before installing the grab
                let before_active_id = keyboard_grab.inner.lock().unwrap().active_input_method_id.clone();
                info!(
                    "InputMethod: GrabKeyboard request - active_input_method_id BEFORE grab installation: {:?}",
                    before_active_id
                );

                // Create the protocol object and store it
                let instance = data_init.init(
                    keyboard,
                    InputMethodKeyboardUserData {
                        handle: keyboard_grab.clone(),
                        keyboard_handle: data.keyboard_handle.clone(),
                    },
                );

                // Store the grab object in the instance
                let mut im = data.handle.inner.lock().unwrap();
                if let Some(inst) = im.instances.iter_mut().find(|i| i.object == *seat) {
                    inst.keyboard_grab = Some(instance.clone());
                    info!(
                        "InputMethod: GrabKeyboard - stored grab object in instance '{}'",
                        inst.app_id
                    );
                } else {
                    warn!("InputMethod: GrabKeyboard - could not find instance for resource");
                }

                // Also store in the shared keyboard grab for backward compatibility
                let mut keyboard = keyboard_grab.inner.lock().unwrap();
                keyboard.grab = Some(instance.clone());
                keyboard.text_input_handle = data.text_input_handle.clone();
                let after_active_id = keyboard.active_input_method_id.clone();
                drop(keyboard);
                drop(im);

                info!(
                    "InputMethod: GrabKeyboard - active_input_method_id AFTER storing grab object: {:?}",
                    after_active_id
                );

                // Install the keyboard grab permanently - it will check active_input_method_id internally
                data.keyboard_handle
                    .set_grab(state, keyboard_grab.clone(), SERIAL_COUNTER.next_serial());

                // Verify the state after installation
                let final_active_id = keyboard_grab.inner.lock().unwrap().active_input_method_id.clone();
                info!(
                    "InputMethod: GrabKeyboard - active_input_method_id AFTER set_grab: {:?}",
                    final_active_id
                );

                // Send keyboard information to the client
                let guard = data.keyboard_handle.arc.internal.lock().unwrap();
                instance.repeat_info(guard.repeat_rate, guard.repeat_delay);
                let keymap_file = data.keyboard_handle.arc.keymap.lock().unwrap();
                let res = keymap_file.with_fd(false, |fd, size| {
                    instance.keymap(KeymapFormat::XkbV1, fd, size as u32);
                });

                if let Err(err) = res {
                    warn!(err = ?err, "Failed to send keymap to client");
                } else {
                    // Modifiers can be latched when taking the grab, thus we must send them to keep
                    // them in sync.
                    let mods = guard.mods_state.serialized;
                    instance.modifiers(
                        SERIAL_COUNTER.next_serial().into(),
                        mods.depressed,
                        mods.latched,
                        mods.locked,
                        mods.layout_effective,
                    );
                }

                info!("InputMethod: GrabKeyboard - keyboard grab object created and grab installed");
            }
            zwp_input_method_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        input_method: &ZwpInputMethodV2,
        data: &InputMethodUserData<D>,
    ) {
        let destroyed_id = input_method.id();
        let mut inner = data.handle.inner.lock().unwrap();

        // Clear active ID if this was the active instance
        if inner.active_input_method_id.as_ref() == Some(&destroyed_id) {
            inner.active_input_method_id = None;
        }

        inner.instances.retain(|inst| inst.object.id() != destroyed_id);
        drop(inner);

        data.text_input_handle.leave();
    }
}
