#![allow(missing_docs)]

use crate::{
    backend::{input::KeyState, renderer::element::Id},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget},
        Seat, SeatHandler,
    },
    utils::{x11rb::X11Source, IsAlive, Logical, Point, Rectangle, Serial, Size},
    wayland::{
        compositor::{get_role, give_role},
        seat::WaylandFocus,
    },
};
use calloop::channel::SyncSender;
use encoding::{DecoderTrap, Encoding};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    convert::TryFrom,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, Weak},
};
use wayland_server::{protocol::wl_surface::WlSurface, Client, DisplayHandle, Resource};

use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    properties::{WmClass, WmHints, WmSizeHints},
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent, ConfigWindow,
            ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, PropMode, Screen,
            StackMode, Window as X11Window, WindowClass,
        },
        Event,
    },
    rust_connection::{ConnectionError, DefaultStream, RustConnection},
    wrapper::ConnectionExt,
    COPY_DEPTH_FROM_PARENT,
};

use super::xserver::XWaylandClientData;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        // wayland-stuff
        WL_SURFACE_ID,

        // private
        _LATE_SURFACE_ID,
        _SMITHAY_CLOSE_CONNECTION,

        // data formats
        UTF8_STRING,

        // client -> server
        WM_HINTS,
        WM_PROTOCOLS,
        WM_TAKE_FOCUS,
        WM_DELETE_WINDOW,
        WM_CHANGE_STATE,
        _NET_WM_NAME,
        _NET_WM_MOVERESIZE,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DROPDOWN_MENU,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_MENU,
        _NET_WM_WINDOW_TYPE_NOTIFICATION,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WM_WINDOW_TYPE_SPLASH,
        _NET_WM_WINDOW_TYPE_TOOLBAR,
        _NET_WM_WINDOW_TYPE_TOOLTIP,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_STATE_MODAL,
        _MOTIF_WM_HINTS,

        // server -> client
        WM_S0,
        WM_STATE,
        _NET_WM_CM_S0,
        _NET_SUPPORTED,
        _NET_ACTIVE_WINDOW,
        _NET_CLIENT_LIST,
        _NET_CLIENT_LIST_STACKING,
        _NET_WM_PING,
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_FOCUSED,
        _NET_SUPPORTING_WM_CHECK,
    }
}

#[derive(Debug, Clone)]
pub struct X11Surface {
    window: X11Window,
    override_redirect: bool,
    conn: Weak<RustConnection>,
    atoms: Atoms,
    state: Arc<Mutex<SharedSurfaceState>>,
    log: slog::Logger,
}

#[derive(Debug)]
struct SharedSurfaceState {
    alive: bool,
    wl_surface: Option<WlSurface>,
    mapped_onto: Option<X11Window>,

    location: Point<i32, Logical>,
    size: Size<i32, Logical>,

    title: String,
    class: String,
    instance: String,
    protocols: Protocols,
    hints: Option<WmHints>,
    normal_hints: Option<WmSizeHints>,
    transient_for: Option<X11Window>,
    net_state: Vec<Atom>,
}

type Protocols = Vec<WMProtocol>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WMProtocol {
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
            if let Some(frame) = self.state.lock().unwrap().mapped_onto {
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

    fn update_properties(&self, atom: Option<Atom>) -> Result<(), ConnectionError> {
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
        let (class, instance) = match WmClass::get(&*conn, self.window)?.reply_unchecked()? {
            Some(wm_class) => (
                encoding::all::ISO_8859_1
                    .decode(wm_class.class(), DecoderTrap::Replace)
                    .ok()
                    .unwrap_or_default(),
                encoding::all::ISO_8859_1
                    .decode(wm_class.instance(), DecoderTrap::Replace)
                    .ok()
                    .unwrap_or_default(),
            ),
            None => (Default::default(), Default::default()), // Getting the property failed
        };

        let mut state = self.state.lock().unwrap();
        state.class = class;
        state.instance = instance;

        Ok(())
    }

    fn update_hints(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let mut state = self.state.lock().unwrap();
        state.hints = WmHints::get(&*conn, self.window)?.reply_unchecked()?;
        Ok(())
    }

    fn update_normal_hints(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let mut state = self.state.lock().unwrap();
        state.normal_hints = WmSizeHints::get_normal_hints(&*conn, self.window)?.reply_unchecked()?;
        Ok(())
    }

    fn update_protocols(&self) -> Result<(), ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let Some(reply) = conn.get_property(false, self.window, self.atoms.WM_PROTOCOLS, AtomEnum::ATOM, 0, 2048)?.reply_unchecked()? else { return Ok(()) };
        let Some(protocols) = reply.value32() else { return Ok(()) };

        let mut state = self.state.lock().unwrap();
        state.protocols = protocols
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
        let Some(reply) = conn.get_property(false, self.window, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, 0, 2048)?.reply_unchecked()? else { return Ok(()) };
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
        let atoms = conn
            .get_property(
                false,
                self.window,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                0,
                1024,
            )?
            .reply_unchecked()?;

        let mut state = self.state.lock().unwrap();
        state.net_state = atoms
            .and_then(|atoms| Some(atoms.value32()?.collect::<Vec<_>>()))
            .unwrap_or_default();
        Ok(())
    }

    fn read_window_property_string(&self, atom: impl Into<Atom>) -> Result<Option<String>, ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let Some(reply) = conn.get_property(false, self.window, atom, AtomEnum::ANY, 0, 2048)?.reply_unchecked()? else { return Ok(None) };
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

#[derive(Debug, Clone)]
pub enum XwmEvent {
    NewWindowNotify {
        window: X11Surface,
    },
    NewORWindowNotify {
        window: X11Surface,
    },
    MapWindowRequest {
        window: X11Surface,
    },
    MapORWindowNotify {
        window: X11Surface,
    },
    UnmappedWindowNotify {
        window: X11Surface,
    },
    DestroyedWindowNotify {
        window: X11Surface,
    },
    ConfigureRequest {
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
    },
    ConfigureNotify {
        window: X11Surface,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    MaximizeRequest {
        window: X11Surface,
    },
    UnmaximizeRequet {
        window: X11Surface,
    },
    FullscreenRequest {
        window: X11Surface,
    },
    UnfullscreenRequest {
        window: X11Surface,
    },
    MinimizeRequest {
        window: X11Surface,
    },
    ResizeRequest {
        window: X11Surface,
        button: u32,
        resize_edge: ResizeEdge,
    },
    MoveRequest {
        window: X11Surface,
    },
}

/// The runtime state of the XWayland window manager.
#[derive(Debug)]
pub struct X11WM {
    conn: Arc<RustConnection>,
    dh: DisplayHandle,
    screen: Screen,
    atoms: Atoms,

    wl_client: Client,
    unpaired_surfaces: HashMap<u32, X11Window>,
    sequences_to_ignore: BinaryHeap<Reverse<u16>>,

    windows: Vec<X11Surface>,
    // oldest mapped -> newest
    client_list: Vec<X11Window>,
    // bottom -> top
    client_list_stacking: Vec<X11Window>,
    log: slog::Logger,
}

struct X11Injector {
    atom: Atom,
    sender: SyncSender<Event>,
}
impl X11Injector {
    pub fn late_window(&self, surface: &WlSurface) {
        let _ = self.sender.send(Event::ClientMessage(ClientMessageEvent {
            response_type: 0,
            format: 0,
            sequence: 0,
            window: 0,
            type_: self.atom,
            data: ClientMessageData::from([surface.id().protocol_id(), 0, 0, 0, 0]),
        }));
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    TopLeft,
    BottomLeft,
    Right,
    TopRight,
    BottomRight,
}

impl X11WM {
    pub fn start_wm<L>(
        dh: DisplayHandle,
        connection: UnixStream,
        client: Client,
        log: L,
    ) -> Result<(Self, X11Source), Box<dyn std::error::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        // Create an X11 connection. XWayland only uses screen 0.
        let screen = 0;
        let stream = DefaultStream::from_unix_stream(connection)?;
        let conn = RustConnection::connect_to_stream(stream, screen)?;
        let atoms = Atoms::new(&conn)?.reply()?;

        let screen = conn.setup().roots[0].clone();

        // Actually become the WM by redirecting some operations
        conn.change_window_attributes(
            screen.root,
            &ChangeWindowAttributesAux::default().event_mask(
                EventMask::SUBSTRUCTURE_REDIRECT
                    | EventMask::SUBSTRUCTURE_NOTIFY
                    | EventMask::PROPERTY_CHANGE
                    | EventMask::FOCUS_CHANGE,
            ),
        )?;

        // Tell XWayland that we are the WM by acquiring the WM_S0 selection. No X11 clients are accepted before this.
        let win = conn.generate_id()?;
        conn.create_window(
            screen.root_depth,
            win,
            screen.root,
            // x, y, width, height, border width
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &Default::default(),
        )?;
        conn.set_selection_owner(win, atoms.WM_S0, x11rb::CURRENT_TIME)?;
        conn.set_selection_owner(win, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)?;
        conn.composite_redirect_subwindows(screen.root, Redirect::MANUAL)?;

        // Set some EWMH properties
        conn.change_property32(
            PropMode::REPLACE,
            screen.root,
            atoms._NET_SUPPORTED,
            AtomEnum::ATOM,
            &[
                atoms._NET_WM_STATE,
                atoms._NET_WM_STATE_MAXIMIZED_HORZ,
                atoms._NET_WM_STATE_MAXIMIZED_VERT,
                atoms._NET_WM_STATE_HIDDEN,
                atoms._NET_WM_STATE_FULLSCREEN,
                atoms._NET_WM_STATE_MODAL,
                atoms._NET_WM_STATE_FOCUSED,
                atoms._NET_ACTIVE_WINDOW,
                atoms._NET_WM_MOVERESIZE,
                atoms._NET_CLIENT_LIST,
                atoms._NET_CLIENT_LIST_STACKING,
            ],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            screen.root,
            atoms._NET_CLIENT_LIST,
            AtomEnum::WINDOW,
            &[],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            screen.root,
            atoms._NET_CLIENT_LIST_STACKING,
            AtomEnum::WINDOW,
            &[],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            screen.root,
            atoms._NET_ACTIVE_WINDOW,
            AtomEnum::WINDOW,
            &[0],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            screen.root,
            atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[win],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            win,
            atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[win],
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            win,
            atoms._NET_WM_NAME,
            atoms.UTF8_STRING,
            "Smithay X WM".as_bytes(),
        )?;

        conn.flush()?;

        let log = crate::slog_or_fallback(log);
        let conn = Arc::new(conn);
        let source = X11Source::new(
            Arc::clone(&conn),
            win,
            atoms._SMITHAY_CLOSE_CONNECTION,
            log.clone(),
        );
        let injector = X11Injector {
            atom: atoms._LATE_SURFACE_ID,
            sender: source.sender.clone(),
        };
        client
            .get_data::<XWaylandClientData>()
            .unwrap()
            .data_map
            .insert_if_missing(move || injector);

        let wm = Self {
            dh,
            conn,
            screen,
            atoms,
            wl_client: client,
            unpaired_surfaces: Default::default(),
            sequences_to_ignore: Default::default(),
            windows: Vec::new(),
            client_list: Vec::new(),
            client_list_stacking: Vec::new(),
            log,
        };
        Ok((wm, source))
    }

    pub fn handle_event<Impl>(&mut self, event: Event, callback: Impl) -> Result<(), ReplyOrIdError>
    where
        Impl: FnOnce(XwmEvent),
    {
        let mut should_ignore = false;
        if let Some(seqno) = event.wire_sequence_number() {
            // Check sequences_to_ignore and remove entries with old (=smaller) numbers.
            while let Some(&Reverse(to_ignore)) = self.sequences_to_ignore.peek() {
                // Sequence numbers can wrap around, so we cannot simply check for
                // "to_ignore <= seqno". This is equivalent to "to_ignore - seqno <= 0", which is what we
                // check instead. Since sequence numbers are unsigned, we need a trick: We decide
                // that values from [MAX/2, MAX] count as "<= 0" and the rest doesn't.
                if to_ignore.wrapping_sub(seqno) <= u16::max_value() / 2 {
                    // If the two sequence numbers are equal, this event should be ignored.
                    should_ignore = to_ignore == seqno;
                    break;
                }
                self.sequences_to_ignore.pop();
            }
        }

        slog::debug!(
            self.log,
            "X11: Got event {:?}{}",
            event,
            if should_ignore { " [ignored]" } else { "" }
        );
        if should_ignore {
            return Ok(());
        }

        match event {
            Event::CreateNotify(n) => {
                if self
                    .windows
                    .iter()
                    .any(|s| s.state.lock().unwrap().mapped_onto == Some(n.window))
                {
                    return Ok(());
                }

                let geo = self.conn.get_geometry(n.window)?.reply()?;

                let surface = X11Surface {
                    window: n.window,
                    override_redirect: n.override_redirect,
                    conn: Arc::downgrade(&self.conn),
                    atoms: self.atoms,
                    state: Arc::new(Mutex::new(SharedSurfaceState {
                        alive: true,
                        wl_surface: None,
                        mapped_onto: None,
                        location: (geo.x as i32, geo.y as i32).into(),
                        size: (geo.width as i32, geo.height as i32).into(),
                        title: String::from(""),
                        class: String::from(""),
                        instance: String::from(""),
                        protocols: Vec::new(),
                        hints: None,
                        normal_hints: None,
                        transient_for: None,
                        net_state: Vec::new(),
                    })),
                    log: self.log.new(slog::o!("X11 Window" => n.window)),
                };
                surface.update_properties(None)?;
                self.windows.push(surface.clone());

                if n.override_redirect {
                    callback(XwmEvent::NewORWindowNotify { window: surface })
                } else {
                    callback(XwmEvent::NewWindowNotify { window: surface });
                }
            }
            Event::MapRequest(r) => {
                if let Some(surface) = self.windows.iter().find(|x| x.window == r.window) {
                    self.client_list.push(surface.window);

                    // we reparent windows, because a lot of stuff expects, that we do
                    let geo = self.conn.get_geometry(r.window)?.reply()?;
                    let win = r.window;
                    let frame_win = self.conn.generate_id()?;
                    let win_aux = CreateWindowAux::new()
                        .event_mask(EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT);

                    self.conn.grab_server()?;
                    let cookie1 = self.conn.create_window(
                        COPY_DEPTH_FROM_PARENT,
                        frame_win,
                        self.screen.root,
                        geo.x,
                        geo.y,
                        geo.width,
                        geo.height,
                        0,
                        WindowClass::INPUT_OUTPUT,
                        x11rb::COPY_FROM_PARENT,
                        &win_aux,
                    )?;
                    let cookie2 = self.conn.reparent_window(win, frame_win, 0, 0)?;
                    self.conn.map_window(win)?;
                    self.conn.change_property32(
                        PropMode::APPEND,
                        self.screen.root,
                        self.atoms._NET_CLIENT_LIST,
                        AtomEnum::WINDOW,
                        &[win],
                    )?;
                    self.conn.ungrab_server()?;

                    // Ignore all events caused by reparent_window(). All those events have the sequence number
                    // of the reparent_window() request, thus remember its sequence number. The
                    // grab_server()/ungrab_server() is done so that the server does not handle other clients
                    // in-between, which could cause other events to get the same sequence number.
                    self.sequences_to_ignore
                        .push(Reverse(cookie1.sequence_number() as u16));
                    self.sequences_to_ignore
                        .push(Reverse(cookie2.sequence_number() as u16));

                    surface.state.lock().unwrap().mapped_onto = Some(frame_win);
                    callback(XwmEvent::MapWindowRequest {
                        window: surface.clone(),
                    });
                }
            }
            Event::MapNotify(n) => {
                slog::trace!(self.log, "X11 Window mapped: {}", n.window);
                if let Some(surface) = self.windows.iter().find(|x| x.window == n.window) {
                    if surface.override_redirect {
                        callback(XwmEvent::MapORWindowNotify {
                            window: surface.clone(),
                        })
                    }
                    self.client_list_stacking.push(surface.window);
                    self.conn.change_property32(
                        PropMode::APPEND,
                        self.screen.root,
                        self.atoms._NET_CLIENT_LIST_STACKING,
                        AtomEnum::WINDOW,
                        &[surface.window],
                    )?;
                } else if let Some(surface) = self
                    .windows
                    .iter()
                    .find(|x| x.state.lock().unwrap().mapped_onto.unwrap() == n.window)
                {
                    self.client_list_stacking.push(surface.window);
                    self.conn.change_property32(
                        PropMode::APPEND,
                        self.screen.root,
                        self.atoms._NET_CLIENT_LIST_STACKING,
                        AtomEnum::WINDOW,
                        &[surface.window],
                    )?;
                }
            }
            Event::ConfigureRequest(r) => {
                if let Some(surface) = self.windows.iter().find(|x| x.window == r.window) {
                    // Pass the request to downstream to decide
                    callback(XwmEvent::ConfigureRequest {
                        window: surface.clone(),
                        x: if r.value_mask & u16::from(ConfigWindow::X) != 0 {
                            Some(i32::try_from(r.x).unwrap())
                        } else {
                            None
                        },
                        y: if r.value_mask & u16::from(ConfigWindow::Y) != 0 {
                            Some(i32::try_from(r.y).unwrap())
                        } else {
                            None
                        },
                        width: if r.value_mask & u16::from(ConfigWindow::WIDTH) != 0 {
                            Some(u32::try_from(r.width).unwrap())
                        } else {
                            None
                        },
                        height: if r.value_mask & u16::from(ConfigWindow::HEIGHT) != 0 {
                            Some(u32::try_from(r.height).unwrap())
                        } else {
                            None
                        },
                    });
                    // Synthetic event
                    surface.configure(None).map_err(|err| match err {
                        X11SurfaceError::Connection(err) => err,
                        X11SurfaceError::UnsupportedForOverrideRedirect => unreachable!(),
                    })?;
                }
            }
            Event::ConfigureNotify(n) => {
                slog::trace!(self.log, "X11 Window configured: {:?}", n);
                if let Some(surface) = self
                    .windows
                    .iter()
                    .find(|x| x.state.lock().unwrap().mapped_onto == Some(n.window))
                {
                    callback(XwmEvent::ConfigureNotify {
                        window: surface.clone(),
                        x: n.x as i32,
                        y: n.y as i32,
                        width: n.width as u32,
                        height: n.height as u32,
                    });
                } else if let Some(surface) = self.windows.iter().find(|x| x.window == n.window) {
                    if surface.override_redirect {
                        callback(XwmEvent::ConfigureNotify {
                            window: surface.clone(),
                            x: n.x as i32,
                            y: n.y as i32,
                            width: n.width as u32,
                            height: n.height as u32,
                        });
                    }
                }
            }
            Event::UnmapNotify(n) => {
                if let Some(surface) = self.windows.iter().find(|x| x.window == n.window) {
                    self.client_list.retain(|w| *w != surface.window);
                    self.client_list_stacking.retain(|w| *w != surface.window);
                    self.conn.grab_server()?;
                    self.conn.change_property32(
                        PropMode::REPLACE,
                        self.screen.root,
                        self.atoms._NET_CLIENT_LIST,
                        AtomEnum::WINDOW,
                        &self.client_list,
                    )?;
                    self.conn.change_property32(
                        PropMode::REPLACE,
                        self.screen.root,
                        self.atoms._NET_CLIENT_LIST_STACKING,
                        AtomEnum::WINDOW,
                        &self.client_list_stacking,
                    )?;
                    {
                        let mut state = surface.state.lock().unwrap();
                        self.conn.reparent_window(
                            n.window,
                            self.screen.root,
                            state.location.x as i16,
                            state.location.y as i16,
                        )?;
                        if let Some(frame) = state.mapped_onto.take() {
                            self.conn.destroy_window(frame)?;
                        }
                    }
                    self.conn.ungrab_server()?;
                    callback(XwmEvent::UnmappedWindowNotify {
                        window: surface.clone(),
                    });
                    {
                        let mut state = surface.state.lock().unwrap();
                        state.wl_surface = None;
                    }
                }
            }
            Event::DestroyNotify(n) => {
                if let Some(pos) = self.windows.iter().position(|x| x.window == n.window) {
                    let surface = self.windows.remove(pos);
                    surface.state.lock().unwrap().alive = false;
                    callback(XwmEvent::DestroyedWindowNotify { window: surface });
                }
            }
            Event::PropertyNotify(n) => {
                if let Some(surface) = self.windows.iter().find(|x| x.window == n.window) {
                    surface.update_properties(Some(n.atom))?;
                }
            }
            Event::FocusIn(n) => {
                self.conn.change_property32(
                    PropMode::REPLACE,
                    self.screen.root,
                    self.atoms._NET_ACTIVE_WINDOW,
                    AtomEnum::WINDOW,
                    &[n.event],
                )?;
            }
            Event::FocusOut(n) => {
                self.conn.change_property32(
                    PropMode::REPLACE,
                    self.screen.root,
                    self.atoms._NET_ACTIVE_WINDOW,
                    AtomEnum::WINDOW,
                    &[n.event],
                )?;
            }
            Event::ClientMessage(msg) => {
                match msg.type_ {
                    x if x == self.atoms.WL_SURFACE_ID => {
                        let id = msg.data.as_data32()[0];
                        slog::info!(
                            self.log,
                            "X11 surface {:?} corresponds to WlSurface {:?}",
                            msg.window,
                            id,
                        );
                        if let Some(surface) = self
                            .windows
                            .iter_mut()
                            .find(|x| x.state.lock().unwrap().mapped_onto == Some(msg.window))
                        {
                            // We get a WL_SURFACE_ID message when Xwayland creates a WlSurface for a
                            // window. Both the creation of the surface and this client message happen at
                            // roughly the same time and are sent over different sockets (X11 socket and
                            // wayland socket). Thus, we could receive these two in any order. Hence, it
                            // can happen that we get None below when X11 was faster than Wayland.

                            let wl_surface =
                                self.wl_client.object_from_protocol_id::<WlSurface>(&self.dh, id);
                            match wl_surface {
                                Err(_) => {
                                    self.unpaired_surfaces.insert(id, msg.window);
                                }
                                Ok(wl_surface) => {
                                    Self::new_surface(surface, wl_surface, self.log.clone());
                                }
                            }
                        }
                    }
                    x if x == self.atoms._LATE_SURFACE_ID => {
                        let id = msg.data.as_data32()[0];
                        if let Some(window) = dbg!(&mut self.unpaired_surfaces).remove(&id) {
                            if let Some(surface) = self
                                .windows
                                .iter_mut()
                                .find(|x| x.state.lock().unwrap().mapped_onto == Some(window))
                            {
                                let wl_surface = self
                                    .wl_client
                                    .object_from_protocol_id::<WlSurface>(&self.dh, id)
                                    .unwrap();
                                Self::new_surface(surface, wl_surface, self.log.clone());
                            }
                        }
                    }
                    x if x == self.atoms.WM_CHANGE_STATE => {
                        if let Some(surface) = self.windows.iter().find(|x| x.window == msg.window) {
                            callback(XwmEvent::MinimizeRequest {
                                window: surface.clone(),
                            });
                        }
                    }
                    x if x == self.atoms._NET_WM_STATE => {
                        if let Some(surface) = self.windows.iter().find(|x| x.window == msg.window) {
                            let data = msg.data.as_data32();
                            match &data[1..=2] {
                                &[x, y]
                                    if (x == self.atoms._NET_WM_STATE_MAXIMIZED_HORZ
                                        && y == self.atoms._NET_WM_STATE_MAXIMIZED_VERT)
                                        || (x == self.atoms._NET_WM_STATE_MAXIMIZED_VERT
                                            && y == self.atoms._NET_WM_STATE_MAXIMIZED_HORZ) =>
                                {
                                    match data[0] {
                                        0 => callback(XwmEvent::UnmaximizeRequet {
                                            window: surface.clone(),
                                        }),
                                        1 => callback(XwmEvent::MaximizeRequest {
                                            window: surface.clone(),
                                        }),
                                        2 => {
                                            if surface.is_maximized() {
                                                callback(XwmEvent::UnmaximizeRequet {
                                                    window: surface.clone(),
                                                })
                                            } else {
                                                callback(XwmEvent::MaximizeRequest {
                                                    window: surface.clone(),
                                                })
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                actions if actions.contains(&self.atoms._NET_WM_STATE_FULLSCREEN) => {
                                    match data[0] {
                                        0 => callback(XwmEvent::UnfullscreenRequest {
                                            window: surface.clone(),
                                        }),
                                        1 => callback(XwmEvent::FullscreenRequest {
                                            window: surface.clone(),
                                        }),
                                        2 => {
                                            if surface.is_fullscreen() {
                                                callback(XwmEvent::UnfullscreenRequest {
                                                    window: surface.clone(),
                                                })
                                            } else {
                                                callback(XwmEvent::FullscreenRequest {
                                                    window: surface.clone(),
                                                })
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    x if x == self.atoms._NET_WM_MOVERESIZE => {
                        if let Some(surface) = self.windows.iter().find(|x| x.window == msg.window) {
                            let data = msg.data.as_data32();
                            match data[2] {
                                x @ 0..=7 => {
                                    let resize_edge = match x {
                                        0 => ResizeEdge::TopLeft,
                                        1 => ResizeEdge::Top,
                                        2 => ResizeEdge::TopRight,
                                        3 => ResizeEdge::Right,
                                        4 => ResizeEdge::BottomRight,
                                        5 => ResizeEdge::Bottom,
                                        6 => ResizeEdge::BottomLeft,
                                        7 => ResizeEdge::Left,
                                        _ => unreachable!(),
                                    };
                                    callback(XwmEvent::ResizeRequest {
                                        window: surface.clone(),
                                        button: data[3],
                                        resize_edge,
                                    });
                                }
                                8 => callback(XwmEvent::MoveRequest {
                                    window: surface.clone(),
                                }),
                                _ => {} // ignore keyboard moves/resizes for now
                            }
                        }
                    }
                    x => {
                        slog::debug!(
                            self.log,
                            "Unhandled client msg of type {:?}",
                            String::from_utf8(self.conn.get_atom_name(x)?.reply_unchecked()?.unwrap().name)
                                .ok()
                        )
                    }
                }
            }
            _ => {}
        }
        self.conn.flush()?;
        Ok(())
    }

    pub fn update_stacking_order_downwards<'a, W: X11Relatable + 'a>(
        &mut self,
        order: impl Iterator<Item = &'a W>,
    ) -> Result<(), ConnectionError> {
        let mut last_pos = None;
        for relatable in order {
            let pos = self
                .client_list_stacking
                .iter()
                .map(|w| self.windows.iter().find(|s| s.window == *w).unwrap())
                .position(|w| relatable.is_window(w));
            if let (Some(pos), Some(last_pos)) = (pos, last_pos) {
                if last_pos < pos {
                    // move pos before last_pos
                    let sibling = self.client_list_stacking[last_pos];
                    let elem = self.client_list_stacking.remove(pos);
                    self.conn.configure_window(
                        elem,
                        &ConfigureWindowAux::new()
                            .sibling(sibling)
                            .stack_mode(StackMode::BELOW),
                    )?;
                    self.client_list_stacking.insert(last_pos, elem);
                    continue;
                }
            }
            if pos.is_some() {
                last_pos = pos;
            }
        }
        Ok(())
    }

    pub fn update_stacking_order_upwards<'a, W: X11Relatable + 'a>(
        &mut self,
        order: impl Iterator<Item = &'a W>,
    ) -> Result<(), ConnectionError> {
        let mut last_pos = None;
        for relatable in order {
            let pos = self
                .client_list_stacking
                .iter()
                .map(|w| self.windows.iter().find(|s| s.window == *w).unwrap())
                .position(|w| relatable.is_window(w));
            if let (Some(pos), Some(last_pos)) = (pos, last_pos) {
                if last_pos > pos {
                    // move pos after last_pos
                    let sibling = self.client_list_stacking[last_pos];
                    let elem = self.client_list_stacking.remove(pos);
                    self.conn.configure_window(
                        elem,
                        &ConfigureWindowAux::new()
                            .sibling(sibling)
                            .stack_mode(StackMode::ABOVE),
                    )?;
                    self.client_list_stacking.insert(last_pos, elem);
                    continue;
                }
            }
            if pos.is_some() {
                last_pos = pos;
            }
        }
        Ok(())
    }

    pub fn commit_hook(surface: &WlSurface) {
        if let Some(client) = surface.client() {
            if let Some(x11) = client
                .get_data::<XWaylandClientData>()
                .and_then(|data| data.data_map.get::<X11Injector>())
            {
                if get_role(surface).is_none() {
                    x11.late_window(surface);
                }
            }
        }
    }

    fn new_surface(surface: &mut X11Surface, wl_surface: WlSurface, log: ::slog::Logger) {
        slog::info!(
            log,
            "Matched X11 surface {:?} to {:x?}",
            surface.window,
            wl_surface
        );
        if give_role(&wl_surface, "x11_surface").is_err() {
            // It makes no sense to post a protocol error here since that would only kill Xwayland
            slog::error!(log, "Surface {:x?} already has a role?!", wl_surface);
            return;
        }

        surface.state.lock().unwrap().wl_surface = Some(wl_surface);
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
        // _NET_WINDOW_STATE_FOCUSED
        match self.input_mode() {
            InputMode::None => return,
            InputMode::Passive => {
                if let Some(conn) = self.conn.upgrade() {
                    conn.set_input_focus(InputFocus::NONE, self.window, x11rb::CURRENT_TIME);
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
                    conn.send_event(false, self.window, EventMask::NO_EVENT, &event);
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
            conn.set_input_focus(InputFocus::NONE, x11rb::NONE, x11rb::CURRENT_TIME);
        }

        // _NET_WINDOW_STATE_UNFOCUSED
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

impl WaylandFocus for X11Surface {
    fn wl_surface(&self) -> Option<WlSurface> {
        self.state.lock().unwrap().wl_surface.clone()
    }
}
