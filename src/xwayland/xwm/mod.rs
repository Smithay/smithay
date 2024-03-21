//!
//! Xwayland Window Manager module
//!
//! Provides an [`X11Wm`] type, which will register itself as a window manager for a previously spawned Xwayland instances,
//! allowing backwards-compatibility by seemlessly integrating X11 windows into a wayland compositor.
//!
//! To use this functionality you must first spawn an [`XWayland`](super::XWayland) instance to attach a [`X11Wm`] to.
//!
//! ```no_run
//! #  use smithay::wayland::selection::SelectionTarget;
//! #  use smithay::xwayland::{XWayland, XWaylandEvent, X11Wm, X11Surface, XwmHandler, xwm::{XwmId, ResizeEdge, Reorder}};
//! #  use smithay::utils::{Rectangle, Logical};
//! #  use std::os::unix::io::OwnedFd;
//! #  use std::process::Stdio;
//! #
//! struct State { /* ... */ }
//! impl XwmHandler for State {
//!     fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm {
//!         // ...
//! #       unreachable!()
//!     }
//!     fn new_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn new_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn map_window_request(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn mapped_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn unmapped_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn destroyed_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn configure_request(&mut self, xwm: XwmId, window: X11Surface, x: Option<i32>, y: Option<i32>, w: Option<u32>, h: Option<u32>, reorder: Option<Reorder>) { /* ... */ }
//!     fn configure_notify(&mut self, xwm: XwmId, window: X11Surface, geometry: Rectangle<i32, Logical>, above: Option<u32>) { /* ... */ }
//!     fn resize_request(&mut self, xwm: XwmId, window: X11Surface, button: u32, resize_edge: ResizeEdge) { /* ... */ }
//!     fn move_request(&mut self, xwm: XwmId, window: X11Surface, button: u32) { /* ... */ }
//!     fn send_selection(&mut self, xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) { /* ... */ }
//! }
//! #
//! # let dh = unreachable!();
//! # let handle: smithay::reexports::calloop::LoopHandle<'static, State> = unreachable!();
//!
//! let (xwayland, client) = XWayland::spawn(
//!     &dh,
//!     None,
//!     std::iter::empty::<(String, String)>(),
//!     true,
//!     Stdio::null(),
//!     Stdio::null(),
//!     |_| (),
//! )
//! .expect("failed to start XWayland");

//! let ret = handle.insert_source(xwayland, move |event, _, data| match event {
//!     XWaylandEvent::Ready {
//!         x11_socket,
//!         display_number: _,
//!     } => {
//!         let wm = X11Wm::start_wm(
//!             handle.clone(),
//!             dh.clone(),
//!             x11_socket,
//!             client.clone(),
//!         )
//!         .expect("Failed to attach X11 Window Manager");
//!         
//!         // store the WM somewhere
//!     }
//!     XWaylandEvent::Error => eprintln!("XWayland failed to start!"),
//! });
//! if let Err(e) = ret {
//!     tracing::error!(
//!         "Failed to insert the XWaylandSource into the event loop: {}", e
//!     );
//! }
//! ```
//!

use crate::{
    utils::{x11rb::X11Source, Logical, Point, Rectangle, Size},
    wayland::{
        compositor::{get_role, give_role},
        selection::SelectionTarget,
    },
};
use calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use rustix::fs::OFlags;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    fmt,
    os::unix::{
        io::{AsFd, BorrowedFd, OwnedFd},
        net::UnixStream,
    },
    sync::Arc,
};
use tracing::{debug, debug_span, error, info, trace, warn};
use wayland_server::{protocol::wl_surface::WlSurface, Client, DisplayHandle, Resource};

use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        render::{ConnectionExt as _, CreatePictureAux, PictureWrapper},
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConfigWindow, ConfigureNotifyEvent,
            ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, CursorWrapper, EventMask,
            FontWrapper, GcontextWrapper, GetPropertyReply, ImageFormat, PixmapWrapper, PropMode, Property,
            QueryExtensionReply, Screen, SelectionNotifyEvent, SelectionRequestEvent, StackMode,
            Window as X11Window, WindowClass, CONFIGURE_NOTIFY_EVENT, SELECTION_NOTIFY_EVENT,
        },
        Event,
    },
    rust_connection::{ConnectionError, DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
    COPY_DEPTH_FROM_PARENT,
};

mod surface;
pub use self::surface::*;
use super::xserver::XWaylandClientData;

/// X11 wl_surface role
pub const X11_SURFACE_ROLE: &str = "x11_surface";
// copied from wlroots - docs say "maximum size can vary widely depending on the implementation"
// and there is no way to query the maximum size, you just get a non-descriptive `Length` error...
const INCR_CHUNK_SIZE: usize = 64 * 1024;

#[allow(missing_docs)]
mod atoms {
    x11rb::atom_manager! {
        /// Atoms used by the XWM and X11Surface types
        pub Atoms:
        AtomsCookie {
            // wayland-stuff
            WL_SURFACE_ID,

            // private
            _SMITHAY_CLOSE_CONNECTION,

            // data formats
            UTF8_STRING,
            TEXT,

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
            _NET_STARTUP_ID,

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

            // selection
            _WL_SELECTION,
            CLIPBOARD_MANAGER,
            CLIPBOARD,
            PRIMARY,
            TARGETS,
            TIMESTAMP,
            INCR,
            DELETE,
        }
    }
}
pub use self::atoms::Atoms;

crate::utils::ids::id_gen!(next_xwm_id, XWM_ID, XWM_IDS);

/// Id of an X11 WM
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwmId(usize);

/// Window asks to be re-stacked
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reorder {
    /// to the top of the stack
    Top,
    /// directly above the given window id
    Above(X11Window),
    /// directly below the given window id
    Below(X11Window),
    /// to the bottom of the stack
    Bottom,
}

enum StackingDirection {
    Downwards,
    Upwards,
}

impl StackingDirection {
    fn pos_comparator(&self, pos: usize, last_pos: usize) -> bool {
        match self {
            Self::Downwards => last_pos < pos,
            Self::Upwards => last_pos > pos,
        }
    }

    fn stack_mode(&self) -> StackMode {
        match self {
            Self::Downwards => StackMode::BELOW,
            Self::Upwards => StackMode::ABOVE,
        }
    }
}

/// Handler trait for X11Wm interactions
pub trait XwmHandler {
    /// [`X11Wm`] getter for a given ID.
    fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm;

    /// A new X11 window was created.
    ///
    /// New windows are not mapped yet, but various information is already accessible.
    /// In general new windows will either stay in this state, if they serve secondary purposes
    /// or request to be mapped shortly afterwards.
    fn new_window(&mut self, xwm: XwmId, window: X11Surface);
    /// A new X11 window with the override redirect flag.
    ///
    /// New override_redirect windows are not mapped yet, but can become any time.
    /// Window manager are not supposed to manage these windows and thus cannot intercept
    /// most operations (including mapping).
    ///
    /// It is best to replicate their state in smithay as faithfully as possible (e.g. positioning)
    /// and don't touch their state in any way.
    fn new_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface);
    /// Window requests to be mapped.
    ///
    /// To grant the wish you have to call `X11Surface::set_mapped(true)` for the window to become visible.
    fn map_window_request(&mut self, xwm: XwmId, window: X11Surface);
    /// Notification a window was mapped sucessfully and now has a usable `wl_surface` attached.
    fn map_window_notify(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    /// Override redirect window was mapped.
    ///
    /// This is a notification. The XWM cannot prohibit override redirect windows to become mapped.
    fn mapped_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface);
    /// Window was unmapped.
    fn unmapped_window(&mut self, xwm: XwmId, window: X11Surface);
    /// Window was destroyed
    fn destroyed_window(&mut self, xwm: XwmId, window: X11Surface);

    /// Window asks to be positioned or sized differently.
    ///
    /// Requests can be granted by calling [`X11Surface::configure`] with updated values.
    #[allow(clippy::too_many_arguments)]
    fn configure_request(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        reorder: Option<Reorder>,
    );
    /// Window was reconfigured.
    ///
    /// This call will be done as a notification for both normal and override-redirect windows.
    /// Override-redirect windows can modify their size, position and stack order at any time and the compositor
    /// should properly reflect these new values to avoid bugs.
    fn configure_notify(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        above: Option<X11Window>,
    );

    /// Window requests to be maximized.
    fn maximize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    /// Window requests to be unmaximized.
    fn unmaximize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    /// Window requests to be fullscreened.
    fn fullscreen_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    /// Window requests to be unfullscreened.
    fn unfullscreen_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    /// Window requests to be minimized.
    fn minimize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }
    /// Window requests to be unminimized.
    fn unminimize_request(&mut self, xwm: XwmId, window: X11Surface) {
        let _ = (xwm, window);
    }

    /// Window requests to be resized.
    ///
    /// The window will be holding a grab on the mouse button provided and requests
    /// to be resized on the edges passed.
    fn resize_request(&mut self, xwm: XwmId, window: X11Surface, button: u32, resize_edge: ResizeEdge);
    /// Window requests to be moved.
    ///
    /// The window will be holding a grab on the mouse button provided.
    fn move_request(&mut self, xwm: XwmId, window: X11Surface, button: u32);

    /// Window requests access to the given selection.
    fn allow_selection_access(&mut self, xwm: XwmId, selection: SelectionTarget) -> bool {
        let _ = (xwm, selection);
        false
    }

    /// The given selection is being read by an X client and needs to be written to the provided file descriptor
    fn send_selection(&mut self, xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        let _ = (xwm, selection, mime_type, fd);
        panic!("`allow_selection_access` returned true without `send_selection` implementation to handle transfers.");
    }

    /// A new selection was set by an X client with provided mime_types
    fn new_selection(&mut self, xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        let _ = (xwm, selection, mime_types);
    }

    /// A proviously set selection of an X client got cleared
    fn cleared_selection(&mut self, xwm: XwmId, selection: SelectionTarget) {
        let _ = (xwm, selection);
    }
}

/// The runtime state of an reparenting XWayland window manager.
#[derive(Debug)]
pub struct X11Wm {
    id: XwmId,
    conn: Arc<RustConnection>,
    dh: DisplayHandle,
    screen: Screen,
    wm_window: X11Window,
    atoms: Atoms,

    wl_client: Client,
    unpaired_surfaces: HashMap<u32, X11Window>,
    sequences_to_ignore: BinaryHeap<Reverse<u16>>,

    // selections
    _xfixes_data: QueryExtensionReply,
    clipboard: XWmSelection,
    primary: XWmSelection,

    windows: Vec<X11Surface>,
    // oldest mapped -> newest
    client_list: Vec<X11Window>,
    // bottom -> top
    client_list_stacking: Vec<X11Window>,

    span: tracing::Span,
}

impl Drop for X11Wm {
    fn drop(&mut self) {
        // TODO: Not really needed for Xwayland, but maybe cleanup set root properties?
        let _ = self.conn.destroy_window(self.wm_window);
        XWM_IDS.lock().unwrap().remove(&self.id.0);
    }
}

#[derive(Debug)]
struct XWmSelection {
    atom: Atom,
    type_: SelectionTarget,

    conn: Arc<RustConnection>,
    window: X11Window,
    owner: X11Window,
    mime_types: Vec<String>,
    timestamp: u32,

    incoming: Vec<IncomingTransfer>,
    outgoing: Vec<OutgoingTransfer>,
}

struct IncomingTransfer {
    conn: Arc<RustConnection>,
    token: Option<RegistrationToken>,
    window: X11Window,

    incr: bool,
    source_data: Vec<u8>,
    incr_done: bool,
}

impl fmt::Debug for IncomingTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncomingTransfer")
            .field("conn", &"...")
            .field("token", &self.token)
            .field("window", &self.window)
            .field("incr", &self.incr)
            .field("source_data", &self.source_data)
            .field("incr_done", &self.incr_done)
            .finish()
    }
}

impl IncomingTransfer {
    fn read_selection_prop(&mut self, reply: GetPropertyReply) {
        self.source_data.extend(&reply.value)
    }

    fn write_selection(&mut self, fd: BorrowedFd<'_>) -> std::io::Result<bool> {
        if self.source_data.is_empty() {
            return Ok(true);
        }

        let len = rustix::io::write(fd, &self.source_data)?;
        self.source_data = self.source_data.split_off(len);

        Ok(self.source_data.is_empty())
    }

    fn destroy<D>(mut self, handle: &LoopHandle<'_, D>) {
        if let Some(token) = self.token.take() {
            handle.remove(token);
        }
    }
}

impl Drop for IncomingTransfer {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.window);
        if self.token.is_some() {
            tracing::warn!(
                ?self,
                "IncomingTransfer freed before being removed from EventLoop"
            );
        }
    }
}

struct OutgoingTransfer {
    conn: Arc<RustConnection>,
    token: Option<RegistrationToken>,

    incr: bool,
    source_data: Vec<u8>,
    request: SelectionRequestEvent,

    property_set: bool,
    flush_property_on_delete: bool,
}

impl fmt::Debug for OutgoingTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingTransfer")
            .field("conn", &"...")
            .field("token", &self.token)
            .field("incr", &self.incr)
            .field("source_data", &self.source_data)
            .field("request", &self.request)
            .field("property_set", &self.property_set)
            .field("flush_property_on_delete", &self.flush_property_on_delete)
            .finish()
    }
}

impl OutgoingTransfer {
    fn flush_data(&mut self) -> Result<usize, ReplyOrIdError> {
        let len = std::cmp::min(self.source_data.len(), INCR_CHUNK_SIZE);
        let mut data = self.source_data.split_off(len);
        std::mem::swap(&mut data, &mut self.source_data);

        self.conn.change_property8(
            PropMode::REPLACE,
            self.request.requestor,
            self.request.property,
            self.request.target,
            &data,
        )?;
        self.conn.flush()?;

        let remaining = self.source_data.len();
        self.source_data = Vec::new();
        self.property_set = true;
        Ok(remaining)
    }

    fn destroy<D>(mut self, handle: &LoopHandle<'_, D>) {
        if let Some(token) = self.token.take() {
            handle.remove(token);
        }
    }
}

impl Drop for OutgoingTransfer {
    fn drop(&mut self) {
        if self.token.is_some() {
            tracing::warn!(
                ?self,
                "OutgoingTransfer freed before being removed from EventLoop"
            );
        }
    }
}

impl XWmSelection {
    fn new(
        conn: &Arc<RustConnection>,
        screen: &Screen,
        atoms: &Atoms,
        atom: Atom,
    ) -> Result<Self, ReplyOrIdError> {
        let window = conn.generate_id()?;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            10,
            10,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;

        if atom == atoms.CLIPBOARD {
            conn.set_selection_owner(window, atoms.CLIPBOARD_MANAGER, x11rb::CURRENT_TIME)?;
        }
        conn.xfixes_select_selection_input(
            window,
            atom,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        conn.flush()?;

        let selection = match atom {
            x if x == atoms.CLIPBOARD => SelectionTarget::Clipboard,
            x if x == atoms.PRIMARY => SelectionTarget::Primary,
            _ => unreachable!(),
        };

        debug!(
            selection_window = ?window,
            ?selection,
            ?atom,
            "Selection init",
        );

        Ok(XWmSelection {
            atom,
            type_: selection,
            conn: conn.clone(),
            window,
            owner: window,
            mime_types: Vec::new(),
            timestamp: x11rb::CURRENT_TIME,
            incoming: Vec::new(),
            outgoing: Vec::new(),
        })
    }
}

impl Drop for XWmSelection {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.window);
    }
}

struct X11Injector<D: XwmHandler> {
    xwm: XwmId,
    handle: LoopHandle<'static, D>,
}
impl<D: XwmHandler> X11Injector<D> {
    pub fn late_window(&self, surface: &WlSurface) {
        let xwm_id = self.xwm;
        let id = surface.id().protocol_id();

        self.handle.insert_idle(move |data| {
            let xwm = data.xwm_state(xwm_id);

            if let Some(window) = xwm.unpaired_surfaces.remove(&id) {
                if let Some(surface) = xwm
                    .windows
                    .iter()
                    .find(|x| x.window_id() == window || x.mapped_window_id() == Some(window))
                {
                    let wl_surface = xwm
                        .wl_client
                        .object_from_protocol_id::<WlSurface>(&xwm.dh, id)
                        .unwrap();
                    let surface = surface.clone();
                    X11Wm::new_surface(data, xwm_id, surface, wl_surface);
                }
            }
        });
    }
}

/// Edge values for resizing
///
// These values are used to indicate which edge of a surface is being dragged in a resize operation.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[allow(missing_docs)]
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

/// Errors generated working with Xwm Selections
#[derive(thiserror::Error, Debug)]
pub enum SelectionError {
    /// X11 Error occured setting the selection
    #[error(transparent)]
    X11Error(#[from] ReplyOrIdError),
    /// Calloop error occured trying to register transfer with event loop
    #[error(transparent)]
    Calloop(#[from] calloop::Error),
    /// Unable to determine internal Atom for given mime-type
    #[error("Unable to determine ATOM matching mime-type")]
    UnableToDetermineAtom,
}

impl From<ConnectionError> for SelectionError {
    fn from(err: ConnectionError) -> Self {
        SelectionError::X11Error(err.into())
    }
}

impl X11Wm {
    /// Start a new window manager for a given Xwayland connection
    ///
    /// ## Arguments
    /// - `handle` is an eventloop handle used to queue up and handle incoming X11 events
    /// - `dh` is the corresponding display handle to the wayland connection of the Xwayland instance
    /// - `connection` is the corresponding x11 client connection of the Xwayland instance
    /// - `client` is the wayland client instance of the Xwayland instance
    pub fn start_wm<D>(
        handle: LoopHandle<'static, D>,
        dh: DisplayHandle,
        connection: UnixStream,
        client: Client,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        D: XwmHandler + 'static,
    {
        let id = XwmId(next_xwm_id());
        let span = debug_span!("xwayland_wm", id = id.0);
        let _guard = span.enter();

        // Create an X11 connection. XWayland only uses screen 0.
        let screen = 0;
        let stream = DefaultStream::from_unix_stream(connection)?.0;
        let conn = RustConnection::connect_to_stream(stream, screen)?;
        let atoms = Atoms::new(&conn)?.reply()?;

        let screen = conn.setup().roots[0].clone();

        {
            let font = FontWrapper::open_font(&conn, "cursor".as_bytes())?;
            let cursor = CursorWrapper::create_glyph_cursor(
                &conn,
                font.font(),
                font.font(),
                68,
                69,
                0,
                0,
                0,
                u16::MAX,
                u16::MAX,
                u16::MAX,
            )?;

            // Actually become the WM by redirecting some operations
            conn.change_window_attributes(
                screen.root,
                &ChangeWindowAttributesAux::default()
                    .event_mask(
                        EventMask::SUBSTRUCTURE_REDIRECT
                            | EventMask::SUBSTRUCTURE_NOTIFY
                            | EventMask::PROPERTY_CHANGE
                            | EventMask::FOCUS_CHANGE,
                    )
                    // and also set a default root cursor in case downstream doesn't
                    .cursor(cursor.cursor()),
            )?;
        }

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
        debug!(window = win, "Created WM Window");
        conn.flush()?;

        let conn = Arc::new(conn);
        let source = X11Source::new(Arc::clone(&conn), win, atoms._SMITHAY_CLOSE_CONNECTION);

        let injector = X11Injector {
            xwm: id,
            handle: handle.clone(),
        };
        client
            .get_data::<XWaylandClientData>()
            .unwrap()
            .user_data()
            .insert_if_missing(move || injector);

        let _xfixes_data = conn
            .query_extension(x11rb::protocol::xfixes::X11_EXTENSION_NAME.as_bytes())?
            .reply_unchecked()?
            .ok_or(ConnectionError::UnsupportedExtension)?;
        if !_xfixes_data.present {
            return Err(ConnectionError::UnsupportedExtension.into());
        }
        conn.xfixes_query_version(1, 0)?.reply_unchecked()?; // we just need version 1 for clipboard monitoring

        let clipboard = XWmSelection::new(&conn, &screen, &atoms, atoms.CLIPBOARD)?;
        let primary = XWmSelection::new(&conn, &screen, &atoms, atoms.PRIMARY)?;

        drop(_guard);
        let wm = Self {
            id,
            dh,
            conn,
            screen,
            atoms,
            wm_window: win,
            wl_client: client,
            _xfixes_data,
            clipboard,
            primary,
            unpaired_surfaces: Default::default(),
            sequences_to_ignore: Default::default(),
            windows: Vec::new(),
            client_list: Vec::new(),
            client_list_stacking: Vec::new(),
            span,
        };

        let event_handle = handle.clone();
        handle.insert_source(source, move |event, _, data| {
            if let Err(err) = handle_event(&event_handle, data, id, event) {
                warn!(id = id.0, err = ?err, "Failed to handle X11 event");
            }
        })?;
        Ok(wm)
    }

    /// Id of this X11 WM
    pub fn id(&self) -> XwmId {
        self.id
    }

    /// Raises a window in the internal X11 state
    ///
    /// Needs to be called to match raising of windows inside the compositor to keep the stacking order
    /// in sync with the compositor to avoid erroneous behavior.
    pub fn raise_window<'a, W: X11Relatable + 'a>(&mut self, window: &'a W) -> Result<(), ConnectionError> {
        if let Some(elem) = self.windows.iter().find(|s| window.is_window(s)) {
            if self.client_list_stacking.last() == Some(&elem.window_id()) {
                return Ok(());
            }

            let _guard = scopeguard::guard((), |_| {
                let _ = self.conn.ungrab_server();
                let _ = self.conn.flush();
            });
            self.conn.grab_server()?;
            self.conn.configure_window(
                elem.mapped_window_id().unwrap_or_else(|| elem.window_id()),
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
            self.client_list_stacking.retain(|e| *e != elem.window_id());
            self.client_list_stacking.push(elem.window_id());
            self.conn.change_property32(
                PropMode::REPLACE,
                self.screen.root,
                self.atoms._NET_CLIENT_LIST_STACKING,
                AtomEnum::WINDOW,
                &self.client_list_stacking,
            )?;
        }
        Ok(())
    }

    fn update_stacking_order_impl<'a, W: X11Relatable + 'a>(
        &mut self,
        order: impl Iterator<Item = &'a W>,
        direction: StackingDirection,
    ) -> Result<(), ConnectionError> {
        let _guard = scopeguard::guard((), |_| {
            let _ = self.conn.ungrab_server();
            let _ = self.conn.flush();
        });
        self.conn.grab_server()?;

        let mut last_pos = None;
        let mut changed = false;

        for relatable in order {
            let stacking_ordered_elems: Vec<&X11Surface> = self
                .client_list_stacking
                .iter()
                .filter_map(|w| self.windows.iter().find(|s| s.window_id() == *w))
                .collect();
            let pos = stacking_ordered_elems.iter().position(|w| relatable.is_window(w));
            if let (Some(pos), Some(last_pos)) = (pos, last_pos) {
                if direction.pos_comparator(pos, last_pos) {
                    let sibling = stacking_ordered_elems[last_pos];
                    let sibling_id = self.client_list_stacking[last_pos];
                    let elem = stacking_ordered_elems[pos];
                    let elem_id = self.client_list_stacking.remove(pos);
                    self.conn.configure_window(
                        elem.mapped_window_id().unwrap_or(elem_id),
                        &ConfigureWindowAux::new()
                            .sibling(sibling.mapped_window_id().unwrap_or(sibling_id))
                            .stack_mode(direction.stack_mode()),
                    )?;
                    self.client_list_stacking.insert(last_pos, elem_id);
                    changed = true;
                    continue;
                }
            }

            if pos.is_some() {
                last_pos = pos;
            }
        }

        if changed {
            self.conn.change_property32(
                PropMode::REPLACE,
                self.screen.root,
                self.atoms._NET_CLIENT_LIST_STACKING,
                AtomEnum::WINDOW,
                &self.client_list_stacking,
            )?;
        }
        Ok(())
    }

    /// Updates the stacking order by matching provided windows downwards.
    ///
    /// This function reorders provided x11 windows in such a way,
    /// that windows inside the internal X11 stack follow the provided `order`
    /// without moving other windows around as much as possible. The internal stack
    /// stores windows bottom -> top, so order here is also bottom -> top.
    ///
    /// Window IDs unknown to this XWM will be ignored.
    /// The first window in `order` found will not be moved.
    ///
    /// If a window is encountered in `order` that is stacked below the first window,
    /// it will be moved to be directly above the previous window in `order`.
    /// E.g. Windows `C -> A -> B -> E` given in order with an internal stack of `D -> A -> B -> C`,
    /// will be reordered as `D -> C -> A -> B`. First `A` is moved to be directly
    /// above `C`, then `B`is moved to be directly above `A`.
    ///
    /// Windows in the internal stack, that are not present in `order`
    /// will be skipped over in the process.
    ///
    /// So if windows `A -> C` are given in order and the internal stack is `A -> B -> C`,
    /// no reordering will occur.
    ///
    /// See [`X11Wm::update_stacking_order_upwards`] for a variant of this algorithm,
    /// which works from the bottom up or [`X11Wm::raise_window`] for an easier but
    /// much more limited way to reorder.
    pub fn update_stacking_order_downwards<'a, W: X11Relatable + 'a>(
        &mut self,
        order: impl Iterator<Item = &'a W>,
    ) -> Result<(), ConnectionError> {
        self.update_stacking_order_impl(order, StackingDirection::Downwards)
    }

    /// Updates the stacking order by moving provided windows upwards.
    ///
    /// This function reorders provided x11 windows in such a way,
    /// that windows inside the internal X11 stack follow the provided `order`
    /// in reverse without moving other windows around as much as possible. The
    /// internal stack stores windows bottom -> top, so due to the reversal, order
    /// here is top -> bottom.
    ///
    /// Window IDs unknown to this XWM will be ignored.
    /// The first window in `order` found will not be moved.
    ///
    /// If a window is encountered in `order` that is stacked above the first window,
    /// it will be moved to be directly below the previous window in `order`.
    /// E.g. Windows C -> A -> B given in order with an internal stack of `D -> A -> B -> C`,
    /// will be reordered as `D -> B -> A -> C`. `A` is below `C`, so it isn't moved,
    /// then `B` is moved to be directly below `A`.
    ///
    /// Windows in the internal stack, that are not present in `order`
    /// will be skipped over in the process.
    ///
    /// So if windows `A -> C` are given in order and the internal stack is `C -> B -> A`,
    /// no reordering will occur.
    ///  
    /// See [`X11Wm::update_stacking_order_downwards`] for a variant of this algorithm,
    /// which works from the top down or [`X11Wm::raise_window`] for an easier but
    /// much more limited way to reorder.
    pub fn update_stacking_order_upwards<'a, W: X11Relatable + 'a>(
        &mut self,
        order: impl Iterator<Item = &'a W>,
    ) -> Result<(), ConnectionError> {
        self.update_stacking_order_impl(order, StackingDirection::Upwards)
    }

    /// This function has to be called on [`CompositorHandler::commit`](crate::wayland::compositor::CompositorHandler::commit) to correctly
    /// update the internal state of Xwayland WMs.
    pub fn commit_hook<D: XwmHandler + 'static>(surface: &WlSurface) {
        if let Some(client) = surface.client() {
            if let Some(x11) = client
                .get_data::<XWaylandClientData>()
                .and_then(|data| data.user_data().get::<X11Injector<D>>())
            {
                if get_role(surface).is_none() {
                    x11.late_window(surface);
                }
            }
        }
    }

    fn new_surface<D: XwmHandler>(state: &mut D, xwm_id: XwmId, surface: X11Surface, wl_surface: WlSurface) {
        info!(
            window_id = surface.window_id(),
            surface = ?wl_surface,
            "Matched X11 surface to wayland surface",
        );
        if give_role(&wl_surface, X11_SURFACE_ROLE).is_err() {
            // It makes no sense to post a protocol error here since that would only kill Xwayland
            error!(surface = ?wl_surface, "Surface already has a role?!");
            return;
        }

        surface.state.lock().unwrap().wl_surface = Some(wl_surface);
        state.map_window_notify(xwm_id, surface);
    }

    /// Set the default cursor used by X clients.
    ///
    /// `pixels` is expected to be in `rgba`-format with each channel encoded as an u8.
    ///
    /// This function will panic, if `pixels` is not at least `size.w * size.h * 4` long.
    pub fn set_cursor(
        &mut self,
        pixels: &[u8],
        size: Size<u16, Logical>,
        hotspot: Point<u16, Logical>,
    ) -> Result<(), ReplyOrIdError> {
        assert!(pixels.len() >= size.w as usize * size.h as usize * 4usize);
        let pixmap = PixmapWrapper::create_pixmap(&*self.conn, 32, self.screen.root, size.w, size.h)?;
        let Some(render_format) = self
            .conn
            .render_query_pict_formats()?
            .reply_unchecked()?
            .unwrap_or_default()
            .formats
            .into_iter()
            .filter(|f| f.depth == 32)
            .map(|f| f.id)
            .next()
        else {
            return Err(ReplyOrIdError::ConnectionError(ConnectionError::UnknownError));
            // TODO proper error type
        };
        let picture = PictureWrapper::create_picture(
            &*self.conn,
            pixmap.pixmap(),
            render_format,
            &CreatePictureAux::new(),
        )?;
        {
            let gc = GcontextWrapper::create_gc(&*self.conn, pixmap.pixmap(), &CreateGCAux::new())?;
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                pixmap.pixmap(),
                gc.gcontext(),
                size.w,
                size.h,
                0,
                0,
                0,
                32,
                pixels,
            )?;
        }
        let cursor = self.conn.generate_id()?;
        self.conn
            .render_create_cursor(cursor, picture.picture(), hotspot.x, hotspot.y)?;
        self.conn
            .change_window_attributes(self.screen.root, &ChangeWindowAttributesAux::new().cursor(cursor))?;
        let _ = self.conn.free_cursor(cursor);
        Ok(())
    }

    /// Notify Xwayland of a new selection.
    ///
    /// `mime_types` being `None` indicate there is no active selection anymore.
    pub fn new_selection(
        &mut self,
        selection: SelectionTarget,
        mime_types: Option<Vec<String>>,
    ) -> Result<(), ReplyOrIdError> {
        let selection = match selection {
            SelectionTarget::Clipboard => &mut self.clipboard,
            SelectionTarget::Primary => &mut self.primary,
        };

        if let Some(mime_types) = mime_types {
            selection.mime_types = mime_types;
            self.conn
                .set_selection_owner(selection.window, selection.atom, x11rb::CURRENT_TIME)?;
        } else if selection.owner == selection.window {
            selection.mime_types = Vec::new();
            self.conn
                .set_selection_owner(x11rb::NONE, selection.atom, selection.timestamp)?;
        }

        Ok(())
    }

    /// Request to transfer the active `selection` for the provided `mime_type` to the provided file descriptor.
    pub fn send_selection<D>(
        &mut self,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        loop_handle: LoopHandle<'_, D>,
    ) -> Result<(), SelectionError>
    where
        D: XwmHandler + 'static,
    {
        let xwm_id = self.id();
        let selection = match selection {
            SelectionTarget::Clipboard => &mut self.clipboard,
            SelectionTarget::Primary => &mut self.primary,
        };

        info!(
            selection = ?selection.type_,
            ?mime_type,
            "Send request from XWayland",
        );

        let atom = match &*mime_type {
            "text/plain;charset=utf-8" => self.atoms.UTF8_STRING,
            "text/plain" => self.atoms.TEXT,
            x => {
                let prop = self
                    .conn
                    .get_property(true, selection.window, self.atoms.TARGETS, AtomEnum::ANY, 0, 4096)?
                    .reply_unchecked()?
                    .ok_or(SelectionError::UnableToDetermineAtom)?;
                if prop.type_ != AtomEnum::ATOM.into() {
                    return Err(SelectionError::UnableToDetermineAtom);
                }
                let values = prop.value32().ok_or(SelectionError::UnableToDetermineAtom)?;

                let Some(atom) = values
                    .filter_map(|atom| {
                        let cookie = self.conn.get_atom_name(atom).ok()?;
                        let reply = cookie.reply_unchecked().ok()?;
                        std::str::from_utf8(&reply?.name)
                            .ok()
                            .map(|name| (atom, name.to_string()))
                    })
                    .find_map(|(atom, name)| if name == x { Some(atom) } else { None })
                else {
                    return Err(SelectionError::UnableToDetermineAtom);
                };

                atom
            }
        };

        debug!("Mime-type {:?} / Atom {:?}", mime_type, atom);

        let incoming_window = self.conn.generate_id()?;
        self.conn.create_window(
            self.screen.root_depth,
            incoming_window,
            self.screen.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            self.screen.root_visual,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        self.conn.convert_selection(
            incoming_window,
            selection.atom,
            atom,
            self.atoms._WL_SELECTION,
            x11rb::CURRENT_TIME,
        )?;

        if let Err(err) = rustix::fs::fcntl_setfl(&fd, OFlags::WRONLY | OFlags::NONBLOCK) {
            warn!(?err, "Failed to restrict wl file descriptor");
        }

        let selection_type = selection.type_;
        let loop_handle_clone = loop_handle.clone();
        let token = loop_handle
            .insert_source(
                Generic::new(fd, Interest::WRITE, Mode::Level),
                move |_, fd, data| {
                    let xwm = data.xwm_state(xwm_id);
                    let conn = &xwm.conn;
                    let atoms = &xwm.atoms;
                    let selection = match selection_type {
                        SelectionTarget::Clipboard => &mut xwm.clipboard,
                        SelectionTarget::Primary => &mut xwm.primary,
                    };
                    if let Some(transfer) = selection
                        .incoming
                        .iter_mut()
                        .find(|t| t.window == incoming_window)
                    {
                        match write_selection_callback(fd.as_fd(), conn, atoms, transfer) {
                            Ok(IncomingAction::WaitForWritable) => return Ok(PostAction::Continue),
                            Ok(IncomingAction::WaitForProperty) if !transfer.incr_done => {
                                return Ok(PostAction::Disable)
                            }
                            Ok(_) | Err(_) => {
                                if let Some(pos) = selection
                                    .incoming
                                    .iter()
                                    .position(|t| t.window == incoming_window)
                                {
                                    selection.incoming.remove(pos).destroy(&loop_handle_clone);
                                }
                            }
                        };
                    }
                    Ok(PostAction::Remove)
                },
            )
            .map_err(|err| err.error)?;
        loop_handle.disable(&token)?;

        let transfer = IncomingTransfer {
            conn: self.conn.clone(),
            token: Some(token),
            window: incoming_window,
            incr: false,
            source_data: Vec::new(),
            incr_done: false,
        };
        selection.incoming.push(transfer);

        self.conn.flush()?;
        Ok(())
    }
}

fn handle_event<D: XwmHandler + 'static>(
    loop_handle: &LoopHandle<'_, D>,
    state: &mut D,
    xwm_id: XwmId,
    event: Event,
) -> Result<(), ReplyOrIdError> {
    let xwm = state.xwm_state(xwm_id);
    let _guard = xwm.span.enter();
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

    debug!(
        event = ?event,
        should_ignore = should_ignore,
        "Got X11 event",
    );
    if should_ignore {
        return Ok(());
    }

    match event {
        Event::CreateNotify(n) => {
            if n.window == xwm.wm_window
                || n.window == xwm.clipboard.window
                || xwm.clipboard.incoming.iter().any(|i| n.window == i.window)
                || n.window == xwm.primary.window
                || xwm.primary.incoming.iter().any(|i| n.window == i.window)
            {
                return Ok(());
            }

            xwm.conn.change_window_attributes(
                n.window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?;
            xwm.conn.flush()?;

            if xwm.windows.iter().any(|s| s.mapped_window_id() == Some(n.window)) {
                return Ok(());
            }

            let geo = conn.get_geometry(n.window)?.reply()?;

            let surface = X11Surface::new(
                xwm_id,
                n.window,
                n.override_redirect,
                Arc::downgrade(&conn),
                xwm.atoms,
                Rectangle::from_loc_and_size(
                    (geo.x as i32, geo.y as i32),
                    (geo.width as i32, geo.height as i32),
                ),
            );
            surface.update_properties(None)?;
            xwm.windows.push(surface.clone());

            drop(_guard);
            if n.override_redirect {
                state.new_override_redirect_window(xwm_id, surface);
            } else {
                state.new_window(xwm_id, surface);
            }
        }
        Event::MapRequest(r) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == r.window).cloned() {
                if surface.state.lock().unwrap().mapped_onto.is_none() {
                    // we reparent windows, because a lot of stuff expects, that we do
                    let geo = conn.get_geometry(r.window)?.reply()?;
                    let win = r.window;
                    let frame_win = conn.generate_id()?;
                    let win_aux = CreateWindowAux::new().event_mask(
                        EventMask::SUBSTRUCTURE_NOTIFY
                            | EventMask::SUBSTRUCTURE_REDIRECT
                            | EventMask::PROPERTY_CHANGE,
                    );

                    {
                        let _guard = scopeguard::guard((), |_| {
                            let _ = conn.ungrab_server();
                        });

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

                        // Ignore all events caused by reparent_window(). All those events have the sequence number
                        // of the reparent_window() request, thus remember its sequence number. The
                        // grab_server()/ungrab_server() is done so that the server does not handle other clients
                        // in-between, which could cause other events to get the same sequence number.
                        xwm.sequences_to_ignore
                            .push(Reverse(cookie1.sequence_number() as u16));
                        xwm.sequences_to_ignore
                            .push(Reverse(cookie2.sequence_number() as u16));
                    }

                    surface.state.lock().unwrap().mapped_onto = Some(frame_win);
                    drop(_guard);
                    state.map_window_request(xwm_id, surface);
                }
            }
        }
        Event::MapNotify(n) => {
            trace!(window = ?n.window, "mapped X11 Window");
            if let Some(surface) = xwm
                .windows
                .iter()
                .find(|x|
                      // don't include the reparenting windows
                      (x.window_id() == n.window && n.override_redirect) ||
                      x.mapped_window_id() == Some(n.window))
                .cloned()
            {
                if surface.is_override_redirect() {
                    drop(_guard);
                    state.mapped_override_redirect_window(xwm_id, surface);
                } else {
                    xwm.client_list.push(surface.window_id());
                    xwm.client_list_stacking.push(surface.window_id());
                    conn.change_property32(
                        PropMode::APPEND,
                        xwm.screen.root,
                        xwm.atoms._NET_CLIENT_LIST,
                        AtomEnum::WINDOW,
                        &[surface.window_id()],
                    )?;
                    conn.change_property32(
                        PropMode::APPEND,
                        xwm.screen.root,
                        xwm.atoms._NET_CLIENT_LIST_STACKING,
                        AtomEnum::WINDOW,
                        &[surface.window_id()],
                    )?;
                }
            }
        }
        Event::ConfigureRequest(r) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == r.window).cloned() {
                drop(_guard);
                // Pass the request to downstream to decide
                state.configure_request(
                    xwm_id,
                    surface.clone(),
                    if u16::from(r.value_mask) & u16::from(ConfigWindow::X) != 0 {
                        Some(i32::from(r.x))
                    } else {
                        None
                    },
                    if u16::from(r.value_mask) & u16::from(ConfigWindow::Y) != 0 {
                        Some(i32::from(r.y))
                    } else {
                        None
                    },
                    if u16::from(r.value_mask) & u16::from(ConfigWindow::WIDTH) != 0 {
                        Some(u32::from(r.width))
                    } else {
                        None
                    },
                    if u16::from(r.value_mask) & u16::from(ConfigWindow::HEIGHT) != 0 {
                        Some(u32::from(r.height))
                    } else {
                        None
                    },
                    if u16::from(r.value_mask) & u16::from(ConfigWindow::STACK_MODE) != 0 {
                        match r.stack_mode {
                            StackMode::ABOVE => {
                                if u16::from(r.value_mask) & u16::from(ConfigWindow::SIBLING) != 0 {
                                    Some(Reorder::Above(r.sibling))
                                } else {
                                    Some(Reorder::Top)
                                }
                            }
                            StackMode::BELOW => {
                                if u16::from(r.value_mask) & u16::from(ConfigWindow::SIBLING) != 0 {
                                    Some(Reorder::Below(r.sibling))
                                } else {
                                    Some(Reorder::Bottom)
                                }
                            }
                            _ => None,
                        }
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
            trace!(window = ?n, "configured X11 Window");
            if let Some(surface) = xwm
                .windows
                .iter()
                .find(|x| x.mapped_window_id() == Some(n.window))
                .cloned()
            {
                drop(_guard);
                state.configure_notify(
                    xwm_id,
                    surface,
                    Rectangle::from_loc_and_size((n.x as i32, n.y as i32), (n.width as i32, n.height as i32)),
                    if n.above_sibling == x11rb::NONE {
                        None
                    } else {
                        Some(n.above_sibling)
                    },
                );
            } else if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == n.window).cloned() {
                if surface.is_override_redirect() {
                    let geometry = Rectangle::from_loc_and_size(
                        (n.x as i32, n.y as i32),
                        (n.width as i32, n.height as i32),
                    );
                    surface.state.lock().unwrap().geometry = geometry;
                    drop(_guard);
                    state.configure_notify(
                        xwm_id,
                        surface,
                        geometry,
                        if n.above_sibling == x11rb::NONE {
                            None
                        } else {
                            Some(n.above_sibling)
                        },
                    );
                }
            }
        }
        Event::UnmapNotify(n) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == n.window).cloned() {
                xwm.client_list.retain(|w| *w != surface.window_id());
                xwm.client_list_stacking.retain(|w| *w != surface.window_id());
                {
                    let _guard = scopeguard::guard((), |_| {
                        let _ = conn.ungrab_server();
                    });
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
                            state.geometry.loc.x as i16,
                            state.geometry.loc.y as i16,
                        )?;
                        if let Some(frame) = state.mapped_onto.take() {
                            conn.destroy_window(frame)?;
                        }
                    }
                }
                drop(_guard);
                state.unmapped_window(xwm_id, surface.clone());
                {
                    let mut state = surface.state.lock().unwrap();
                    state.wl_surface = None;
                }
            }
        }
        Event::DestroyNotify(n) => {
            if let Some(selection) = if xwm.clipboard.incoming.iter().any(|t| t.window == n.window)
                || xwm
                    .clipboard
                    .outgoing
                    .iter()
                    .any(|t| t.request.requestor == n.window)
            {
                Some(&mut xwm.clipboard)
            } else if xwm.primary.incoming.iter().any(|t| t.window == n.window)
                || xwm
                    .primary
                    .outgoing
                    .iter()
                    .any(|t| t.request.requestor == n.window)
            {
                Some(&mut xwm.primary)
            } else {
                None
            } {
                // TODO: drain_filter
                let mut i = 0;
                while i < selection.incoming.len() {
                    if selection.incoming[i].window == n.window {
                        selection.incoming.remove(i).destroy(loop_handle);
                    } else {
                        i += 1;
                    }
                }
                let mut i = 0;
                while i < selection.outgoing.len() {
                    if selection.outgoing[i].request.requestor == n.window {
                        selection.outgoing.remove(i).destroy(loop_handle);
                    } else {
                        i += 1;
                    }
                }
            }

            if let Some(pos) = xwm.windows.iter().position(|x| x.window_id() == n.window) {
                let surface = xwm.windows.remove(pos);
                surface.state.lock().unwrap().alive = false;
                drop(_guard);
                state.destroyed_window(xwm_id, surface);
            }
        }
        Event::XfixesSelectionNotify(n) => {
            let selection = match n.selection {
                x if x == xwm.atoms.CLIPBOARD => &mut xwm.clipboard,
                x if x == xwm.atoms.PRIMARY => &mut xwm.primary,
                _ => return Ok(()),
            };

            selection.owner = n.owner;
            if selection.owner == selection.window {
                selection.timestamp = n.timestamp;
                return Ok(());
            }

            if n.owner == x11rb::NONE && selection.owner != selection.window {
                // A real X clients selection went away, not our proxy
                let selection = selection.type_;
                drop(_guard);
                state.cleared_selection(xwm_id, selection);
                return Ok(());
            }

            // Actually query the new selection, which will give us a SelectioNotify event
            conn.convert_selection(
                selection.window,
                selection.atom,
                xwm.atoms.TARGETS,
                xwm.atoms._WL_SELECTION,
                n.timestamp,
            )?;
        }
        Event::SelectionNotify(n) => {
            let selection = match n.selection {
                x if x == xwm.atoms.CLIPBOARD => &mut xwm.clipboard,
                x if x == xwm.atoms.PRIMARY => &mut xwm.primary,
                _ => return Ok(()),
            };

            match n.target {
                x if x == xwm.atoms.TARGETS => {
                    if let Some(prop) = conn
                        .get_property(
                            true,
                            selection.window,
                            xwm.atoms._WL_SELECTION,
                            AtomEnum::ANY,
                            0,
                            4096,
                        )?
                        .reply_unchecked()?
                    {
                        if prop.type_ == AtomEnum::ATOM.into() {
                            if let Some(values) = prop.value32() {
                                let mime_types = values
                                    .filter_map(|val| {
                                        match val {
                                            val if val == xwm.atoms.UTF8_STRING => {
                                                Some(Ok(String::from("text/plain;charset=utf-8")))
                                            }
                                            val if val == xwm.atoms.TEXT => {
                                                Some(Ok(String::from("text/plain")))
                                            }
                                            val if val == xwm.atoms.TARGETS || val == xwm.atoms.TIMESTAMP => {
                                                None
                                            }
                                            val => {
                                                let cookie = match conn.get_atom_name(val) {
                                                    Ok(cookie) => cookie,
                                                    Err(err) => return Some(Err(err)),
                                                };
                                                let reply = match cookie.reply_unchecked() {
                                                    Ok(reply) => reply,
                                                    Err(err) => return Some(Err(err)),
                                                };
                                                if let Some(reply) = reply {
                                                    if let Ok(name) = std::str::from_utf8(&reply.name) {
                                                        if name.contains('/') {
                                                            // hopefully a mime-type
                                                            Some(Ok(String::from(name)))
                                                        } else {
                                                            None
                                                        }
                                                    } else {
                                                        None
                                                    }
                                                } else {
                                                    None
                                                }
                                            }
                                        }
                                    })
                                    .collect::<Result<Vec<String>, _>>()?;

                                let selection = selection.type_;
                                drop(_guard);
                                state.new_selection(xwm_id, selection, mime_types);
                            }
                        }
                    }
                }
                x if x == AtomEnum::NONE.into() => {
                    // transfer failed
                    if let Some(pos) = selection.incoming.iter().position(|t| t.window == n.requestor) {
                        selection.incoming.remove(pos).destroy(loop_handle);
                    }
                }
                _ => {
                    let Some(transfer) = selection.incoming.iter_mut().find(|t| t.window == n.requestor)
                    else {
                        return Ok(());
                    };

                    if let Some(prop) = conn
                        .get_property(
                            true,
                            transfer.window,
                            xwm.atoms._WL_SELECTION,
                            AtomEnum::ANY,
                            0,
                            0x1fffffff,
                        )?
                        .reply_unchecked()?
                    {
                        let type_ = prop.type_;
                        transfer.read_selection_prop(prop);
                        if type_ == xwm.atoms.INCR {
                            transfer.incr = true;
                            return Ok(());
                        } else if let Some(token) = transfer.token.as_ref() {
                            let _ = loop_handle.enable(token);
                        } else if let Some(pos) =
                            selection.incoming.iter().position(|t| t.window == n.requestor)
                        {
                            selection.incoming.remove(pos);
                        }
                    }
                }
            }
        }
        Event::SelectionRequest(n) => {
            let selection_type = match n.selection {
                x if x == xwm.atoms.CLIPBOARD => xwm.clipboard.type_,
                x if x == xwm.atoms.PRIMARY => xwm.primary.type_,
                _ => {
                    warn!(
                        target = ?n.selection,
                        "Got SelectionRequest for unknown Target",
                    );
                    send_selection_notify_resp(&conn, &n, false)?;
                    return Ok(());
                }
            };

            // work around borrowing
            drop(_guard);
            let allow_access = state.allow_selection_access(xwm_id, selection_type);
            let xwm = state.xwm_state(xwm_id);
            let selection = match selection_type {
                SelectionTarget::Clipboard => &mut xwm.clipboard,
                SelectionTarget::Primary => &mut xwm.primary,
            };

            let _guard = xwm.span.enter();
            if n.requestor == selection.window {
                warn!("Got SelectionRequest from our own selection window.");
                send_selection_notify_resp(&conn, &n, false)?;
                return Ok(());
            }

            if selection.window != n.owner {
                if n.time != x11rb::CURRENT_TIME && n.time < selection.timestamp {
                    warn!(
                        got = n.time,
                        expected = selection.timestamp,
                        "Ignoring request with too old timestamp",
                    );
                    send_selection_notify_resp(&conn, &n, false)?;
                }

                // dont fail requests, when we are not the owner anymore
                return Ok(());
            }

            if allow_access {
                match n.target {
                    x if x == xwm.atoms.TARGETS => {
                        if selection.mime_types.is_empty() {
                            send_selection_notify_resp(&conn, &n, false)?;
                            return Ok(());
                        }

                        let targets = [xwm.atoms.TARGETS, xwm.atoms.TIMESTAMP]
                            .iter()
                            .copied()
                            .chain(selection.mime_types.iter().filter_map(|mime| {
                                Some(match &**mime {
                                    "text/plain" => xwm.atoms.TEXT,
                                    "text/plain;charset=utf-8" => xwm.atoms.UTF8_STRING,
                                    mime => {
                                        conn.intern_atom(false, mime.as_bytes())
                                            .ok()?
                                            .reply_unchecked()
                                            .ok()??
                                            .atom
                                    }
                                })
                            }))
                            .collect::<Vec<u32>>();
                        trace!(requstor = n.requestor, ?targets, "Sending TARGETS");
                        conn.change_property32(
                            PropMode::REPLACE,
                            n.requestor,
                            n.property,
                            AtomEnum::ATOM,
                            &targets,
                        )?;
                        send_selection_notify_resp(&conn, &n, true)?;
                    }
                    x if x == xwm.atoms.TIMESTAMP => {
                        trace!(
                            requestor = n.requestor,
                            timestamp = selection.timestamp,
                            "Sending TIMESTAMP",
                        );
                        conn.change_property32(
                            PropMode::REPLACE,
                            n.requestor,
                            n.property,
                            AtomEnum::INTEGER,
                            &[selection.timestamp],
                        )?;
                        send_selection_notify_resp(&conn, &n, true)?;
                    }
                    x if x == xwm.atoms.DELETE => {
                        send_selection_notify_resp(&conn, &n, true)?;
                    }
                    target => {
                        let mime_type = match target {
                            x if x == xwm.atoms.TEXT => "text/plain".to_string(),
                            x if x == xwm.atoms.UTF8_STRING => "text/plain;charset=utf-8".to_string(),
                            x => {
                                let Some(mime) = conn
                                    .get_atom_name(x)?
                                    .reply_unchecked()?
                                    .and_then(|reply| String::from_utf8(reply.name).ok())
                                else {
                                    debug!("Unable to determine mime type from atom: {}", x);
                                    send_selection_notify_resp(&conn, &n, false)?;
                                    return Ok(());
                                };

                                if !selection.mime_types.contains(&mime) {
                                    warn!(mime, "Mime type requested by X client not offered",);
                                    send_selection_notify_resp(&conn, &n, false)?;
                                    return Ok(());
                                }

                                mime
                            }
                        };

                        let (recv_fd, send_fd) = rustix::pipe::pipe_with(
                            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
                        )
                        .map_err(|err| ConnectionError::IoError(std::io::Error::from(err)))?;

                        // It seems that if we ever try to reply to a selection request after
                        // another has been sent by the same requestor, the requestor never reads
                        // from it. It appears to only ever read from the latest, so purge stale
                        // transfers to prevent clipboard hangs.

                        // TODO: Drain filter
                        let mut i = 0;
                        while i < selection.outgoing.len() {
                            let transfer = &mut selection.outgoing[i];
                            if transfer.request.requestor == n.requestor {
                                debug!(
                                    requestor = transfer.request.requestor,
                                    "Destroying stale transfer",
                                );
                                send_selection_notify_resp(&transfer.conn, &transfer.request, false)?;
                                selection.outgoing.remove(i).destroy(loop_handle);
                            } else {
                                i += 1;
                            }
                        }

                        let requestor = n.requestor;

                        let token = loop_handle.insert_source(
                            Generic::new(recv_fd, Interest::READ, Mode::Level),
                            move |_, fd, data| {
                                let xwm = data.xwm_state(xwm_id);
                                let selection = match selection_type {
                                    SelectionTarget::Clipboard => &mut xwm.clipboard,
                                    SelectionTarget::Primary => &mut xwm.primary,
                                };

                                if let Some(transfer) = selection
                                    .outgoing
                                    .iter_mut()
                                    .find(|t| t.request.requestor == requestor)
                                {
                                    match read_selection_callback(&xwm.conn, &xwm.atoms, fd.as_fd(), transfer)
                                    {
                                        Ok(OutgoingAction::WaitForReadable) => {
                                            return Ok(PostAction::Continue);
                                        } // transfer ongoing
                                        Ok(_) => {}
                                        Err(err) => {
                                            warn!(?err, "Transfer aborted");
                                        }
                                    };
                                    let _ = transfer.token.take();
                                }

                                Ok(PostAction::Remove)
                            },
                        );

                        let token = match token {
                            Ok(token) => token,
                            Err(err) => {
                                warn!(
                                    err = ?err.error,
                                    "Failed to initialize event loop source for clipboard transfer",

                                );
                                send_selection_notify_resp(&conn, &n, false)?;
                                return Ok(());
                            }
                        };

                        debug!(
                            selection = ?selection.type_,
                            requestor = n.requestor,
                            ?mime_type,
                            "Created outgoing transfer",
                        );
                        let transfer = OutgoingTransfer {
                            conn: conn.clone(),
                            incr: false,
                            token: Some(token),
                            source_data: Vec::new(),
                            request: n,
                            property_set: false,
                            flush_property_on_delete: false,
                        };
                        selection.outgoing.push(transfer);

                        let selection_type = selection.type_;
                        drop(_guard);
                        state.send_selection(xwm_id, selection_type, mime_type, send_fd);
                    }
                }
            } else {
                // selection was denied
                send_selection_notify_resp(&conn, &n, false)?;
            }
        }
        Event::PropertyNotify(n) => {
            if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == n.window) {
                surface.update_properties(Some(n.atom))?;
            }

            if n.state == Property::NEW_VALUE && n.atom == xwm.atoms._WL_SELECTION {
                if let Some(selection) = if xwm.clipboard.incoming.iter().any(|t| t.window == n.window) {
                    Some(&mut xwm.clipboard)
                } else if xwm.primary.incoming.iter().any(|t| t.window == n.window) {
                    Some(&mut xwm.primary)
                } else {
                    None
                } {
                    let transfer = selection
                        .incoming
                        .iter_mut()
                        .find(|t| t.window == n.window)
                        .unwrap();
                    if transfer.incr {
                        if let Some(prop) = conn
                            .get_property(
                                true,
                                transfer.window,
                                xwm.atoms._WL_SELECTION,
                                AtomEnum::ANY,
                                0,
                                0x1fffffff,
                            )?
                            .reply_unchecked()?
                        {
                            if prop.value_len == 0 {
                                debug!(?transfer, "Incr Transfer complete!");
                                if transfer.source_data.is_empty() {
                                    if let Some(pos) =
                                        selection.incoming.iter().position(|t| t.window == n.window)
                                    {
                                        selection.incoming.remove(pos).destroy(loop_handle);
                                    }
                                } else {
                                    transfer.incr_done = true;
                                }
                            } else {
                                transfer.read_selection_prop(prop);
                                if let Some(token) = transfer.token.as_ref() {
                                    let _ = loop_handle.enable(token);
                                } else if let Some(pos) =
                                    selection.incoming.iter().position(|t| t.window == n.window)
                                {
                                    selection.incoming.remove(pos);
                                }
                            }
                        }
                    }
                }
            }

            if n.state == Property::DELETE {
                if let Some(selection) = if xwm
                    .clipboard
                    .outgoing
                    .iter()
                    .any(|t| t.incr && t.request.requestor == n.window && t.request.property == n.atom)
                {
                    Some(&mut xwm.clipboard)
                } else if xwm
                    .primary
                    .outgoing
                    .iter()
                    .any(|t| t.incr && t.request.requestor == n.window && t.request.property == n.atom)
                {
                    Some(&mut xwm.primary)
                } else {
                    None
                } {
                    let transfer = selection
                        .outgoing
                        .iter_mut()
                        .find(|t| t.incr && t.request.requestor == n.window && t.request.property == n.atom)
                        .unwrap();

                    transfer.property_set = false;
                    if transfer.flush_property_on_delete {
                        transfer.flush_property_on_delete = false;
                        let len = transfer.flush_data()?;
                        let requestor = transfer.request.requestor;
                        trace!(requestor, len, "Send data chunk");

                        if transfer.token.is_none() {
                            if len > 0 {
                                // Transfer is done, but we still have bytes left
                                transfer.flush_property_on_delete = true;
                            } else if let Some(pos) = selection
                                .outgoing
                                .iter()
                                .position(|t| t.request.requestor == requestor)
                            {
                                // done
                                selection.outgoing.remove(pos);
                            }
                        }
                    }
                }
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
            if let Some(reply) = conn.get_atom_name(msg.type_)?.reply_unchecked()? {
                debug!(
                    event = std::str::from_utf8(&reply.name).unwrap(),
                    message = ?msg,
                    "got X11 client event message",
                );
            }
            match msg.type_ {
                x if x == xwm.atoms.WL_SURFACE_ID => {
                    let wid = msg.data.as_data32()[0];
                    info!(
                        window = ?msg.window,
                        surface = ?wid,
                        "mapped X11 window to surface",
                    );
                    if let Some(surface) = xwm
                        .windows
                        .iter_mut()
                        .find(|x| x.window_id() == msg.window || x.mapped_window_id() == Some(msg.window))
                    {
                        // We get a WL_SURFACE_ID message when Xwayland creates a WlSurface for a
                        // window. Both the creation of the surface and this client message happen at
                        // roughly the same time and are sent over different sockets (X11 socket and
                        // wayland socket). Thus, we could receive these two in any order. Hence, it
                        // can happen that we get None below when X11 was faster than Wayland.

                        let wl_surface = xwm.wl_client.object_from_protocol_id::<WlSurface>(&xwm.dh, wid);
                        match wl_surface {
                            Err(_) => {
                                xwm.unpaired_surfaces.insert(wid, msg.window);
                            }
                            Ok(wl_surface) => {
                                let surface = surface.clone();
                                drop(_guard);
                                X11Wm::new_surface(state, xwm_id, surface, wl_surface);
                            }
                        }
                    }
                }
                x if x == xwm.atoms.WM_CHANGE_STATE => {
                    let data = msg.data.as_data32();
                    if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == msg.window).cloned() {
                        drop(_guard);
                        match data[0] {
                            1 => state.unminimize_request(xwm_id, surface),
                            3 => state.minimize_request(xwm_id, surface),
                            _ => {}
                        }
                    }
                }
                x if x == xwm.atoms._NET_WM_STATE => {
                    let data = msg.data.as_data32();
                    debug!(
                        "X11: Got _NET_WM_STATE change request to ({:?}): {:?} / {:?}",
                        match &data[0] {
                            0 => "REMOVE",
                            1 => "SET",
                            2 => "TOGGLE",
                            _ => "Unknown",
                        },
                        conn.get_atom_name(data[1])?
                            .reply_unchecked()?
                            .map(|reply| String::from_utf8(reply.name)),
                        conn.get_atom_name(data[2])?
                            .reply_unchecked()?
                            .map(|reply| String::from_utf8(reply.name)),
                    );
                    if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == msg.window).cloned() {
                        drop(_guard);
                        match &data[1..=2] {
                            &[x, y]
                                if (x == xwm.atoms._NET_WM_STATE_MAXIMIZED_HORZ
                                    && y == xwm.atoms._NET_WM_STATE_MAXIMIZED_VERT)
                                    || (x == xwm.atoms._NET_WM_STATE_MAXIMIZED_VERT
                                        && y == xwm.atoms._NET_WM_STATE_MAXIMIZED_HORZ) =>
                            {
                                match data[0] {
                                    0 => {
                                        if surface.is_maximized() {
                                            state.unmaximize_request(xwm_id, surface)
                                        }
                                    }
                                    1 => {
                                        if !surface.is_maximized() {
                                            state.maximize_request(xwm_id, surface)
                                        }
                                    }
                                    2 => {
                                        if surface.is_maximized() {
                                            state.unmaximize_request(xwm_id, surface)
                                        } else {
                                            state.maximize_request(xwm_id, surface)
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            actions if actions.contains(&xwm.atoms._NET_WM_STATE_FULLSCREEN) => {
                                match data[0] {
                                    0 => state.unfullscreen_request(xwm_id, surface),
                                    1 => state.fullscreen_request(xwm_id, surface),
                                    2 => {
                                        if surface.is_fullscreen() {
                                            state.unfullscreen_request(xwm_id, surface)
                                        } else {
                                            state.fullscreen_request(xwm_id, surface)
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
                    if let Some(surface) = xwm.windows.iter().find(|x| x.window_id() == msg.window).cloned() {
                        drop(_guard);
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
                                state.resize_request(xwm_id, surface, data[3], resize_edge);
                            }
                            8 => state.move_request(xwm_id, surface, data[3]),
                            _ => {} // ignore keyboard moves/resizes for now
                        }
                    }
                }
                x => {
                    debug!(
                        "Unhandled client msg of type {:?}",
                        String::from_utf8(conn.get_atom_name(x)?.reply_unchecked()?.unwrap().name).ok()
                    )
                }
            }
        }
        Event::Error(err) => {
            info!(?err, "Got X11 Error");
        }
        _ => {}
    }
    conn.flush()?;
    Ok(())
}

enum OutgoingAction {
    Done,
    DoneReading,
    WaitForReadable,
}

fn read_selection_callback(
    conn: &RustConnection,
    atoms: &Atoms,
    fd: BorrowedFd<'_>,
    transfer: &mut OutgoingTransfer,
) -> Result<OutgoingAction, ReplyOrIdError> {
    let mut buf = [0; INCR_CHUNK_SIZE];
    let Ok(len) = rustix::io::read(fd, &mut buf) else {
        debug!(
            requestor = transfer.request.requestor,
            "File descriptor closed, aborting transfer."
        );
        send_selection_notify_resp(conn, &transfer.request, false)?;
        return Ok(OutgoingAction::Done);
    };
    trace!(
        requestor = transfer.request.requestor,
        "Transfer became readable, read {} bytes",
        len
    );

    transfer.source_data.extend_from_slice(&buf[..len]);
    if transfer.source_data.len() >= INCR_CHUNK_SIZE {
        if !transfer.incr {
            // start incr transfer
            trace!(
                requestor = transfer.request.requestor,
                "Transfer became incremental",
            );
            conn.change_property32(
                PropMode::REPLACE,
                transfer.request.requestor,
                transfer.request.property,
                atoms.INCR,
                &[INCR_CHUNK_SIZE as u32],
            )?;
            conn.flush()?;
            transfer.incr = true;
            transfer.property_set = true;
            transfer.flush_property_on_delete = true;
            send_selection_notify_resp(conn, &transfer.request, true)?;
        } else if transfer.property_set {
            // got more bytes, waiting for property delete
            transfer.flush_property_on_delete = true;
        } else {
            // got more bytes, property deleted
            let len = transfer.flush_data()?;
            trace!(
                requestor = transfer.request.requestor,
                "Send data chunk: {} bytes",
                len
            );
        }
    }

    if len == 0 {
        if transfer.incr {
            debug!("Incr transfer completed");
            if !transfer.property_set {
                let len = transfer.flush_data()?;
                trace!(
                    requestor = transfer.request.requestor,
                    "Send data chunk: {} bytes",
                    len
                );
            }
            transfer.flush_property_on_delete = true;
            Ok(OutgoingAction::DoneReading)
        } else {
            let len = transfer.flush_data()?;
            debug!("Non-Incr transfer completed with {} bytes", len);
            send_selection_notify_resp(conn, &transfer.request, true)?;
            Ok(OutgoingAction::Done)
        }
    } else {
        Ok(OutgoingAction::WaitForReadable)
    } // nothing to be done, buffered the bytes
}

enum IncomingAction {
    Done,
    WaitForProperty,
    WaitForWritable,
}

fn write_selection_callback(
    fd: BorrowedFd<'_>,
    conn: &RustConnection,
    atoms: &Atoms,
    transfer: &mut IncomingTransfer,
) -> Result<IncomingAction, ReplyOrIdError> {
    match transfer.write_selection(fd) {
        Ok(true) => {
            if transfer.incr {
                conn.delete_property(transfer.window, atoms._WL_SELECTION)?;
                Ok(IncomingAction::WaitForProperty)
            } else {
                debug!(?transfer, "Non-Incr Transfer complete!");
                Ok(IncomingAction::Done)
            }
        }
        Ok(false) => Ok(IncomingAction::WaitForWritable),
        Err(err) => {
            warn!(?err, "Transfer errored");
            if transfer.incr {
                // even if it failed, we still need to drain the incr transfer
                conn.delete_property(transfer.window, atoms._WL_SELECTION)?;
            }
            Ok(IncomingAction::Done)
        }
    }
}

fn send_selection_notify_resp(
    conn: &RustConnection,
    req: &SelectionRequestEvent,
    success: bool,
) -> Result<(), ReplyOrIdError> {
    conn.send_event(
        false,
        req.requestor,
        EventMask::NO_EVENT,
        SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property: if success {
                req.property
            } else {
                AtomEnum::NONE.into()
            },
        },
    )?;
    conn.flush()?;
    Ok(())
}

fn send_configure_notify(
    conn: &RustConnection,
    win: &X11Window,
    geometry: Rectangle<i32, Logical>,
    override_redirect: bool,
) -> Result<(), ConnectionError> {
    conn.send_event(
        false,
        *win,
        EventMask::STRUCTURE_NOTIFY,
        ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: *win,
            window: *win,
            above_sibling: x11rb::NONE,
            x: geometry.loc.x as i16,
            y: geometry.loc.y as i16,
            width: geometry.size.w as u16,
            height: geometry.size.h as u16,
            border_width: 0,
            override_redirect,
        },
    )?;
    conn.flush()?;
    Ok(())
}
