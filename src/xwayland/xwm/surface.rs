use crate::{
    backend::{input::KeyState, renderer::element::Id},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget},
        Seat, SeatHandler,
    },
    utils::{user_data::UserDataMap, IsAlive, Logical, Point, Rectangle, Serial, Size},
};
use encoding::{DecoderTrap, Encoding};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, Weak},
};
use wayland_server::protocol::wl_surface::WlSurface;

use x11rb::{
    connection::Connection as _,
    properties::{WmClass, WmHints, WmSizeHints},
    protocol::xproto::{
        Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus,
        PropMode, Window as X11Window,
    },
    rust_connection::{ConnectionError, RustConnection},
    wrapper::ConnectionExt,
};

#[derive(Debug, Clone)]
pub struct X11Surface {
    pub(super) window: X11Window,
    pub(super) override_redirect: bool,
    pub(super) conn: Weak<RustConnection>,
    pub(super) atoms: super::Atoms,
    pub(crate) state: Arc<Mutex<SharedSurfaceState>>,
    pub(super) user_data: Arc<UserDataMap>,
    pub(super) log: slog::Logger,
}

#[derive(Debug)]
pub(crate) struct SharedSurfaceState {
    pub(super) alive: bool,
    pub(crate) wl_surface: Option<WlSurface>,
    pub(super) mapped_onto: Option<X11Window>,

    pub(super) location: Point<i32, Logical>,
    pub(super) size: Size<i32, Logical>,

    pub(super) title: String,
    pub(super) class: String,
    pub(super) instance: String,
    pub(super) protocols: Protocols,
    pub(super) hints: Option<WmHints>,
    pub(super) normal_hints: Option<WmSizeHints>,
    pub(super) transient_for: Option<X11Window>,
    pub(super) net_state: Vec<Atom>,
}

pub(super) type Protocols = Vec<WMProtocol>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WMProtocol {
    TakeFocus,
    DeleteWindow,
}

/// https://x.org/releases/X11R7.6/doc/xorg-docs/specs/ICCCM/icccm.html#input_focus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum InputMode {
    None,
    Passive,
    LocallyActive,
    GloballyActive,
}

impl PartialEq for X11Surface {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window
    }
}

#[derive(Debug, thiserror::Error)]
pub enum X11SurfaceError {
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error("Operation was unsupported for an override_redirect window")]
    UnsupportedForOverrideRedirect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WmWindowType {
    DropdownMenu,
    Dialog,
    Menu,
    Notification,
    Normal,
    PopupMenu,
    Splash,
    Toolbar,
    Tooltip,
    Utility,
}

impl X11Surface {
    pub fn set_mapped(&self, mapped: bool) -> Result<(), X11SurfaceError> {
        if self.override_redirect {
            if mapped {
                return Ok(());
            } else {
                return Err(X11SurfaceError::UnsupportedForOverrideRedirect);
            }
        }

        if let Some(conn) = self.conn.upgrade() {
            if let Some(frame) = self.state.lock().unwrap().mapped_onto {
                if mapped {
                    let property = [1u32 /*NormalState*/, 0 /*WINDOW_NONE*/];
                    conn.change_property32(
                        PropMode::REPLACE,
                        self.window,
                        self.atoms.WM_STATE,
                        self.atoms.WM_STATE,
                        &property,
                    )?;
                    conn.map_window(frame)?;
                } else {
                    let property = [3u32 /*IconicState*/, 0 /*WINDOW_NONE*/];
                    conn.change_property32(
                        PropMode::REPLACE,
                        self.window,
                        self.atoms.WM_STATE,
                        self.atoms.WM_STATE,
                        &property,
                    )?;
                    conn.unmap_window(frame)?;
                }
                conn.flush()?;
            }
        }
        Ok(())
    }

    pub fn is_override_redirect(&self) -> bool {
        self.override_redirect
    }

    pub fn is_mapped(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.wl_surface.is_some() || state.mapped_onto.is_some()
    }

    pub fn is_visible(&self) -> bool {
        let state = self.state.lock().unwrap();
        (self.override_redirect || state.mapped_onto.is_some()) && state.wl_surface.is_some()
    }

    pub fn alive(&self) -> bool {
        self.state.lock().unwrap().alive && self.conn.upgrade().is_some()
    }

    pub fn configure(&self, rect: impl Into<Option<Rectangle<i32, Logical>>>) -> Result<(), X11SurfaceError> {
        let rect = rect.into();
        if self.override_redirect && rect.is_some() {
            return Err(X11SurfaceError::UnsupportedForOverrideRedirect);
        }

        if let Some(conn) = self.conn.upgrade() {
            let mut state = self.state.lock().unwrap();
            let rect = rect.unwrap_or_else(|| Rectangle::from_loc_and_size(state.location, state.size));
            let aux = ConfigureWindowAux::default()
                .x(rect.loc.x)
                .y(rect.loc.y)
                .width(rect.size.w as u32)
                .height(rect.size.h as u32)
                .border_width(0);
            if let Some(frame) = state.mapped_onto {
                let win_aux = ConfigureWindowAux::default()
                    .width(rect.size.w as u32)
                    .height(rect.size.h as u32)
                    .border_width(0);
                conn.configure_window(frame, &aux)?;
                conn.configure_window(self.window, &win_aux)?;
            } else {
                conn.configure_window(self.window, &aux)?;
            }
            conn.flush()?;

            state.location = rect.loc;
            state.size = rect.size;
        }
        Ok(())
    }

    pub fn window_id(&self) -> X11Window {
        self.window
    }

    pub fn wl_surface(&self) -> Option<WlSurface> {
        self.state.lock().unwrap().wl_surface.clone()
    }

    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let state = self.state.lock().unwrap();
        Rectangle::from_loc_and_size(state.location, state.size)
    }

    pub fn title(&self) -> String {
        self.state.lock().unwrap().title.clone()
    }

    pub fn class(&self) -> String {
        self.state.lock().unwrap().class.clone()
    }

    pub fn instance(&self) -> String {
        self.state.lock().unwrap().class.clone()
    }

    pub fn is_popup(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.net_state.contains(&self.atoms._NET_WM_STATE_MODAL)
    }

    pub fn is_transient_for(&self) -> Option<X11Window> {
        self.state.lock().unwrap().transient_for
    }

    pub fn size_hints(&self) -> Option<WmSizeHints> {
        self.state.lock().unwrap().normal_hints
    }

    pub fn min_size(&self) -> Option<Size<i32, Logical>> {
        let state = self.state.lock().unwrap();
        state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.min_size)
            .map(Size::from)
    }

    pub fn max_size(&self) -> Option<Size<i32, Logical>> {
        let state = self.state.lock().unwrap();
        state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.max_size)
            .map(Size::from)
    }

    pub fn base_size(&self) -> Option<Size<i32, Logical>> {
        let state = self.state.lock().unwrap();
        let res = state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.base_size)
            .map(Size::from);
        std::mem::drop(state);
        res.or_else(|| self.min_size())
    }

    pub fn is_maximized(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.net_state.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_HORZ)
            && state.net_state.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_VERT)
    }

    pub fn is_fullscreen(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_FULLSCREEN)
    }

    pub fn is_minimized(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_HIDDEN)
    }

    pub fn is_activated(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_FOCUSED)
    }

    pub fn set_maximized(&self, maximized: bool) -> Result<(), ConnectionError> {
        if maximized {
            self.change_net_state(
                &[
                    self.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
                    self.atoms._NET_WM_STATE_MAXIMIZED_VERT,
                ],
                &[],
            )?;
        } else {
            self.change_net_state(
                &[],
                &[
                    self.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
                    self.atoms._NET_WM_STATE_MAXIMIZED_VERT,
                ],
            )?;
        }
        Ok(())
    }

    pub fn set_fullscreen(&self, fullscreen: bool) -> Result<(), ConnectionError> {
        if fullscreen {
            self.change_net_state(&[self.atoms._NET_WM_STATE_FULLSCREEN], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_FULLSCREEN])?;
        }
        Ok(())
    }

    pub fn set_minimized(&self, minimized: bool) -> Result<(), ConnectionError> {
        if minimized {
            self.change_net_state(&[self.atoms._NET_WM_STATE_HIDDEN], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_HIDDEN])?;
        }
        Ok(())
    }

    pub fn set_activated(&self, activated: bool) -> Result<(), ConnectionError> {
        if activated {
            self.change_net_state(&[self.atoms._NET_WM_STATE_FOCUSED], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_FOCUSED])?;
        }
        Ok(())
    }

    pub fn window_type(&self) -> Option<WmWindowType> {
        self.state
            .lock()
            .unwrap()
            .net_state
            .iter()
            .find_map(|atom| match atom {
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_DROPDOWN_MENU => Some(WmWindowType::DropdownMenu),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_DIALOG => Some(WmWindowType::Dialog),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_MENU => Some(WmWindowType::Menu),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_NOTIFICATION => Some(WmWindowType::Notification),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_NORMAL => Some(WmWindowType::Normal),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_POPUP_MENU => Some(WmWindowType::PopupMenu),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_SPLASH => Some(WmWindowType::Splash),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_TOOLBAR => Some(WmWindowType::Toolbar),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_TOOLTIP => Some(WmWindowType::Tooltip),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_UTILITY => Some(WmWindowType::Utility),
                _ => None,
            })
    }

    fn change_net_state(&self, added: &[Atom], removed: &[Atom]) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        conn.grab_server()?;

        let props = conn
            .get_property(
                false,
                self.window,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                0,
                1024,
            )?
            .reply_unchecked()?;
        let mut new_props = props
            .and_then(|props| Some(props.value32()?.collect::<HashSet<_>>()))
            .unwrap_or_default();
        new_props.retain(|p| !removed.contains(p));
        new_props.extend(added);
        let new_props = Vec::from_iter(new_props.into_iter());

        conn.change_property32(
            PropMode::REPLACE,
            self.window,
            self.atoms._NET_WM_STATE,
            AtomEnum::ATOM,
            &new_props,
        )?;
        self.update_net_state()?;
        conn.ungrab_server()?;
        conn.flush()?;
        Ok(())
    }

    fn input_mode(&self) -> InputMode {
        let state = self.state.lock().unwrap();
        match (
            state.hints.as_ref().and_then(|hints| hints.input).unwrap_or(true),
            state.protocols.contains(&WMProtocol::TakeFocus),
        ) {
            (false, false) => InputMode::None,
            (true, false) => InputMode::Passive, // the default
            (true, true) => InputMode::LocallyActive,
            (false, true) => InputMode::GloballyActive,
        }
    }

    pub(super) fn update_properties(&self, atom: Option<Atom>) -> Result<(), ConnectionError> {
        match atom {
            Some(atom) if atom == self.atoms._NET_WM_NAME || atom == AtomEnum::WM_NAME.into() => {
                self.update_title()
            }
            Some(atom) if atom == AtomEnum::WM_CLASS.into() => self.update_class(),
            Some(atom) if atom == self.atoms.WM_PROTOCOLS => self.update_protocols(),
            Some(atom) if atom == self.atoms.WM_HINTS => self.update_hints(),
            Some(atom) if atom == AtomEnum::WM_NORMAL_HINTS.into() => self.update_normal_hints(),
            Some(atom) if atom == AtomEnum::WM_TRANSIENT_FOR.into() => self.update_transient_for(),
            Some(_) => Ok(()), // unknown
            None => {
                self.update_title()?;
                self.update_class()?;
                self.update_protocols()?;
                self.update_hints()?;
                self.update_normal_hints()?;
                self.update_transient_for()?;
                self.update_net_state()?;
                Ok(())
            }
        }
    }

    fn update_class(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let (class, instance) = match WmClass::get(&*conn, self.window)?.reply_unchecked() {
            Ok(Some(wm_class)) => (
                encoding::all::ISO_8859_1
                    .decode(wm_class.class(), DecoderTrap::Replace)
                    .ok()
                    .unwrap_or_default(),
                encoding::all::ISO_8859_1
                    .decode(wm_class.instance(), DecoderTrap::Replace)
                    .ok()
                    .unwrap_or_default(),
            ),
            Ok(None) | Err(ConnectionError::ParseError(_)) => (Default::default(), Default::default()), // Getting the property failed
            Err(err) => return Err(err),
        };

        let mut state = self.state.lock().unwrap();
        state.class = class;
        state.instance = instance;

        Ok(())
    }

    fn update_hints(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let mut state = self.state.lock().unwrap();
        state.hints = match WmHints::get(&*conn, self.window)?.reply_unchecked() {
            Ok(hints) => hints,
            Err(ConnectionError::ParseError(_)) => None,
            Err(err) => return Err(err),
        };
        Ok(())
    }

    fn update_normal_hints(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let mut state = self.state.lock().unwrap();
        state.normal_hints = match WmSizeHints::get_normal_hints(&*conn, self.window)?.reply_unchecked() {
            Ok(hints) => hints,
            Err(ConnectionError::ParseError(_)) => None,
            Err(err) => return Err(err),
        };
        Ok(())
    }

    fn update_protocols(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let Some(protocols) = (match conn
            .get_property(
                false,
                self.window,
                self.atoms.WM_PROTOCOLS,
                AtomEnum::ATOM,
                0,
                2048,
            )?
            .reply_unchecked()
        {
            Ok(Some(reply)) => reply.value32().map(|vals| vals.collect::<Vec<_>>()),
            Ok(None) | Err(ConnectionError::ParseError(_)) => return Ok(()),
            Err(err) => return Err(err),
        }) else { return Ok(()) };

        let mut state = self.state.lock().unwrap();
        state.protocols = protocols
            .into_iter()
            .filter_map(|atom| match atom {
                x if x == self.atoms.WM_TAKE_FOCUS => Some(WMProtocol::TakeFocus),
                x if x == self.atoms.WM_DELETE_WINDOW => Some(WMProtocol::DeleteWindow),
                _ => None,
            })
            .collect::<Vec<_>>();
        Ok(())
    }

    fn update_transient_for(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let reply = match conn
            .get_property(
                false,
                self.window,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                0,
                2048,
            )?
            .reply_unchecked()
        {
            Ok(Some(reply)) => reply,
            Ok(None) | Err(ConnectionError::ParseError(_)) => return Ok(()),
            Err(err) => return Err(err),
        };
        let window = reply
            .value32()
            .and_then(|mut iter| iter.next())
            .filter(|w| *w != 0);

        let mut state = self.state.lock().unwrap();
        state.transient_for = window;
        Ok(())
    }

    fn update_title(&self) -> Result<(), ConnectionError> {
        let title = self
            .read_window_property_string(self.atoms._NET_WM_NAME)?
            .or(self.read_window_property_string(AtomEnum::WM_NAME)?)
            .unwrap_or_default();

        let mut state = self.state.lock().unwrap();
        state.title = title;
        Ok(())
    }

    fn update_net_state(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let atoms = match conn
            .get_property(
                false,
                self.window,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                0,
                1024,
            )?
            .reply_unchecked()
        {
            Ok(atoms) => atoms,
            Err(ConnectionError::ParseError(_)) => return Ok(()),
            Err(err) => return Err(err),
        };

        let mut state = self.state.lock().unwrap();
        state.net_state = atoms
            .and_then(|atoms| Some(atoms.value32()?.collect::<Vec<_>>()))
            .unwrap_or_default();
        Ok(())
    }

    fn read_window_property_string(&self, atom: impl Into<Atom>) -> Result<Option<String>, ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let reply = match conn
            .get_property(false, self.window, atom, AtomEnum::ANY, 0, 2048)?
            .reply_unchecked()
        {
            Ok(Some(reply)) => reply,
            Ok(None) | Err(ConnectionError::ParseError(_)) => return Ok(None),
            Err(err) => return Err(err),
        };
        let Some(bytes) = reply.value8() else { return Ok(None) };
        let bytes = bytes.collect::<Vec<u8>>();

        match reply.type_ {
            x if x == AtomEnum::STRING.into() => Ok(encoding::all::ISO_8859_1
                .decode(&bytes, DecoderTrap::Replace)
                .ok()),
            x if x == self.atoms.UTF8_STRING => Ok(String::from_utf8(bytes).ok()),
            _ => Ok(None),
        }
    }

    pub fn user_data(&self) -> &UserDataMap {
        &self.user_data
    }

    pub fn close(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let state = self.state.lock().unwrap();
        if state.protocols.contains(&WMProtocol::DeleteWindow) {
            let event = ClientMessageEvent::new(
                32,
                self.window,
                self.atoms.WM_PROTOCOLS,
                [self.atoms.WM_DELETE_WINDOW, 0, 0, 0, 0],
            );
            conn.send_event(false, self.window, EventMask::NO_EVENT, event)?;
        } else {
            conn.destroy_window(self.window)?;
        }
        conn.flush()
    }
}
pub trait X11Relatable {
    fn is_window(&self, window: &X11Surface) -> bool;
}

impl X11Relatable for X11Surface {
    fn is_window(&self, window: &X11Surface) -> bool {
        self == window
    }
}

impl X11Relatable for X11Window {
    fn is_window(&self, window: &X11Surface) -> bool {
        self == &window.window
    }
}

impl X11Relatable for WlSurface {
    fn is_window(&self, window: &X11Surface) -> bool {
        let state = window.state.lock().unwrap();
        state
            .wl_surface
            .as_ref()
            .map(|surface| surface == self)
            .unwrap_or(false)
    }
}

impl X11Relatable for Id {
    fn is_window(&self, window: &X11Surface) -> bool {
        let state = window.state.lock().unwrap();
        state
            .wl_surface
            .as_ref()
            .map(Id::from_wayland_resource)
            .map(|id| &id == self)
            .unwrap_or(false)
    }
}

impl IsAlive for X11Surface {
    fn alive(&self) -> bool {
        self.state.lock().unwrap().alive
    }
}

impl<D: SeatHandler + 'static> KeyboardTarget<D> for X11Surface {
    fn enter(&self, seat: &Seat<D>, data: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        match self.input_mode() {
            InputMode::None => return,
            InputMode::Passive => {
                if let Some(conn) = self.conn.upgrade() {
                    if let Err(err) = conn.set_input_focus(InputFocus::NONE, self.window, x11rb::CURRENT_TIME)
                    {
                        slog::warn!(
                            self.log,
                            "Unable to set focus for X11Surface ({:?}): {}",
                            self.window,
                            err
                        );
                    }
                    let _ = conn.flush();
                }
            }
            InputMode::LocallyActive => {
                if let Some(conn) = self.conn.upgrade() {
                    let event = ClientMessageEvent::new(
                        32,
                        self.window,
                        self.atoms.WM_PROTOCOLS,
                        [self.atoms.WM_TAKE_FOCUS, x11rb::CURRENT_TIME, 0, 0, 0],
                    );
                    if let Err(err) = conn.send_event(false, self.window, EventMask::NO_EVENT, event) {
                        slog::warn!(
                            self.log,
                            "Unable to send take focus event for X11Surface ({:?}): {}",
                            self.window,
                            err
                        );
                    }
                    let _ = conn.flush();
                }
            }
            _ => {}
        };
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            KeyboardTarget::enter(surface, seat, data, keys, serial);
        }
    }

    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial) {
        if self.input_mode() == InputMode::None {
            return;
        } else if let Some(conn) = self.conn.upgrade() {
            if let Err(err) = conn.set_input_focus(InputFocus::NONE, x11rb::NONE, x11rb::CURRENT_TIME) {
                slog::warn!(
                    self.log,
                    "Unable to unfocus X11Surface ({:?}): {}",
                    self.window,
                    err
                );
            }
            let _ = conn.flush();
        }

        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            KeyboardTarget::leave(surface, seat, data, serial);
        }
    }

    fn key(
        &self,
        seat: &Seat<D>,
        data: &mut D,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        if self.input_mode() == InputMode::None {
            return;
        }

        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            KeyboardTarget::key(surface, seat, data, key, state, serial, time)
        }
    }

    fn modifiers(&self, seat: &Seat<D>, data: &mut D, modifiers: ModifiersState, serial: Serial) {
        if self.input_mode() == InputMode::None {
            return;
        }

        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            KeyboardTarget::modifiers(surface, seat, data, modifiers, serial);
        }
    }
}

impl<D: SeatHandler + 'static> PointerTarget<D> for X11Surface {
    fn enter(&self, seat: &Seat<D>, data: &mut D, event: &MotionEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::enter(surface, seat, data, event);
        }
    }

    fn motion(&self, seat: &Seat<D>, data: &mut D, event: &MotionEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::motion(surface, seat, data, event);
        }
    }

    fn button(&self, seat: &Seat<D>, data: &mut D, event: &ButtonEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::button(surface, seat, data, event);
        }
    }

    fn axis(&self, seat: &Seat<D>, data: &mut D, frame: AxisFrame) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::axis(surface, seat, data, frame);
        }
    }

    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial, time: u32) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::leave(surface, seat, data, serial, time);
        }
    }
}
