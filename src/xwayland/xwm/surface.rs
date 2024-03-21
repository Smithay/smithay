use crate::{
    backend::input::KeyState,
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
        },
        touch::TouchTarget,
        Seat, SeatHandler,
    },
    utils::{user_data::UserDataMap, IsAlive, Logical, Rectangle, Serial, Size},
    wayland::compositor,
};
use encoding_rs::WINDOWS_1252;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, Weak},
};
use tracing::warn;
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

use super::{send_configure_notify, XwmId};

/// X11 window managed by an [`X11Wm`](super::X11Wm)
#[derive(Debug, Clone)]
pub struct X11Surface {
    xwm: Option<XwmId>,
    window: X11Window,
    override_redirect: bool,
    conn: Weak<RustConnection>,
    atoms: super::Atoms,
    pub(crate) state: Arc<Mutex<SharedSurfaceState>>,
    user_data: Arc<UserDataMap>,
}

const MWM_HINTS_FLAGS_FIELD: usize = 0;
const MWM_HINTS_DECORATIONS_FIELD: usize = 2;
const MWM_HINTS_DECORATIONS: u32 = 1 << 1;

#[derive(Debug)]
pub(crate) struct SharedSurfaceState {
    pub(super) alive: bool,
    pub(super) wl_surface_id: Option<u32>,
    pub(super) wl_surface_serial: Option<u64>,
    pub(super) mapped_onto: Option<X11Window>,
    pub(super) geometry: Rectangle<i32, Logical>,

    // The associated wl_surface.
    pub(crate) wl_surface: Option<WlSurface>,

    title: String,
    class: String,
    instance: String,
    startup_id: Option<String>,
    protocols: Protocols,
    hints: Option<WmHints>,
    normal_hints: Option<WmSizeHints>,
    transient_for: Option<X11Window>,
    net_state: HashSet<Atom>,
    motif_hints: Vec<u32>,
    window_type: Vec<Atom>,
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
        let self_alive = self.state.lock().unwrap().alive;
        let other_alive = other.state.lock().unwrap().alive;
        self.xwm == other.xwm && self.window == other.window && self_alive && other_alive
    }
}

/// Errors that can happen for operations on an [`X11Surface`]
#[derive(Debug, thiserror::Error)]
pub enum X11SurfaceError {
    /// Error on the underlying X11 Connection
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    /// Operation was unsupported for an override_redirect window
    #[error("Operation was unsupported for an override_redirect window")]
    UnsupportedForOverrideRedirect,
}

/// Window types of [`X11Surface`]s
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
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
    /// Create a new [`X11Surface`] usually handled by an [`X11Wm`](super::X11Wm)
    ///
    /// ## Arguments
    ///
    /// - `window` X11 window id
    /// - `override_redirect` set if the X11 window has the override redirect flag set
    /// - `conn` Weak reference on the X11 connection
    /// - `atoms` Atoms struct as defined by the [xwm module](super).
    /// - `geometry` Initial geometry of the window
    pub fn new(
        xwm: impl Into<Option<XwmId>>,
        window: u32,
        override_redirect: bool,
        conn: Weak<RustConnection>,
        atoms: super::Atoms,
        geometry: Rectangle<i32, Logical>,
    ) -> X11Surface {
        X11Surface {
            xwm: xwm.into(),
            window,
            override_redirect,
            conn,
            atoms,
            state: Arc::new(Mutex::new(SharedSurfaceState {
                alive: true,
                wl_surface_id: None,
                wl_surface_serial: None,
                wl_surface: None,
                mapped_onto: None,
                geometry,
                title: String::from(""),
                class: String::from(""),
                instance: String::from(""),
                startup_id: None,
                protocols: Vec::new(),
                hints: None,
                normal_hints: None,
                transient_for: None,
                net_state: HashSet::new(),
                motif_hints: vec![0; 5],
                window_type: Vec::new(),
            })),
            user_data: Arc::new(UserDataMap::new()),
        }
    }

    /// Returns the id of the X11Wm responsible for this surface, if any
    pub fn xwm_id(&self) -> Option<XwmId> {
        self.xwm
    }

    /// X11 protocol id of the underlying window
    pub fn window_id(&self) -> X11Window {
        self.window
    }

    /// X11 protocol id of the reparented window, if any
    pub fn mapped_window_id(&self) -> Option<X11Window> {
        self.state.lock().unwrap().mapped_onto
    }

    /// Set the X11 windows as mapped/unmapped affecting its visibility.
    ///
    /// It is an error to call this function on override redirect windows
    pub fn set_mapped(&self, mapped: bool) -> Result<(), X11SurfaceError> {
        if self.override_redirect {
            return Err(X11SurfaceError::UnsupportedForOverrideRedirect);
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

    /// Returns if this window has the override redirect flag set or not
    pub fn is_override_redirect(&self) -> bool {
        self.override_redirect
    }

    /// Returns if the window is currently mapped or not
    pub fn is_mapped(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.mapped_onto.is_some()
    }

    /// Returns if the window is still alive
    pub fn alive(&self) -> bool {
        self.state.lock().unwrap().alive && self.conn.upgrade().is_some()
    }

    /// Send a configure to this window.
    ///
    /// If `rect` is provided the new state will be send to the window.
    /// If `rect` is `None` a synthetic configure event with the existing state will be send.
    pub fn configure(&self, rect: impl Into<Option<Rectangle<i32, Logical>>>) -> Result<(), X11SurfaceError> {
        let rect = rect.into();
        if self.override_redirect && rect.is_some() {
            return Err(X11SurfaceError::UnsupportedForOverrideRedirect);
        }

        if let Some(conn) = self.conn.upgrade() {
            let mut state = self.state.lock().unwrap();
            let rect = rect.unwrap_or(state.geometry);
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
                send_configure_notify(&conn, &self.window, rect, false)?;
            } else {
                conn.configure_window(self.window, &aux)?;
            }
            conn.flush()?;

            state.geometry = rect;
        }
        Ok(())
    }

    /// Returns the associated wl_surface.
    ///
    /// This will only return `Some` once:
    ///   - The `WL_SURFACE_SERIAL` has been set on the x11 window, and
    ///   - The wl_surface has been assigned the same serial using the [xwayland
    ///     shell](crate::wayland::xwayland_shell) protocol on the wayland side,
    ///     and then committed.
    pub fn wl_surface(&self) -> Option<WlSurface> {
        self.state.lock().unwrap().wl_surface.clone()
    }

    /// Returns the associated `wl_surface` id, once it has been set by
    /// xwayland.
    ///
    /// Note that XWayland will only set this if it was unable to bind the
    /// [xwayland shell](crate::wayland::xwayland_shell) protocol on the wayland
    /// side.
    #[deprecated = "Since XWayland 23.1, the recommended approach is to use [wl_surface_serial] and the [xwayland shell](crate::wayland::xwayland_shell) protocol on the wayland side to match X11 windows."]
    pub fn wl_surface_id(&self) -> Option<u32> {
        self.state.lock().unwrap().wl_surface_id
    }

    /// Returns the associated `wl_surface` serial, once it has been set by
    /// xwayland.
    ///
    /// XWayland will set this if it has bound the [xwayland
    /// shell](crate::wayland::xwayland_shell) protocol on the wayland side.
    /// Otherwise, it will set [wl_surface_id] instead.
    pub fn wl_surface_serial(&self) -> Option<u64> {
        self.state.lock().unwrap().wl_surface_serial
    }

    /// Returns the current geometry of the underlying X11 window
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        self.state.lock().unwrap().geometry
    }

    /// Returns the current title of the underlying X11 window
    pub fn title(&self) -> String {
        self.state.lock().unwrap().title.clone()
    }

    /// Returns the current window class of the underlying X11 window
    pub fn class(&self) -> String {
        self.state.lock().unwrap().class.clone()
    }

    /// Returns the current window instance of the underlying X11 window
    pub fn instance(&self) -> String {
        self.state.lock().unwrap().instance.clone()
    }

    /// Returns the startup id of the underlying X11 window
    pub fn startup_id(&self) -> Option<String> {
        self.state.lock().unwrap().startup_id.clone()
    }

    /// Returns if the window is considered to be a popup.
    ///
    /// Corresponds to the internal `_NET_WM_STATE_MODAL` state of the underlying X11 window.
    pub fn is_popup(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.net_state.contains(&self.atoms._NET_WM_STATE_MODAL)
    }

    /// Returns if the underlying window is transient to another window.
    ///
    /// This might be used as a hint to manage windows in a group.
    pub fn is_transient_for(&self) -> Option<X11Window> {
        self.state.lock().unwrap().transient_for
    }

    /// Returns the size hints for the underlying X11 window
    pub fn size_hints(&self) -> Option<WmSizeHints> {
        self.state.lock().unwrap().normal_hints
    }

    /// Returns the suggested minimum size of the underlying X11 window
    pub fn min_size(&self) -> Option<Size<i32, Logical>> {
        let state = self.state.lock().unwrap();
        state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.min_size)
            .map(Size::from)
    }

    /// Returns the suggested minimum size of the underlying X11 window
    pub fn max_size(&self) -> Option<Size<i32, Logical>> {
        let state = self.state.lock().unwrap();
        state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.max_size)
            .map(Size::from)
    }

    /// Returns the suggested base size of the underlying X11 window
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

    /// Returns if the window is in the maximized state
    pub fn is_maximized(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.net_state.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_HORZ)
            && state.net_state.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_VERT)
    }

    /// Returns if the window is in the fullscreen state
    pub fn is_fullscreen(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_FULLSCREEN)
    }

    /// Returns if the window is in the minimized state
    pub fn is_minimized(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_HIDDEN)
    }

    /// Returns if the window is in the activated state
    pub fn is_activated(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_FOCUSED)
    }

    /// Returns true if the window is client-side decorated
    pub fn is_decorated(&self) -> bool {
        let state = self.state.lock().unwrap();
        if (state.motif_hints[MWM_HINTS_FLAGS_FIELD] & MWM_HINTS_DECORATIONS) != 0 {
            return state.motif_hints[MWM_HINTS_DECORATIONS_FIELD] == 0;
        }
        false
    }

    /// Sets the window as maximized or not.
    ///
    /// Allows the client to reflect this state in their UI.
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

    /// Sets the window as fullscreen or not.
    ///
    /// Allows the client to reflect this state in their UI.
    pub fn set_fullscreen(&self, fullscreen: bool) -> Result<(), ConnectionError> {
        if fullscreen {
            self.change_net_state(&[self.atoms._NET_WM_STATE_FULLSCREEN], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_FULLSCREEN])?;
        }
        Ok(())
    }

    /// Sets the window as minimized or not.
    ///
    /// Allows the client to e.g. stop rendering while minimized.
    pub fn set_minimized(&self, minimized: bool) -> Result<(), ConnectionError> {
        if minimized {
            self.change_net_state(&[self.atoms._NET_WM_STATE_HIDDEN], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_HIDDEN])?;
        }
        Ok(())
    }

    /// Sets the window as activated or not.
    ///
    /// Allows the client to reflect this state in their UI.
    pub fn set_activated(&self, activated: bool) -> Result<(), ConnectionError> {
        if activated {
            self.change_net_state(&[self.atoms._NET_WM_STATE_FOCUSED], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_FOCUSED])?;
        }
        Ok(())
    }

    /// Returns the reported window type of the underlying X11 window if set.
    ///
    /// Windows without a window type set should be considered to be of type `Normal` for
    /// backwards compatibility.
    pub fn window_type(&self) -> Option<WmWindowType> {
        self.state
            .lock()
            .unwrap()
            .window_type
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
        let mut state = self.state.lock().unwrap();

        let mut changed = false;
        for atom in removed {
            changed |= state.net_state.remove(atom);
        }
        for atom in added {
            changed |= state.net_state.insert(*atom);
        }

        if changed {
            let new_props = Vec::from_iter(state.net_state.iter().copied());

            let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
            conn.grab_server()?;
            let _guard = scopeguard::guard((), |_| {
                let _ = conn.ungrab_server();
                let _ = conn.flush();
            });

            conn.change_property32(
                PropMode::REPLACE,
                self.window,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                &new_props,
            )?;
        }

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
            Some(atom) if atom == self.atoms._NET_WM_WINDOW_TYPE => self.update_net_window_type(),
            Some(atom) if atom == self.atoms._MOTIF_WM_HINTS => self.update_motif_hints(),
            Some(atom) if atom == self.atoms._NET_STARTUP_ID => self.update_startup_id(),
            Some(_) => Ok(()), // unknown
            None => {
                self.update_title()?;
                self.update_class()?;
                self.update_protocols()?;
                self.update_hints()?;
                self.update_normal_hints()?;
                self.update_transient_for()?;
                // NET_WM_STATE is managed by the WM, we don't need to update it unless explicitly asked to
                self.update_net_window_type()?;
                self.update_motif_hints()?;
                self.update_startup_id()?;
                Ok(())
            }
        }
    }

    fn update_class(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let (class, instance) = match WmClass::get(&*conn, self.window)?.reply_unchecked() {
            Ok(Some(wm_class)) => (
                WINDOWS_1252.decode(wm_class.class()).0.to_string(),
                WINDOWS_1252.decode(wm_class.instance()).0.to_string(),
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

    fn update_motif_hints(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let Some(hints) = (match conn
            .get_property(
                false,
                self.window,
                self.atoms._MOTIF_WM_HINTS,
                AtomEnum::ANY,
                0,
                2048,
            )?
            .reply_unchecked()
        {
            Ok(Some(reply)) => reply.value32().map(|vals| vals.collect::<Vec<_>>()),
            Ok(None) | Err(ConnectionError::ParseError(_)) => return Ok(()),
            Err(err) => return Err(err),
        }) else {
            return Ok(());
        };

        if hints.len() < 5 {
            return Ok(());
        }

        let mut state = self.state.lock().unwrap();
        state.motif_hints = hints;
        Ok(())
    }

    fn update_startup_id(&self) -> Result<(), ConnectionError> {
        if let Some(startup_id) = self.read_window_property_string(self.atoms._NET_STARTUP_ID)? {
            let mut state = self.state.lock().unwrap();
            state.startup_id = Some(startup_id);
        }
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
        }) else {
            return Ok(());
        };

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

    fn update_net_window_type(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let atoms = match conn
            .get_property(
                false,
                self.window,
                self.atoms._NET_WM_WINDOW_TYPE,
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
        state.window_type = atoms
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
        let Some(bytes) = reply.value8() else {
            return Ok(None);
        };
        let bytes = bytes.collect::<Vec<u8>>();

        match reply.type_ {
            x if x == AtomEnum::STRING.into() => Ok(Some(WINDOWS_1252.decode(&bytes).0.to_string())),
            x if x == self.atoms.UTF8_STRING => Ok(String::from_utf8(bytes).ok()),
            _ => Ok(None),
        }
    }

    /// Retrieve user_data associated with this X11 window
    pub fn user_data(&self) -> &UserDataMap {
        &self.user_data
    }

    /// Send a close request to this window.
    ///
    /// Will outright destroy windows that don't support the `NET_DELETE_WINDOW` protocol.
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

/// Trait for objects, that represent an x11 window in some shape or form
/// and can be tested for equality.
pub trait X11Relatable {
    /// Returns if this object is considered to represent the same underlying x11 window as provided
    fn is_window(&self, window: &X11Surface) -> bool;
}

impl X11Relatable for X11Surface {
    fn is_window(&self, window: &X11Surface) -> bool {
        self == window
    }
}

impl X11Relatable for WlSurface {
    fn is_window(&self, window: &X11Surface) -> bool {
        let serial = compositor::with_states(self, |states| {
            states
                .cached_state
                .current::<crate::wayland::xwayland_shell::XWaylandShellCachedState>()
                .serial
        });

        window.wl_surface_serial() == serial
    }
}

impl IsAlive for X11Surface {
    fn alive(&self) -> bool {
        self.state.lock().unwrap().alive
    }
}

impl<D: SeatHandler + 'static> KeyboardTarget<D> for X11Surface {
    fn enter(&self, seat: &Seat<D>, data: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        let (set_input_focus, send_take_focus) = match self.input_mode() {
            InputMode::None => return,
            InputMode::Passive => (true, false),
            InputMode::LocallyActive => (true, true),
            InputMode::GloballyActive => (false, true),
        };

        if let Some(conn) = self.conn.upgrade() {
            if set_input_focus {
                if let Err(err) = conn.set_input_focus(InputFocus::NONE, self.window, x11rb::CURRENT_TIME) {
                    warn!("Unable to set focus for X11Surface ({:?}): {}", self.window, err);
                }
            }

            if send_take_focus {
                let event = ClientMessageEvent::new(
                    32,
                    self.window,
                    self.atoms.WM_PROTOCOLS,
                    [self.atoms.WM_TAKE_FOCUS, x11rb::CURRENT_TIME, 0, 0, 0],
                );
                if let Err(err) = conn.send_event(false, self.window, EventMask::NO_EVENT, event) {
                    warn!(
                        "Unable to send take focus event for X11Surface ({:?}): {}",
                        self.window, err
                    );
                }
                let _ = conn.flush();
            }

            let _ = conn.flush();
        }

        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            KeyboardTarget::enter(surface, seat, data, keys, serial);
        }
    }

    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial) {
        if self.input_mode() == InputMode::None {
            return;
        } else if let Some(conn) = self.conn.upgrade() {
            if let Err(err) = conn.set_input_focus(InputFocus::NONE, x11rb::NONE, x11rb::CURRENT_TIME) {
                warn!("Unable to unfocus X11Surface ({:?}): {}", self.window, err);
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

    fn relative_motion(&self, seat: &Seat<D>, data: &mut D, event: &RelativeMotionEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::relative_motion(surface, seat, data, event);
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

    fn frame(&self, seat: &Seat<D>, data: &mut D) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::frame(surface, seat, data);
        }
    }

    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial, time: u32) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::leave(surface, seat, data, serial, time);
        }
    }

    fn gesture_swipe_begin(&self, seat: &Seat<D>, data: &mut D, event: &GestureSwipeBeginEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_swipe_begin(surface, seat, data, event);
        }
    }

    fn gesture_swipe_update(&self, seat: &Seat<D>, data: &mut D, event: &GestureSwipeUpdateEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_swipe_update(surface, seat, data, event);
        }
    }

    fn gesture_swipe_end(&self, seat: &Seat<D>, data: &mut D, event: &GestureSwipeEndEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_swipe_end(surface, seat, data, event);
        }
    }

    fn gesture_pinch_begin(&self, seat: &Seat<D>, data: &mut D, event: &GesturePinchBeginEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_pinch_begin(surface, seat, data, event)
        }
    }

    fn gesture_pinch_update(&self, seat: &Seat<D>, data: &mut D, event: &GesturePinchUpdateEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_pinch_update(surface, seat, data, event)
        }
    }

    fn gesture_pinch_end(&self, seat: &Seat<D>, data: &mut D, event: &GesturePinchEndEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_pinch_end(surface, seat, data, event)
        }
    }

    fn gesture_hold_begin(&self, seat: &Seat<D>, data: &mut D, event: &GestureHoldBeginEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_hold_begin(surface, seat, data, event)
        }
    }

    fn gesture_hold_end(&self, seat: &Seat<D>, data: &mut D, event: &GestureHoldEndEvent) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            PointerTarget::gesture_hold_end(surface, seat, data, event)
        }
    }
}

impl<D: SeatHandler + 'static> TouchTarget<D> for X11Surface {
    fn down(&self, seat: &Seat<D>, data: &mut D, event: &crate::input::touch::DownEvent, seq: Serial) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            TouchTarget::down(surface, seat, data, event, seq)
        }
    }

    fn up(&self, seat: &Seat<D>, data: &mut D, event: &crate::input::touch::UpEvent, seq: Serial) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            TouchTarget::up(surface, seat, data, event, seq)
        }
    }

    fn motion(&self, seat: &Seat<D>, data: &mut D, event: &crate::input::touch::MotionEvent, seq: Serial) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            TouchTarget::motion(surface, seat, data, event, seq)
        }
    }

    fn frame(&self, seat: &Seat<D>, data: &mut D, seq: Serial) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            TouchTarget::frame(surface, seat, data, seq)
        }
    }

    fn cancel(&self, seat: &Seat<D>, data: &mut D, seq: Serial) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            TouchTarget::cancel(surface, seat, data, seq)
        }
    }

    fn shape(&self, seat: &Seat<D>, data: &mut D, event: &crate::input::touch::ShapeEvent, seq: Serial) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            TouchTarget::shape(surface, seat, data, event, seq)
        }
    }

    fn orientation(
        &self,
        seat: &Seat<D>,
        data: &mut D,
        event: &crate::input::touch::OrientationEvent,
        seq: Serial,
    ) {
        if let Some(surface) = self.state.lock().unwrap().wl_surface.as_ref() {
            TouchTarget::orientation(surface, seat, data, event, seq)
        }
    }
}
