//! Utilities for text-input-v1 and input-method-v1 support.
//!
//! This module wires together the unstable `text-input-v1`,
//! `input-method-v1` and `input-panel-v1` protocols.
//!
//! The high-level integration pattern is:
//! - Implement [`InputMethodV1Handler`] for your compositor state.
//! - Create globals through [`InputMethodV1ManagerState::new`].
//! - Delegate protocol dispatch with [`delegate_input_method_manager_v1!`].

use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::warn;
use wayland_protocols::wp::input_method::zv1::server::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
    zwp_input_panel_surface_v1::{self, ZwpInputPanelSurfaceV1},
    zwp_input_panel_v1::{self, ZwpInputPanelV1},
};
use wayland_protocols::wp::text_input::zv1::server::{
    zwp_text_input_manager_v1::{self, ZwpTextInputManagerV1},
    zwp_text_input_v1::{self, ZwpTextInputV1},
};
use wayland_server::{
    backend::{ClientId, GlobalId},
    protocol::{
        wl_keyboard::{self, KeymapFormat, WlKeyboard},
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::input::{
    keyboard::{
        GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle, KeyboardInnerHandle,
        ModifiersState,
    },
    Seat, SeatHandler,
};
use crate::utils::{Logical, Point, Rectangle, SERIAL_COUNTER};
use crate::wayland::compositor;
use crate::wayland::seat::keyboard::for_each_focused_kbds;

const INPUT_METHOD_VERSION: u32 = 1;
const TEXT_INPUT_VERSION: u32 = 1;
const INPUT_PANEL_VERSION: u32 = 1;
const INPUT_PANEL_SURFACE_ROLE: &str = "zwp_input_panel_surface_v1";

/// Parent surface and geometry used to place an IME popup.
#[derive(Debug, Clone)]
pub struct PopupParent {
    /// The parent `wl_surface` the popup is associated with.
    pub surface: WlSurface,
    /// Parent geometry in compositor logical coordinates.
    pub location: Rectangle<i32, Logical>,
}

/// Handle to an input panel popup surface (`zwp_input_panel_surface_v1`).
#[derive(Debug, Clone)]
pub struct PopupSurface {
    surface_role: ZwpInputPanelSurfaceV1,
    surface: WlSurface,
    rectangle: Arc<Mutex<Rectangle<i32, Logical>>>,
    location: Arc<Mutex<Point<i32, Logical>>>,
    parent: Option<PopupParent>,
}

impl PopupSurface {
    fn new(surface_role: ZwpInputPanelSurfaceV1, surface: WlSurface, parent: Option<PopupParent>) -> Self {
        Self {
            surface_role,
            surface,
            rectangle: Arc::new(Mutex::new(Rectangle::default())),
            location: Arc::new(Mutex::new(Point::default())),
            parent,
        }
    }

    /// Returns `true` if both the role object and `wl_surface` are still alive.
    pub fn alive(&self) -> bool {
        self.surface_role.is_alive() && self.surface.is_alive()
    }

    /// Access the underlying `wl_surface`.
    pub fn wl_surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Access the current popup parent, if any.
    pub fn get_parent(&self) -> Option<&PopupParent> {
        self.parent.as_ref()
    }

    /// Update the popup parent.
    pub fn set_parent(&mut self, parent: Option<PopupParent>) {
        self.parent = parent;
    }

    /// Popup location relative to its parent.
    pub fn location(&self) -> Point<i32, Logical> {
        *self.location.lock().unwrap()
    }

    /// Set popup location relative to its parent.
    pub fn set_location(&self, location: Point<i32, Logical>) {
        *self.location.lock().unwrap() = location;
    }

    /// Current text cursor rectangle associated with the popup.
    pub fn text_input_rectangle(&self) -> Rectangle<i32, Logical> {
        *self.rectangle.lock().unwrap()
    }

    /// Set the text cursor rectangle associated with the popup.
    pub fn set_text_input_rectangle(&self, rect: Rectangle<i32, Logical>) {
        *self.rectangle.lock().unwrap() = rect;
    }
}

impl PartialEq for PopupSurface {
    fn eq(&self, other: &Self) -> bool {
        self.surface_role == other.surface_role
    }
}

/// Callbacks used by the v1 input method helpers to manage popup lifecycle.
pub trait InputMethodV1Handler {
    /// A new popup should be added to compositor state.
    fn new_popup(&mut self, surface: PopupSurface);
    /// A popup should be removed from compositor state.
    fn dismiss_popup(&mut self, surface: PopupSurface);
    /// Popup location or geometry changed.
    fn popup_repositioned(&mut self, surface: PopupSurface);
    /// Returns geometry for a parent surface used for popup placement.
    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical>;
}

#[derive(Clone, Default, Debug)]
struct TextInputV1State {
    active: bool,
    seat: Option<WlSeat>,
    surface: Option<WlSurface>,
    serial: u32,
    cursor_rectangle: Rectangle<i32, Logical>,
}

#[derive(Default)]
struct InputMethodV1KeyboardGrabState {
    grab: Option<WlKeyboard>,
}

#[derive(Default, Clone)]
struct InputMethodV1KeyboardGrab {
    inner: Arc<Mutex<InputMethodV1KeyboardGrabState>>,
}

impl<D> KeyboardGrab<D> for InputMethodV1KeyboardGrab
where
    D: SeatHandler + 'static,
{
    fn input(
        &mut self,
        _data: &mut D,
        _handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: crate::backend::input::Keycode,
        key_state: crate::backend::input::KeyState,
        modifiers: Option<ModifiersState>,
        serial: crate::utils::Serial,
        time: u32,
    ) {
        let Some(kbd) = self.inner.lock().unwrap().grab.clone() else {
            return;
        };

        let serial = serial.0;
        kbd.key(serial, time, keycode.raw() - 8, key_state.into());

        if let Some(serialized) = modifiers.map(|m| m.serialized) {
            kbd.modifiers(
                serial,
                serialized.depressed,
                serialized.latched,
                serialized.locked,
                serialized.layout_effective,
            );
        }
    }

    fn set_focus(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<<D as SeatHandler>::KeyboardFocus>,
        serial: crate::utils::Serial,
    ) {
        handle.set_focus(data, focus, serial)
    }

    fn start_data(&self) -> &KeyboardGrabStartData<D> {
        &KeyboardGrabStartData { focus: None }
    }

    fn unset(&mut self, _data: &mut D) {}
}

#[derive(Default)]
struct ActiveContext {
    context: Option<ZwpInputMethodContextV1>,
    text_input: Option<ZwpTextInputV1>,
    seat: Option<WlSeat>,
    surface: Option<WlSurface>,
}

#[derive(Default)]
struct InputMethodV1Inner {
    input_method: Option<ZwpInputMethodV1>,
    text_inputs: Vec<ZwpTextInputV1>,
    active: ActiveContext,
    keyboard_grab: InputMethodV1KeyboardGrab,
    popup: Option<PopupSurface>,
    popup_enabled: bool,
    popup_visible: bool,
}

#[derive(Default, Clone)]
/// Internal handle shared between text-input-v1 and input-method-v1 objects.
pub struct InputMethodV1Handle {
    inner: Arc<Mutex<InputMethodV1Inner>>,
}

impl fmt::Debug for InputMethodV1Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputMethodV1Handle").finish()
    }
}

impl InputMethodV1Handle {
    fn add_text_input(&self, text_input: &ZwpTextInputV1) {
        self.inner.lock().unwrap().text_inputs.push(text_input.clone());
    }

    fn remove_text_input(&self, text_input: &ZwpTextInputV1) {
        self.inner
            .lock()
            .unwrap()
            .text_inputs
            .retain(|ti| ti.id() != text_input.id());
    }

    fn hide_popup<D: SeatHandler + InputMethodV1Handler + 'static>(&self, state: &mut D) {
        let (popup, im) = {
            let mut inner = self.inner.lock().unwrap();
            if !inner.popup_visible {
                return;
            }
            inner.popup_visible = false;
            let popup = inner.popup.clone();
            let im = inner.input_method.clone();
            if let Some(p) = inner.popup.as_mut() {
                p.set_parent(None);
            }
            (popup, im)
        };

        let (Some(popup), Some(im)) = (popup, im) else {
            return;
        };
        let data = im.data::<InputMethodUserData<D>>().unwrap();
        (data.dismiss_popup)(state, popup);
    }

    fn refresh_popup<D: SeatHandler + InputMethodV1Handler + 'static>(&self, state: &mut D) {
        let (mut popup, im, parent_surface, text_rect, was_visible, enabled) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.popup.clone(),
                inner.input_method.clone(),
                inner.active.surface.clone(),
                inner
                    .active
                    .text_input
                    .as_ref()
                    .and_then(|ti| ti.data::<TextInputUserData>())
                    .map(|ud| ud.state.lock().unwrap().cursor_rectangle)
                    .unwrap_or_default(),
                inner.popup_visible,
                inner.popup_enabled,
            )
        };

        let (Some(mut popup_value), Some(im), Some(parent_surface)) = (popup.take(), im, parent_surface)
        else {
            self.hide_popup(state);
            return;
        };

        if !enabled {
            self.hide_popup(state);
            return;
        }

        let data = im.data::<InputMethodUserData<D>>().unwrap();
        let parent = PopupParent {
            location: (data.popup_geometry_callback)(state, &parent_surface),
            surface: parent_surface,
        };

        popup_value.set_parent(Some(parent));
        popup_value.set_text_input_rectangle(text_rect);
        popup_value.set_location((text_rect.loc.x, text_rect.loc.y + text_rect.size.h).into());

        {
            let mut inner = self.inner.lock().unwrap();
            inner.popup = Some(popup_value.clone());
            inner.popup_visible = true;
        }

        if was_visible {
            (data.popup_repositioned)(state, popup_value);
        } else {
            (data.new_popup)(state, popup_value);
        }
    }

    fn deactivate_if_active_for<D: SeatHandler + InputMethodV1Handler + 'static>(
        &self,
        state: &mut D,
        text_input: &ZwpTextInputV1,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if inner
            .active
            .text_input
            .as_ref()
            .is_some_and(|ti| ti.id() == text_input.id())
        {
            let active_seat = inner.active.seat.clone();
            if let (Some(im), Some(ctx)) = (inner.input_method.as_ref(), inner.active.context.as_ref()) {
                im.deactivate(ctx);
            }
            inner.active = ActiveContext::default();
            drop(inner);
            self.hide_popup(state);

            if let Some(seat) = active_seat.and_then(|s| Seat::<D>::from_resource(&s)) {
                if let Some(keyboard) = seat.get_keyboard() {
                    keyboard.unset_grab(state);
                }
            }
        }
    }
}

#[derive(Debug)]
/// User data attached to `zwp_text_input_v1`.
pub struct TextInputUserData {
    handle: InputMethodV1Handle,
    state: Arc<Mutex<TextInputV1State>>,
}

#[derive(Debug)]
/// User data attached to `zwp_input_method_v1`.
pub struct InputMethodUserData<D: SeatHandler> {
    handle: InputMethodV1Handle,
    popup_geometry_callback: fn(&D, &WlSurface) -> Rectangle<i32, Logical>,
    new_popup: fn(&mut D, PopupSurface),
    popup_repositioned: fn(&mut D, PopupSurface),
    dismiss_popup: fn(&mut D, PopupSurface),
}

#[derive(Debug)]
/// User data attached to `zwp_input_method_context_v1`.
pub struct InputMethodContextUserData {
    handle: InputMethodV1Handle,
}

#[derive(Debug)]
/// User data attached to keyboard resources grabbed by input-method-v1.
pub struct InputMethodKeyboardUserData<D: SeatHandler> {
    handle: InputMethodV1Handle,
    keyboard_handle: KeyboardHandle<D>,
}

#[derive(Debug)]
/// User data attached to `zwp_input_panel_v1`.
pub struct InputPanelUserData {
    handle: InputMethodV1Handle,
}

#[derive(Debug)]
/// User data attached to `zwp_input_panel_surface_v1`.
pub struct InputPanelSurfaceUserData {
    handle: InputMethodV1Handle,
}

#[derive(Clone)]
/// Global data shared by input-method-v1, text-input-v1 and input-panel-v1 globals.
pub struct InputMethodManagerGlobalData {
    filter: Arc<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
    handle: InputMethodV1Handle,
}

impl fmt::Debug for InputMethodManagerGlobalData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputMethodManagerGlobalData").finish()
    }
}

/// State of text-input-v1 + input-method-v1 protocols.
#[derive(Debug)]
pub struct InputMethodV1ManagerState {
    input_method_global: GlobalId,
    text_input_global: GlobalId,
    input_panel_global: GlobalId,
}

impl InputMethodV1ManagerState {
    /// Creates globals for `zwp_input_method_v1`, `zwp_input_panel_v1` and
    /// `zwp_text_input_manager_v1`.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwpInputMethodV1, InputMethodManagerGlobalData>,
        D: Dispatch<ZwpInputMethodV1, InputMethodUserData<D>>,
        D: Dispatch<ZwpInputMethodContextV1, InputMethodContextUserData>,
        D: Dispatch<WlKeyboard, InputMethodKeyboardUserData<D>>,
        D: GlobalDispatch<ZwpTextInputManagerV1, InputMethodManagerGlobalData>,
        D: Dispatch<ZwpTextInputManagerV1, InputMethodV1Handle>,
        D: Dispatch<ZwpTextInputV1, TextInputUserData>,
        D: GlobalDispatch<ZwpInputPanelV1, InputMethodManagerGlobalData>,
        D: Dispatch<ZwpInputPanelV1, InputPanelUserData>,
        D: Dispatch<ZwpInputPanelSurfaceV1, InputPanelSurfaceUserData>,
        D: SeatHandler + InputMethodV1Handler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let filter = InputMethodManagerGlobalData {
            filter: Arc::new(filter),
            handle: InputMethodV1Handle::default(),
        };

        let input_method_global =
            display.create_global::<D, ZwpInputMethodV1, _>(INPUT_METHOD_VERSION, filter.clone());
        let input_panel_global =
            display.create_global::<D, ZwpInputPanelV1, _>(INPUT_PANEL_VERSION, filter.clone());
        let text_input_global =
            display.create_global::<D, ZwpTextInputManagerV1, _>(TEXT_INPUT_VERSION, filter);

        Self {
            input_method_global,
            text_input_global,
            input_panel_global,
        }
    }

    /// Returns the global id of `zwp_input_method_v1`.
    pub fn input_method_global(&self) -> GlobalId {
        self.input_method_global.clone()
    }

    /// Returns the global id of `zwp_text_input_manager_v1`.
    pub fn text_input_global(&self) -> GlobalId {
        self.text_input_global.clone()
    }

    /// Returns the global id of `zwp_input_panel_v1`.
    pub fn input_panel_global(&self) -> GlobalId {
        self.input_panel_global.clone()
    }
}

impl<D> GlobalDispatch<ZwpInputMethodV1, InputMethodManagerGlobalData, D> for InputMethodV1ManagerState
where
    D: GlobalDispatch<ZwpInputMethodV1, InputMethodManagerGlobalData>,
    D: Dispatch<ZwpInputMethodV1, InputMethodUserData<D>>,
    D: SeatHandler + InputMethodV1Handler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpInputMethodV1>,
        data: &InputMethodManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(
            resource,
            InputMethodUserData {
                handle: data.handle.clone(),
                popup_geometry_callback: D::parent_geometry,
                new_popup: D::new_popup,
                popup_repositioned: D::popup_repositioned,
                dismiss_popup: D::dismiss_popup,
            },
        );

        let mut inner = data.handle.inner.lock().unwrap();
        let _ = inner.input_method.take();
        inner.input_method = Some(instance);
    }

    fn can_view(client: Client, global_data: &InputMethodManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> GlobalDispatch<ZwpInputPanelV1, InputMethodManagerGlobalData, D> for InputMethodV1ManagerState
where
    D: GlobalDispatch<ZwpInputPanelV1, InputMethodManagerGlobalData>,
    D: Dispatch<ZwpInputPanelV1, InputPanelUserData>,
    D: SeatHandler,
    D: 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpInputPanelV1>,
        data: &InputMethodManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(
            resource,
            InputPanelUserData {
                handle: data.handle.clone(),
            },
        );
    }

    fn can_view(client: Client, global_data: &InputMethodManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> GlobalDispatch<ZwpTextInputManagerV1, InputMethodManagerGlobalData, D> for InputMethodV1ManagerState
where
    D: GlobalDispatch<ZwpTextInputManagerV1, InputMethodManagerGlobalData>,
    D: Dispatch<ZwpTextInputManagerV1, InputMethodV1Handle>,
    D: Dispatch<ZwpTextInputV1, TextInputUserData>,
    D: 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpTextInputManagerV1>,
        data: &InputMethodManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, data.handle.clone());
    }

    fn can_view(client: Client, global_data: &InputMethodManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZwpTextInputManagerV1, InputMethodV1Handle, D> for InputMethodV1ManagerState
where
    D: Dispatch<ZwpTextInputManagerV1, InputMethodV1Handle>,
    D: Dispatch<ZwpTextInputV1, TextInputUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ZwpTextInputManagerV1,
        request: zwp_text_input_manager_v1::Request,
        handle: &InputMethodV1Handle,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        if let zwp_text_input_manager_v1::Request::CreateTextInput { id } = request {
            let state = Arc::new(Mutex::new(TextInputV1State::default()));
            let text_input = data_init.init(
                id,
                TextInputUserData {
                    handle: handle.clone(),
                    state,
                },
            );
            handle.add_text_input(&text_input);
        }
    }
}

impl<D> Dispatch<ZwpTextInputV1, TextInputUserData, D> for InputMethodV1ManagerState
where
    D: Dispatch<ZwpTextInputV1, TextInputUserData>,
    D: Dispatch<ZwpInputMethodContextV1, InputMethodContextUserData>,
    D: Dispatch<WlKeyboard, InputMethodKeyboardUserData<D>>,
    D: SeatHandler + InputMethodV1Handler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ZwpTextInputV1,
        request: zwp_text_input_v1::Request,
        data: &TextInputUserData,
        dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_text_input_v1::Request::Activate { seat, surface } => {
                let mut ti_state = data.state.lock().unwrap();
                ti_state.active = true;
                ti_state.seat = Some(seat.clone());
                ti_state.surface = Some(surface.clone());
                drop(ti_state);

                resource.enter(&surface);

                let mut inner = data.handle.inner.lock().unwrap();
                let Some(im) = inner.input_method.clone() else {
                    return;
                };

                if let Some(ctx) = inner.active.context.take() {
                    im.deactivate(&ctx);
                }

                let Some(client) = im.client() else {
                    return;
                };

                let Ok(context) = client.create_resource::<ZwpInputMethodContextV1, _, D>(
                    dhandle,
                    1,
                    InputMethodContextUserData {
                        handle: data.handle.clone(),
                    },
                ) else {
                    return;
                };

                im.activate(&context);

                inner.active.context = Some(context);
                inner.active.text_input = Some(resource.clone());
                inner.active.seat = Some(seat);
                inner.active.surface = Some(surface);

                if let Some(kbd_handle) = Seat::<D>::from_resource(inner.active.seat.as_ref().unwrap())
                    .and_then(|s| s.get_keyboard())
                {
                    kbd_handle.unset_grab(state);
                }
                drop(inner);
                data.handle.refresh_popup(state);
            }
            zwp_text_input_v1::Request::Deactivate { seat } => {
                let mut ti_state = data.state.lock().unwrap();
                ti_state.active = false;
                if ti_state.seat.as_ref().is_some_and(|s| s != &seat) {
                    return;
                }
                drop(ti_state);

                resource.leave();
                data.handle.deactivate_if_active_for(state, resource);
            }
            zwp_text_input_v1::Request::ShowInputPanel => {
                resource.input_panel_state(1);
            }
            zwp_text_input_v1::Request::HideInputPanel => {
                resource.input_panel_state(0);
            }
            zwp_text_input_v1::Request::Reset => {
                if let Some(ctx) = data.handle.inner.lock().unwrap().active.context.as_ref() {
                    ctx.reset();
                }
            }
            zwp_text_input_v1::Request::SetSurroundingText { text, cursor, anchor } => {
                if let Some(ctx) = data.handle.inner.lock().unwrap().active.context.as_ref() {
                    ctx.surrounding_text(text, cursor, anchor);
                }
            }
            zwp_text_input_v1::Request::SetContentType { hint, purpose } => {
                if let Some(ctx) = data.handle.inner.lock().unwrap().active.context.as_ref() {
                    let hint = u32::from(hint.into_result().unwrap_or(zwp_text_input_v1::ContentHint::None));
                    let purpose = u32::from(
                        purpose
                            .into_result()
                            .unwrap_or(zwp_text_input_v1::ContentPurpose::Normal),
                    );
                    ctx.content_type(hint, purpose);
                }
            }
            zwp_text_input_v1::Request::SetPreferredLanguage { language } => {
                if let Some(ctx) = data.handle.inner.lock().unwrap().active.context.as_ref() {
                    ctx.preferred_language(language);
                }
            }
            zwp_text_input_v1::Request::CommitState { serial } => {
                data.state.lock().unwrap().serial = serial;
                if let Some(ctx) = data.handle.inner.lock().unwrap().active.context.as_ref() {
                    ctx.commit_state(serial);
                }
            }
            zwp_text_input_v1::Request::InvokeAction { button, index } => {
                if let Some(ctx) = data.handle.inner.lock().unwrap().active.context.as_ref() {
                    ctx.invoke_action(button, index);
                }
            }
            zwp_text_input_v1::Request::SetCursorRectangle { x, y, width, height } => {
                data.state.lock().unwrap().cursor_rectangle =
                    Rectangle::new((x, y).into(), (width, height).into());
                data.handle.refresh_popup(state);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut D, _client: ClientId, resource: &ZwpTextInputV1, data: &TextInputUserData) {
        data.handle.deactivate_if_active_for(state, resource);
        data.handle.remove_text_input(resource);

        let maybe_seat = data.state.lock().unwrap().seat.clone();
        if let Some(seat) = maybe_seat.and_then(|s| Seat::<D>::from_resource(&s)) {
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.unset_grab(state);
            }
        }
    }
}

impl<D> Dispatch<ZwpInputMethodV1, InputMethodUserData<D>, D> for InputMethodV1ManagerState
where
    D: Dispatch<ZwpInputMethodV1, InputMethodUserData<D>>,
    D: SeatHandler + InputMethodV1Handler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ZwpInputMethodV1,
        _request: zwp_input_method_v1::Request,
        _data: &InputMethodUserData<D>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        resource: &ZwpInputMethodV1,
        data: &InputMethodUserData<D>,
    ) {
        let active_seat = {
            let inner = data.handle.inner.lock().unwrap();
            if !inner
                .input_method
                .as_ref()
                .is_some_and(|im| im.id() == resource.id())
            {
                return;
            }
            inner.active.seat.clone()
        };

        data.handle.hide_popup(state);

        {
            let mut inner = data.handle.inner.lock().unwrap();
            if !inner
                .input_method
                .as_ref()
                .is_some_and(|im| im.id() == resource.id())
            {
                return;
            }
            inner.input_method = None;
            inner.active = ActiveContext::default();
        }

        if let Some(seat) = active_seat.and_then(|s| Seat::<D>::from_resource(&s)) {
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.unset_grab(state);
            }
        }
    }
}

impl<D> Dispatch<ZwpInputMethodContextV1, InputMethodContextUserData, D> for InputMethodV1ManagerState
where
    D: Dispatch<ZwpInputMethodContextV1, InputMethodContextUserData>,
    D: Dispatch<WlKeyboard, InputMethodKeyboardUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ZwpInputMethodContextV1,
        request: zwp_input_method_context_v1::Request,
        data: &InputMethodContextUserData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let is_active_context = {
            let inner = data.handle.inner.lock().unwrap();
            inner
                .active
                .context
                .as_ref()
                .is_some_and(|ctx| ctx.id() == resource.id())
        };
        if !is_active_context {
            return;
        }

        match request {
            zwp_input_method_context_v1::Request::CommitString { serial, text } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.commit_string(serial, text);
                }
            }
            zwp_input_method_context_v1::Request::PreeditString { serial, text, commit } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.preedit_string(serial, text, commit);
                }
            }
            zwp_input_method_context_v1::Request::PreeditStyling { index, length, style } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    let style = style
                        .try_into()
                        .unwrap_or(zwp_text_input_v1::PreeditStyle::Default);
                    ti.preedit_styling(index, length, style);
                }
            }
            zwp_input_method_context_v1::Request::PreeditCursor { index } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.preedit_cursor(index);
                }
            }
            zwp_input_method_context_v1::Request::DeleteSurroundingText { index, length } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.delete_surrounding_text(index, length);
                }
            }
            zwp_input_method_context_v1::Request::CursorPosition { index, anchor } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.cursor_position(index, anchor);
                }
            }
            zwp_input_method_context_v1::Request::ModifiersMap { map } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.modifiers_map(map.to_vec());
                }
            }
            zwp_input_method_context_v1::Request::Keysym {
                serial,
                time,
                sym,
                state,
                modifiers,
            } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.keysym(serial, time, sym, state, modifiers);
                }
            }
            zwp_input_method_context_v1::Request::Language { serial, language } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    ti.language(serial, language);
                }
            }
            zwp_input_method_context_v1::Request::TextDirection { serial, direction } => {
                if let Some(ti) = data.handle.inner.lock().unwrap().active.text_input.as_ref() {
                    let direction = direction
                        .try_into()
                        .unwrap_or(zwp_text_input_v1::TextDirection::Auto);
                    ti.text_direction(serial, direction);
                }
            }
            zwp_input_method_context_v1::Request::GrabKeyboard { keyboard } => {
                let inner = data.handle.inner.lock().unwrap();
                let Some(seat_resource) = inner.active.seat.clone() else {
                    return;
                };
                let Some(seat) = Seat::<D>::from_resource(&seat_resource) else {
                    return;
                };
                let Some(keyboard_handle) = seat.get_keyboard() else {
                    return;
                };

                keyboard_handle.set_grab(state, inner.keyboard_grab.clone(), SERIAL_COUNTER.next_serial());

                let instance = data_init.init(
                    keyboard,
                    InputMethodKeyboardUserData {
                        handle: data.handle.clone(),
                        keyboard_handle: keyboard_handle.clone(),
                    },
                );

                inner.keyboard_grab.inner.lock().unwrap().grab = Some(instance.clone());

                let guard = keyboard_handle.arc.internal.lock().unwrap();
                instance.repeat_info(guard.repeat_rate, guard.repeat_delay);
                let keymap_file = keyboard_handle.arc.keymap.lock().unwrap();
                let res = keymap_file.with_fd(false, |fd, size| {
                    instance.keymap(KeymapFormat::XkbV1, fd, size as u32);
                });
                if let Err(err) = res {
                    warn!(err = ?err, "failed to send v1 IME keymap");
                } else {
                    let mods = guard.mods_state.serialized;
                    instance.modifiers(
                        SERIAL_COUNTER.next_serial().0,
                        mods.depressed,
                        mods.latched,
                        mods.locked,
                        mods.layout_effective,
                    );
                }
            }
            zwp_input_method_context_v1::Request::Key {
                serial,
                time,
                key,
                state,
            } => {
                let inner = data.handle.inner.lock().unwrap();
                let Some(seat_resource) = inner.active.seat.as_ref() else {
                    return;
                };
                let Some(surface) = inner.active.surface.as_ref() else {
                    return;
                };
                let Some(seat) = Seat::<D>::from_resource(seat_resource) else {
                    return;
                };
                for_each_focused_kbds(&seat, surface, |kbd| {
                    let key_state = if state == 1 {
                        wl_keyboard::KeyState::Pressed
                    } else {
                        wl_keyboard::KeyState::Released
                    };
                    kbd.key(serial, time, key, key_state);
                });
            }
            zwp_input_method_context_v1::Request::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                let inner = data.handle.inner.lock().unwrap();
                let Some(seat_resource) = inner.active.seat.as_ref() else {
                    return;
                };
                let Some(surface) = inner.active.surface.as_ref() else {
                    return;
                };
                let Some(seat) = Seat::<D>::from_resource(seat_resource) else {
                    return;
                };
                for_each_focused_kbds(&seat, surface, |kbd| {
                    kbd.modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
                });
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        resource: &ZwpInputMethodContextV1,
        data: &InputMethodContextUserData,
    ) {
        let active_seat = {
            let mut inner = data.handle.inner.lock().unwrap();
            let is_active_context = inner
                .active
                .context
                .as_ref()
                .is_some_and(|ctx| ctx.id() == resource.id());
            if is_active_context {
                let active_seat = inner.active.seat.clone();
                inner.active = ActiveContext::default();
                active_seat
            } else {
                None
            }
        };

        if let Some(seat) = active_seat.and_then(|s| Seat::<D>::from_resource(&s)) {
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.unset_grab(state);
            }
        }
    }
}

impl<D> Dispatch<WlKeyboard, InputMethodKeyboardUserData<D>, D> for InputMethodV1ManagerState
where
    D: Dispatch<WlKeyboard, InputMethodKeyboardUserData<D>>,
    D: SeatHandler + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &WlKeyboard,
        _request: wl_keyboard::Request,
        _data: &InputMethodKeyboardUserData<D>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        resource: &WlKeyboard,
        data: &InputMethodKeyboardUserData<D>,
    ) {
        let should_unset = {
            let inner = data.handle.inner.lock().unwrap();
            let mut grab = inner.keyboard_grab.inner.lock().unwrap();
            if grab.grab.as_ref().is_some_and(|g| g.id() == resource.id()) {
                grab.grab = None;
                true
            } else {
                false
            }
        };
        if should_unset {
            data.keyboard_handle.unset_grab(state);
        }
    }
}

impl<D> Dispatch<ZwpInputPanelV1, InputPanelUserData, D> for InputMethodV1ManagerState
where
    D: Dispatch<ZwpInputPanelV1, InputPanelUserData>,
    D: Dispatch<ZwpInputPanelSurfaceV1, InputPanelSurfaceUserData>,
    D: SeatHandler + InputMethodV1Handler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ZwpInputPanelV1,
        request: zwp_input_panel_v1::Request,
        data: &InputPanelUserData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        if let zwp_input_panel_v1::Request::GetInputPanelSurface { id, surface } = request {
            if compositor::give_role(&surface, INPUT_PANEL_SURFACE_ROLE).is_err()
                && compositor::get_role(&surface) != Some(INPUT_PANEL_SURFACE_ROLE)
            {
                // Protocol requires this raise an error, but doesn't define an error enum.
                resource.post_error(0u32, "Surface already has a role.");
                return;
            }

            let panel_surface = data_init.init(
                id,
                InputPanelSurfaceUserData {
                    handle: data.handle.clone(),
                },
            );

            let parent = {
                let inner = data.handle.inner.lock().unwrap();
                inner.active.surface.as_ref().map(|s| PopupParent {
                    surface: s.clone(),
                    location: Rectangle::default(),
                })
            };

            data.handle.hide_popup(state);

            let popup = PopupSurface::new(panel_surface, surface, parent);
            {
                let mut inner = data.handle.inner.lock().unwrap();
                inner.popup = Some(popup);
                inner.popup_enabled = false;
                inner.popup_visible = false;
            }
            data.handle.refresh_popup(state);
        }
    }
}

impl<D> Dispatch<ZwpInputPanelSurfaceV1, InputPanelSurfaceUserData, D> for InputMethodV1ManagerState
where
    D: Dispatch<ZwpInputPanelSurfaceV1, InputPanelSurfaceUserData>,
    D: SeatHandler + InputMethodV1Handler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ZwpInputPanelSurfaceV1,
        request: zwp_input_panel_surface_v1::Request,
        data: &InputPanelSurfaceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let is_active_popup_surface = {
            let inner = data.handle.inner.lock().unwrap();
            inner
                .popup
                .as_ref()
                .is_some_and(|popup| popup.surface_role.id() == resource.id())
        };
        if !is_active_popup_surface {
            return;
        }

        match request {
            zwp_input_panel_surface_v1::Request::SetOverlayPanel => {
                data.handle.inner.lock().unwrap().popup_enabled = true;
                data.handle.refresh_popup(state);
            }
            zwp_input_panel_surface_v1::Request::SetToplevel { .. } => {
                data.handle.inner.lock().unwrap().popup_enabled = true;
                data.handle.refresh_popup(state);
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        resource: &ZwpInputPanelSurfaceV1,
        data: &InputPanelSurfaceUserData,
    ) {
        let is_active_popup_surface = {
            let inner = data.handle.inner.lock().unwrap();
            inner
                .popup
                .as_ref()
                .is_some_and(|popup| popup.surface_role.id() == resource.id())
        };
        if !is_active_popup_surface {
            return;
        }

        {
            let mut inner = data.handle.inner.lock().unwrap();
            inner.popup = None;
            inner.popup_enabled = false;
        }
        data.handle.hide_popup(state);
    }
}

#[allow(missing_docs)]
#[macro_export]
macro_rules! delegate_input_method_manager_v1 {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        const _: () = {
            use $crate::{
                reexports::{
                    wayland_protocols::wp::{
                        input_method::zv1::server::{
                            zwp_input_method_context_v1::ZwpInputMethodContextV1,
                            zwp_input_method_v1::ZwpInputMethodV1,
                            zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1,
                            zwp_input_panel_v1::ZwpInputPanelV1,
                        },
                        text_input::zv1::server::{
                            zwp_text_input_manager_v1::ZwpTextInputManagerV1,
                            zwp_text_input_v1::ZwpTextInputV1,
                        },
                    },
                    wayland_server::{
                        delegate_dispatch, delegate_global_dispatch,
                        protocol::wl_keyboard::WlKeyboard,
                    },
                },
                wayland::input_method_v1::{
                    InputMethodContextUserData, InputMethodKeyboardUserData,
                    InputMethodManagerGlobalData, InputMethodUserData, InputMethodV1Handle,
                    InputMethodV1ManagerState, InputPanelSurfaceUserData, InputPanelUserData,
                    TextInputUserData,
                },
            };

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpInputMethodV1: InputMethodManagerGlobalData] => InputMethodV1ManagerState
            );

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpInputPanelV1: InputMethodManagerGlobalData] => InputMethodV1ManagerState
            );

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpTextInputManagerV1: InputMethodManagerGlobalData] => InputMethodV1ManagerState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpInputMethodV1: InputMethodUserData<Self>] => InputMethodV1ManagerState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpInputMethodContextV1: InputMethodContextUserData] => InputMethodV1ManagerState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [WlKeyboard: InputMethodKeyboardUserData<Self>] => InputMethodV1ManagerState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpInputPanelV1: InputPanelUserData] => InputMethodV1ManagerState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpInputPanelSurfaceV1: InputPanelSurfaceUserData] => InputMethodV1ManagerState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpTextInputManagerV1: InputMethodV1Handle] => InputMethodV1ManagerState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwpTextInputV1: TextInputUserData] => InputMethodV1ManagerState
            );
        };
    };
}
