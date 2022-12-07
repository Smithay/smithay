#![allow(missing_docs)]

use crate::{
    utils::{x11rb::X11Source, Logical, Point, Rectangle, Size},
    wayland::compositor::{get_role, give_role},
};
use calloop::channel::SyncSender;
use encoding::{DecoderTrap, Encoding};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    convert::TryFrom,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, Weak},
};
use wayland_server::{protocol::wl_surface::WlSurface, Client, DisplayHandle, Resource};

use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    properties::WmClass,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent, ConfigWindow,
            ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Screen,
            Window as X11Window, WindowClass,
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
        // wm selections & wayland-stuff
        WM_S0,
        WL_SURFACE_ID,

        // private
        _LATE_SURFACE_ID,
        _SMITHAY_CLOSE_CONNECTION,

        // data formats
        UTF8_STRING,

        // client -> server
        _NET_WM_NAME,

        // server -> client
        WM_STATE,
    }
}

#[derive(Debug, Clone)]
pub struct X11Surface {
    window: X11Window,
    override_redirect: bool,
    conn: Weak<RustConnection>,
    atoms: Atoms,
    state: Arc<Mutex<SharedSurfaceState>>,
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
                    let property = [0u32 /*WithdrawnState*/, 0 /*WINDOW_NONE*/];
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

    pub fn is_client_mapped(&self) -> bool {
        self.override_redirect || self.state.lock().unwrap().mapped_onto.is_some()
    }

    pub fn is_visible(&self) -> bool {
        let state = self.state.lock().unwrap();
        (self.override_redirect || state.mapped_onto.is_some()) && state.wl_surface.is_some()
    }

    pub fn alive(&self) -> bool {
        self.state.lock().unwrap().alive
    }

    pub fn configure(&mut self, rect: Rectangle<i32, Logical>) -> Result<(), X11SurfaceError> {
        if self.override_redirect {
            return Err(X11SurfaceError::UnsupportedForOverrideRedirect);
        }

        if let Some(conn) = self.conn.upgrade() {
            let aux = ConfigureWindowAux::default()
                .x(rect.loc.x)
                .y(rect.loc.y)
                .width(rect.size.w as u32)
                .height(rect.size.h as u32);
            if let Some(frame) = self.state.lock().unwrap().mapped_onto {
                let win_aux = ConfigureWindowAux::default()
                    .width(rect.size.w as u32)
                    .height(rect.size.h as u32);
                conn.configure_window(frame, &aux)?;
                conn.configure_window(self.window, &win_aux)?;
            } else {
                conn.configure_window(self.window, &aux)?;
            }
            conn.flush()?;
            // TODO: This should technically happen later on a ConfigureNotify
            let mut state = self.state.lock().unwrap();
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

    fn update_properties(&self, atom: Option<Atom>) -> Result<(), ConnectionError> {
        match atom {
            Some(atom) if atom == self.atoms._NET_WM_NAME || atom == AtomEnum::WM_NAME.into() => {
                self.update_title()
            }
            Some(atom) if atom == AtomEnum::WM_CLASS.into() => self.update_class(),
            Some(_) => Ok(()), // unknown
            None => {
                self.update_title()?;
                self.update_class()?;
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

    fn update_title(&self) -> Result<(), ConnectionError> {
        let title = self
            .read_window_property_string(self.atoms._NET_WM_NAME)?
            .or(self.read_window_property_string(AtomEnum::WM_NAME)?)
            .unwrap_or_default();

        let mut state = self.state.lock().unwrap();
        state.title = title;
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
                    | EventMask::PROPERTY_CHANGE,
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
        conn.composite_redirect_subwindows(screen.root, Redirect::MANUAL)?;

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
                    })),
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
                    self.conn.flush()?;
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
                    // TODO: If the window is not configured as part of this callback, we need to send a synthetic configure event
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
                    callback(XwmEvent::UnmappedWindowNotify {
                        window: surface.clone(),
                    });
                    {
                        let mut state = surface.state.lock().unwrap();
                        state.wl_surface = None;
                    }
                    self.conn.flush()?;
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
            Event::ClientMessage(msg) => {
                if msg.type_ == self.atoms.WL_SURFACE_ID {
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

                        let wl_surface = self.wl_client.object_from_protocol_id::<WlSurface>(&self.dh, id);
                        match wl_surface {
                            Err(_) => {
                                self.unpaired_surfaces.insert(id, msg.window);
                            }
                            Ok(wl_surface) => {
                                Self::new_surface(surface, wl_surface, self.log.clone());
                            }
                        }
                    }
                } else if msg.type_ == self.atoms._LATE_SURFACE_ID {
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
            }
            _ => {}
        }
        self.conn.flush()?;
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
