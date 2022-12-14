#![allow(missing_docs)]

use crate::{
    utils::x11rb::X11Source,
    wayland::compositor::{get_role, give_role},
};
use calloop::{channel::SyncSender, LoopHandle};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    convert::TryFrom,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};
use wayland_server::{protocol::wl_surface::WlSurface, Client, DisplayHandle, Resource};

use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent, ConfigWindow,
            ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Screen, StackMode,
            Window as X11Window, WindowClass,
        },
        Event,
    },
    rust_connection::{ConnectionError, DefaultStream, RustConnection},
    wrapper::ConnectionExt,
    COPY_DEPTH_FROM_PARENT,
};

mod surface;
pub use self::surface::*;
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

crate::utils::ids::id_gen!(next_xwm_id, XWM_ID, XWM_IDS);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwmId(usize);

pub trait XwmHandler {
    fn xwm_state(&mut self, xwm: XwmId) -> &mut X11WM;

    fn new_window(&mut self, xwm: XwmId, window: X11Surface);
    fn new_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface);
    fn map_window_request(&mut self, xwm: XwmId, window: X11Surface);
    fn mapped_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface);
    fn unmapped_window(&mut self, xwm: XwmId, window: X11Surface);
    fn destroyed_window(&mut self, xwm: XwmId, window: X11Surface);

    fn configure_request(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
    );
    fn configure_notify(&mut self, xwm: XwmId, window: X11Surface, x: i32, y: i32, w: u32, h: u32);

    fn maximize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    fn unmaximize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    fn fullscreen_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    fn unfullscreen_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    fn minimize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    fn unminimize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }

    fn resize_request(&mut self, xwm: XwmId, window: X11Surface, button: u32, resize_edge: ResizeEdge);
    fn move_request(&mut self, xwm: XwmId, window: X11Surface, button: u32);
}

/// The runtime state of the XWayland window manager.
#[derive(Debug)]
pub struct X11WM {
    id: XwmId,
    conn: Arc<RustConnection>,
    dh: DisplayHandle,
    screen: Screen,
    wm_window: X11Window,
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

impl Drop for X11WM {
    fn drop(&mut self) {
        XWM_IDS.lock().unwrap().remove(&self.id.0);
    }
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
    pub fn start_wm<D, L>(
        handle: LoopHandle<'_, D>,
        dh: DisplayHandle,
        connection: UnixStream,
        client: Client,
        log: L,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        D: XwmHandler,
        L: Into<Option<::slog::Logger>>,
    {
        // Create an X11 connection. XWayland only uses screen 0.
        let log = crate::slog_or_fallback(log);
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
        slog::debug!(log, "WM Window Id: {}", win);
        conn.flush()?;

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

        let id = XwmId(next_xwm_id());
        let wm = Self {
            id,
            dh,
            conn,
            screen,
            atoms,
            wm_window: win,
            wl_client: client,
            unpaired_surfaces: Default::default(),
            sequences_to_ignore: Default::default(),
            windows: Vec::new(),
            client_list: Vec::new(),
            client_list_stacking: Vec::new(),
            log: log.clone(),
        };

        handle.insert_source(source, move |event, _, data| {
            if let Err(err) = handle_event(data, id, event) {
                slog::warn!(log, "Failed to handle X11 event ({:?}): {}", id, err);
            }
        })?;
        Ok(wm)
    }

    pub fn id(&self) -> XwmId {
        self.id
    }

    pub fn raise_window<'a, W: X11Relatable + 'a>(&mut self, window: &'a W) -> Result<(), ConnectionError> {
        if let Some(elem) = self.windows.iter().find(|s| window.is_window(s)) {
            self.conn.grab_server()?;
            self.conn.configure_window(
                elem.window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
            self.client_list_stacking.retain(|e| *e != elem.window);
            self.client_list_stacking.push(elem.window);
            self.conn.change_property32(
                PropMode::REPLACE,
                self.screen.root,
                self.atoms._NET_CLIENT_LIST_STACKING,
                AtomEnum::WINDOW,
                &self.client_list_stacking,
            )?;
            self.conn.ungrab_server()?;
            self.conn.flush()?;
        }
        Ok(())
    }

    pub fn update_stacking_order_downwards<'a, W: X11Relatable + 'a>(
        &mut self,
        order: impl Iterator<Item = &'a W>,
    ) -> Result<(), ConnectionError> {
        let mut last_pos = None;
        self.conn.grab_server()?;
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
        self.conn.change_property32(
            PropMode::REPLACE,
            self.screen.root,
            self.atoms._NET_CLIENT_LIST_STACKING,
            AtomEnum::WINDOW,
            &self.client_list_stacking,
        )?;
        self.conn.ungrab_server()?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn update_stacking_order_upwards<'a, W: X11Relatable + 'a>(
        &mut self,
        order: impl Iterator<Item = &'a W>,
    ) -> Result<(), ConnectionError> {
        let mut last_pos = None;
        self.conn.grab_server()?;
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
        self.conn.change_property32(
            PropMode::REPLACE,
            self.screen.root,
            self.atoms._NET_CLIENT_LIST_STACKING,
            AtomEnum::WINDOW,
            &self.client_list_stacking,
        )?;
        self.conn.ungrab_server()?;
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

fn handle_event<D: XwmHandler>(state: &mut D, xwmid: XwmId, event: Event) -> Result<(), ReplyOrIdError> {
    let xwm = state.xwm_state(xwmid);
    let id = xwm.id;
    let conn = xwm.conn.clone();

    let mut should_ignore = false;
    if let Some(seqno) = event.wire_sequence_number() {
        // Check sequences_to_ignore and remove entries with old (=smaller) numbers.
        while let Some(&Reverse(to_ignore)) = xwm.sequences_to_ignore.peek() {
            // Sequence numbers can wrap around, so we cannot simply check for
            // "to_ignore <= seqno". This is equivalent to "to_ignore - seqno <= 0", which is what we
            // check instead. Since sequence numbers are unsigned, we need a trick: We decide
            // that values from [MAX/2, MAX] count as "<= 0" and the rest doesn't.
            if to_ignore.wrapping_sub(seqno) <= u16::max_value() / 2 {
                // If the two sequence numbers are equal, this event should be ignored.
                should_ignore = to_ignore == seqno;
                break;
            }
            xwm.sequences_to_ignore.pop();
        }
    }

    slog::debug!(
        xwm.log,
        "X11: Got event {:?}{}",
        event,
        if should_ignore { " [ignored]" } else { "" }
    );
    if should_ignore {
        return Ok(());
    }

    match event {
        Event::CreateNotify(n) => {
            if n.window == xwm.wm_window {
                return Ok(());
            }

            if xwm
                .windows
                .iter()
                .any(|s| s.state.lock().unwrap().mapped_onto == Some(n.window))
            {
                return Ok(());
            }

            let geo = conn.get_geometry(n.window)?.reply()?;

            let surface = X11Surface {
                window: n.window,
                override_redirect: n.override_redirect,
                conn: Arc::downgrade(&conn),
                atoms: xwm.atoms,
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
                log: xwm.log.new(slog::o!("X11 Window" => n.window)),
            };
            surface.update_properties(None)?;
            xwm.windows.push(surface.clone());

            if n.override_redirect {
                state.new_override_redirect_window(id, surface);
            } else {
                state.new_window(id, surface);
            }
        }
        Event::MapRequest(r) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window == r.window).cloned() {
                xwm.client_list.push(surface.window);

                // we reparent windows, because a lot of stuff expects, that we do
                let geo = conn.get_geometry(r.window)?.reply()?;
                let win = r.window;
                let frame_win = conn.generate_id()?;
                let win_aux = CreateWindowAux::new()
                    .event_mask(EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT);

                conn.grab_server()?;
                let cookie1 = conn.create_window(
                    COPY_DEPTH_FROM_PARENT,
                    frame_win,
                    xwm.screen.root,
                    geo.x,
                    geo.y,
                    geo.width,
                    geo.height,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    x11rb::COPY_FROM_PARENT,
                    &win_aux,
                )?;
                let cookie2 = conn.reparent_window(win, frame_win, 0, 0)?;
                conn.map_window(win)?;
                conn.change_property32(
                    PropMode::APPEND,
                    xwm.screen.root,
                    xwm.atoms._NET_CLIENT_LIST,
                    AtomEnum::WINDOW,
                    &[win],
                )?;
                conn.ungrab_server()?;

                // Ignore all events caused by reparent_window(). All those events have the sequence number
                // of the reparent_window() request, thus remember its sequence number. The
                // grab_server()/ungrab_server() is done so that the server does not handle other clients
                // in-between, which could cause other events to get the same sequence number.
                xwm.sequences_to_ignore
                    .push(Reverse(cookie1.sequence_number() as u16));
                xwm.sequences_to_ignore
                    .push(Reverse(cookie2.sequence_number() as u16));

                surface.state.lock().unwrap().mapped_onto = Some(frame_win);
                state.map_window_request(id, surface);
            }
        }
        Event::MapNotify(n) => {
            slog::trace!(xwm.log, "X11 Window mapped: {}", n.window);
            if let Some(surface) = xwm.windows.iter().find(|x| x.window == n.window).cloned() {
                xwm.client_list_stacking.push(surface.window);
                conn.change_property32(
                    PropMode::APPEND,
                    xwm.screen.root,
                    xwm.atoms._NET_CLIENT_LIST_STACKING,
                    AtomEnum::WINDOW,
                    &[surface.window],
                )?;
                if surface.override_redirect {
                    state.mapped_override_redirect_window(id, surface);
                }
            } else if let Some(surface) = xwm
                .windows
                .iter()
                .find(|x| x.state.lock().unwrap().mapped_onto.unwrap() == n.window)
            {
                xwm.client_list_stacking.push(surface.window);
                conn.change_property32(
                    PropMode::APPEND,
                    xwm.screen.root,
                    xwm.atoms._NET_CLIENT_LIST_STACKING,
                    AtomEnum::WINDOW,
                    &[surface.window],
                )?;
            }
        }
        Event::ConfigureRequest(r) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window == r.window).cloned() {
                // Pass the request to downstream to decide
                state.configure_request(
                    id,
                    surface.clone(),
                    if r.value_mask & u16::from(ConfigWindow::X) != 0 {
                        Some(i32::try_from(r.x).unwrap())
                    } else {
                        None
                    },
                    if r.value_mask & u16::from(ConfigWindow::Y) != 0 {
                        Some(i32::try_from(r.y).unwrap())
                    } else {
                        None
                    },
                    if r.value_mask & u16::from(ConfigWindow::WIDTH) != 0 {
                        Some(u32::try_from(r.width).unwrap())
                    } else {
                        None
                    },
                    if r.value_mask & u16::from(ConfigWindow::HEIGHT) != 0 {
                        Some(u32::try_from(r.height).unwrap())
                    } else {
                        None
                    },
                );
                // Synthetic event
                surface.configure(None).map_err(|err| match err {
                    X11SurfaceError::Connection(err) => err,
                    X11SurfaceError::UnsupportedForOverrideRedirect => unreachable!(),
                })?;
            }
        }
        Event::ConfigureNotify(n) => {
            slog::trace!(xwm.log, "X11 Window configured: {:?}", n);
            if let Some(surface) = xwm
                .windows
                .iter()
                .find(|x| x.state.lock().unwrap().mapped_onto == Some(n.window))
                .cloned()
            {
                state.configure_notify(
                    id,
                    surface,
                    n.x as i32,
                    n.y as i32,
                    n.width as u32,
                    n.height as u32,
                );
            } else if let Some(surface) = xwm.windows.iter().find(|x| x.window == n.window).cloned() {
                if surface.override_redirect {
                    state.configure_notify(
                        id,
                        surface,
                        n.x as i32,
                        n.y as i32,
                        n.width as u32,
                        n.height as u32,
                    );
                }
            }
        }
        Event::UnmapNotify(n) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window == n.window).cloned() {
                xwm.client_list.retain(|w| *w != surface.window);
                xwm.client_list_stacking.retain(|w| *w != surface.window);
                conn.grab_server()?;
                conn.change_property32(
                    PropMode::REPLACE,
                    xwm.screen.root,
                    xwm.atoms._NET_CLIENT_LIST,
                    AtomEnum::WINDOW,
                    &xwm.client_list,
                )?;
                conn.change_property32(
                    PropMode::REPLACE,
                    xwm.screen.root,
                    xwm.atoms._NET_CLIENT_LIST_STACKING,
                    AtomEnum::WINDOW,
                    &xwm.client_list_stacking,
                )?;
                {
                    let mut state = surface.state.lock().unwrap();
                    conn.reparent_window(
                        n.window,
                        xwm.screen.root,
                        state.location.x as i16,
                        state.location.y as i16,
                    )?;
                    if let Some(frame) = state.mapped_onto.take() {
                        conn.destroy_window(frame)?;
                    }
                }
                conn.ungrab_server()?;
                state.unmapped_window(id, surface.clone());
                {
                    let mut state = surface.state.lock().unwrap();
                    state.wl_surface = None;
                }
            }
        }
        Event::DestroyNotify(n) => {
            if let Some(pos) = xwm.windows.iter().position(|x| x.window == n.window) {
                let surface = xwm.windows.remove(pos);
                surface.state.lock().unwrap().alive = false;
                state.destroyed_window(id, surface);
            }
        }
        Event::PropertyNotify(n) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window == n.window) {
                surface.update_properties(Some(n.atom))?;
            }
        }
        Event::FocusIn(n) => {
            conn.change_property32(
                PropMode::REPLACE,
                xwm.screen.root,
                xwm.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                &[n.event],
            )?;
        }
        Event::FocusOut(n) => {
            conn.change_property32(
                PropMode::REPLACE,
                xwm.screen.root,
                xwm.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                &[n.event],
            )?;
        }
        Event::ClientMessage(msg) => {
            match msg.type_ {
                x if x == xwm.atoms.WL_SURFACE_ID => {
                    let id = msg.data.as_data32()[0];
                    slog::info!(
                        xwm.log,
                        "X11 surface {:?} corresponds to WlSurface {:?}",
                        msg.window,
                        id,
                    );
                    if let Some(surface) = xwm
                        .windows
                        .iter_mut()
                        .find(|x| x.state.lock().unwrap().mapped_onto == Some(msg.window))
                    {
                        // We get a WL_SURFACE_ID message when Xwayland creates a WlSurface for a
                        // window. Both the creation of the surface and this client message happen at
                        // roughly the same time and are sent over different sockets (X11 socket and
                        // wayland socket). Thus, we could receive these two in any order. Hence, it
                        // can happen that we get None below when X11 was faster than Wayland.

                        let wl_surface = xwm.wl_client.object_from_protocol_id::<WlSurface>(&xwm.dh, id);
                        match wl_surface {
                            Err(_) => {
                                xwm.unpaired_surfaces.insert(id, msg.window);
                            }
                            Ok(wl_surface) => {
                                X11WM::new_surface(surface, wl_surface, xwm.log.clone());
                            }
                        }
                    }
                }
                x if x == xwm.atoms._LATE_SURFACE_ID => {
                    let id = msg.data.as_data32()[0];
                    if let Some(window) = dbg!(&mut xwm.unpaired_surfaces).remove(&id) {
                        if let Some(surface) = xwm
                            .windows
                            .iter_mut()
                            .find(|x| x.state.lock().unwrap().mapped_onto == Some(window))
                        {
                            let wl_surface = xwm
                                .wl_client
                                .object_from_protocol_id::<WlSurface>(&xwm.dh, id)
                                .unwrap();
                            X11WM::new_surface(surface, wl_surface, xwm.log.clone());
                        }
                    }
                }
                x if x == xwm.atoms.WM_CHANGE_STATE => {
                    if let Some(surface) = xwm.windows.iter().find(|x| x.window == msg.window).cloned() {
                        state.minimize_request(id, surface);
                    }
                }
                x if x == xwm.atoms._NET_WM_STATE => {
                    if let Some(surface) = xwm.windows.iter().find(|x| x.window == msg.window).cloned() {
                        let data = msg.data.as_data32();
                        match &data[1..=2] {
                            &[x, y]
                                if (x == xwm.atoms._NET_WM_STATE_MAXIMIZED_HORZ
                                    && y == xwm.atoms._NET_WM_STATE_MAXIMIZED_VERT)
                                    || (x == xwm.atoms._NET_WM_STATE_MAXIMIZED_VERT
                                        && y == xwm.atoms._NET_WM_STATE_MAXIMIZED_HORZ) =>
                            {
                                match data[0] {
                                    0 => state.unmaximize_request(id, surface),
                                    1 => state.maximize_request(id, surface),
                                    2 => {
                                        if surface.is_maximized() {
                                            state.unmaximize_request(id, surface)
                                        } else {
                                            state.maximize_request(id, surface)
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            actions if actions.contains(&xwm.atoms._NET_WM_STATE_FULLSCREEN) => {
                                match data[0] {
                                    0 => state.unfullscreen_request(id, surface),
                                    1 => state.fullscreen_request(id, surface),
                                    2 => {
                                        if surface.is_fullscreen() {
                                            state.unfullscreen_request(id, surface)
                                        } else {
                                            state.fullscreen_request(id, surface)
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                }
                x if x == xwm.atoms._NET_WM_MOVERESIZE => {
                    if let Some(surface) = xwm.windows.iter().find(|x| x.window == msg.window).cloned() {
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
                                state.resize_request(id, surface, data[3], resize_edge);
                            }
                            8 => state.move_request(id, surface, data[3]),
                            _ => {} // ignore keyboard moves/resizes for now
                        }
                    }
                }
                x => {
                    slog::debug!(
                        xwm.log,
                        "Unhandled client msg of type {:?}",
                        String::from_utf8(conn.get_atom_name(x)?.reply_unchecked()?.unwrap().name).ok()
                    )
                }
            }
        }
        _ => {}
    }
    conn.flush()?;
    Ok(())
}
