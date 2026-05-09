use crate::{
    backend::input::KeyState,
    input::{
        Seat, SeatHandler,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
        },
        touch::TouchTarget,
    },
    utils::{Client, HookId, IsAlive, Logical, Physical, Rectangle, Serial, Size, user_data::UserDataMap},
    wayland::{
        compositor::{self, RectangleKind, RegionAttributes, SurfaceAttributes},
        seat::{WaylandFocus, keyboard::enter_internal},
    },
};
#[cfg(feature = "desktop")]
use crate::{
    desktop::{WindowSurfaceType, utils::under_from_surface_tree},
    utils::Point,
};

use atomic_float::AtomicF64;
use encoding_rs::WINDOWS_1252;
use std::{
    borrow::Cow,
    collections::HashSet,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tracing::warn;
use wayland_server::protocol::wl_surface::WlSurface;
use xkbcommon::xkb::Keycode;

use x11rb::{
    connection::Connection as _,
    errors::{ReplyError, ReplyOrIdError},
    properties::{WmClass, WmHints, WmSizeHints},
    protocol::{
        res::{ClientIdSpec, query_client_ids},
        sync::{Alarm, ConnectionExt as _, Counter, CreateAlarmAux, Int64, TESTTYPE, VALUETYPE},
        xproto::{
            Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
            InputFocus, PropMode, Window as X11Window,
        },
    },
    rust_connection::{ConnectionError, RustConnection},
    wrapper::ConnectionExt,
    x11_utils::X11Error,
};

use super::{X11Wm, XwmId, send_configure_notify};

/// X11 window managed by an [`X11Wm`](super::X11Wm)
#[derive(Debug, Clone)]
pub struct X11Surface {
    xwm: Option<XwmId>,
    client_scale: Option<Arc<AtomicF64>>,
    window: X11Window,
    focus_release: Option<super::FocusReleaseHandle>,
    pub(super) conn: Weak<RustConnection>,
    pub(super) atoms: super::Atoms,
    servertime_counter: Option<Counter>,
    pub(crate) state: Arc<Mutex<SharedSurfaceState>>,
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))]
    pub(super) xdnd_active: Arc<AtomicBool>,
    user_data: Arc<UserDataMap>,
}

/// Possible errors when calling [X11Surface::send_ping]
#[derive(Debug, thiserror::Error)]
pub enum PingError {
    /// Not supported
    #[error("Ping protocol is not supported for this window")]
    NotSupported,
    /// Invalid timestamp (must not be `0`/`CURRENT_TIME`)
    #[error("Invalid timestamp provided")]
    InvalidTimestamp,
    /// Another ping is already in flight.
    #[error("Ping with timestamp {0} is still in flight")]
    PingAlreadyPending(u32),
    /// Error on the underlying X11 Connection
    #[error(transparent)]
    Connection(#[from] ConnectionError),
}

const MWM_HINTS_FLAGS_FIELD: usize = 0;
const MWM_HINTS_DECORATIONS_FIELD: usize = 2;
const MWM_HINTS_DECORATIONS: u32 = 1 << 1;

const DEFAULT_SYNC_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
// From http://fishsoup.net/misc/wm-spec-synchronization.html
//   "If the client is continually redrawing, then the last seen value may be out of date when the
//   window manager sends the message. Picking a number that is 240 later would allow for 1 second
//   of frames at 60fps."
const SYNC_REQUEST_INCREMENT: i64 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncRequestCounter {
    Basic(Counter),
    Extended(Counter),
}

#[derive(Debug, thiserror::Error)]
enum SyncRequestError {
    #[error("sync request protocol not supported")]
    NotSupported,
    #[error("sync request already in flight")]
    RequestPending,
    #[error("no more XIDs")]
    IdsExhausted,
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error("X11 protocol error: {0:?}")]
    X11(X11Error),
}

#[derive(Debug)]
pub(crate) struct SharedSurfaceState {
    pub(super) alive: bool,
    pub(super) wl_surface_id: Option<u32>,
    pub(super) wl_surface_serial: Option<u64>,
    pub(super) mapped_onto: Option<X11Window>,
    pub(super) geometry: Rectangle<i32, Logical>,
    pub(super) override_redirect: bool,

    // The associated wl_surface.
    wl_surface: Option<WlSurface>,
    opaque_regions_hook_id: Option<HookId>,
    deferred_sync_hook_id: Option<HookId>,

    // State for _NET_WM_SYNC_REQUEST.
    sync_counter: Option<SyncRequestCounter>,
    pub(super) sync_alarm: Option<Alarm>,
    last_set_sync_timeout: Duration,
    pub(super) sync_timeout_alarm: Option<Alarm>,
    counter_value: Int64,
    // Highest counter value seen or sent.
    next_counter_value: Int64,
    pending_sync_wait_value: Option<Int64>,

    // Geometry to set after we receive an ACK for the in-flight sync request.
    pending_geometry: Option<Rectangle<i32, Logical>>,
    // Geometry update deferred while a sync request is in progress.
    buffered_geometry: Option<Rectangle<i32, Logical>>,

    title: String,
    class: String,
    instance: String,
    startup_id: Option<String>,
    pid: Option<u32>,
    protocols: Protocols,
    hints: Option<WmHints>,
    normal_hints: Option<WmSizeHints>,
    transient_for: Option<X11Window>,
    pub(super) net_state: HashSet<Atom>,
    motif_hints: Vec<u32>,
    window_type: Vec<Atom>,
    pub(crate) opacity: Option<u32>,
    opaque_region: Option<RegionAttributes>,
    opaque_region_dirty: bool,
    pending_enter: Option<(
        Box<dyn std::any::Any + Send + 'static>,
        Vec<Keycode>,
        Option<ModifiersState>,
        Serial,
    )>,
    pub(super) pending_ping_timestamp: Option<u32>,
}

pub(super) type Protocols = Vec<WMProtocol>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WMProtocol {
    TakeFocus,
    DeleteWindow,
    NetWmPing,
    NetWmSyncRequest,
}

impl PartialEq for X11Surface {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        let self_alive = self.state.lock().unwrap().alive;
        let other_alive = other.state.lock().unwrap().alive;
        self.xwm == other.xwm && self.window == other.window && self_alive && other_alive
    }
}

impl Drop for SharedSurfaceState {
    fn drop(&mut self) {
        if let Some(wl_surface) = self.wl_surface.as_ref() {
            if let Some(hook_id) = self.opaque_regions_hook_id.take() {
                compositor::remove_pre_commit_hook(wl_surface, &hook_id);
            }

            if let Some(hook_id) = self.deferred_sync_hook_id.take() {
                compositor::remove_pre_commit_hook(wl_surface, &hook_id);
            }
        }
    }
}

impl From<ReplyError> for SyncRequestError {
    fn from(value: ReplyError) -> Self {
        match value {
            ReplyError::X11Error(err) => err.into(),
            ReplyError::ConnectionError(err) => err.into(),
        }
    }
}

impl From<ReplyOrIdError> for SyncRequestError {
    fn from(value: ReplyOrIdError) -> Self {
        match value {
            ReplyOrIdError::IdsExhausted => Self::IdsExhausted,
            ReplyOrIdError::X11Error(err) => err.into(),
            ReplyOrIdError::ConnectionError(err) => err.into(),
        }
    }
}

impl From<X11Error> for SyncRequestError {
    fn from(value: X11Error) -> Self {
        Self::X11(value)
    }
}

/// Errors that can happen for operations on an [`X11Surface`]
#[derive(Debug, thiserror::Error)]
pub enum X11SurfaceError {
    /// Error on the underlying X11 Connection
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    /// X11 protocol error
    #[error("X11 protocol error: {0:?}")]
    X11(X11Error),
    /// Operation was unsupported for an override_redirect window
    #[error("Operation was unsupported for an override_redirect window")]
    UnsupportedForOverrideRedirect,
}

impl From<X11Error> for X11SurfaceError {
    fn from(value: X11Error) -> Self {
        Self::X11(value)
    }
}

/// Window types of [`X11Surface`]s
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum WmWindowType {
    Combo,
    Desktop,
    Dnd,
    Dock,
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

/// Window properties of [`X11Surface`]s
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum WmWindowProperty {
    Title,
    Class,
    Protocols,
    Hints,
    NormalHints,
    TransientFor,
    WindowType,
    MotifHints,
    StartupId,
    Pid,
    Opacity,
}

/// https://x.org/releases/X11R7.6/doc/xorg-docs/specs/ICCCM/icccm.html#input_focus
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WmInputModel {
    /// The client never expects keyboard input.
    None,
    /// The client expects keyboard input but never explicitly sets the input focus.
    #[default]
    Passive,
    /// The client expects keyboard input and explicitly sets the input focus,
    /// but it only does so when one of its windows already has the focus.
    LocallyActive,
    /// The client expects keyboard input and explicitly sets the input focus,
    /// even when it is in windows the client does not own.
    GloballyActive,
}

impl X11Surface {
    /// Create a new [`X11Surface`] usually handled by an [`X11Wm`](super::X11Wm)
    ///
    /// ## Arguments
    ///
    /// - `window` X11 window id
    /// - `override_redirect` set if the X11 window has the override redirect flag set
    /// - `conn` Weak reference on the X11 connection
    /// - `servertime_counter` the XID of the server's SERVERTIME counter, or `None`
    /// - `atoms` Atoms struct as defined by the [xwm module](super).
    /// - `geometry` Initial geometry of the window
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        xwm: Option<&X11Wm>,
        window: u32,
        override_redirect: bool,
        conn: Weak<RustConnection>,
        atoms: super::Atoms,
        servertime_counter: Option<Counter>,
        geometry: Rectangle<i32, Logical>,
        xdnd_active: Arc<AtomicBool>,
    ) -> X11Surface {
        X11Surface {
            xwm: xwm.map(|wm| wm.id),
            client_scale: xwm.map(|wm| wm.client_scale.clone()),
            window,
            conn,
            atoms,
            servertime_counter,
            state: Arc::new(Mutex::new(SharedSurfaceState {
                alive: true,
                wl_surface_id: None,
                wl_surface_serial: None,
                wl_surface: None,
                opaque_regions_hook_id: None,
                deferred_sync_hook_id: None,
                sync_counter: None,
                sync_alarm: None,
                last_set_sync_timeout: DEFAULT_SYNC_REQUEST_TIMEOUT,
                sync_timeout_alarm: None,
                counter_value: Default::default(),
                next_counter_value: Default::default(),
                pending_sync_wait_value: None,
                pending_geometry: None,
                buffered_geometry: None,
                mapped_onto: None,
                geometry,
                override_redirect,
                title: String::from(""),
                class: String::from(""),
                instance: String::from(""),
                startup_id: None,
                pid: None,
                protocols: Vec::new(),
                hints: None,
                normal_hints: None,
                transient_for: None,
                net_state: HashSet::new(),
                motif_hints: vec![0; 5],
                window_type: Vec::new(),
                opacity: None,
                opaque_region: None,
                opaque_region_dirty: true,
                pending_enter: None,
                pending_ping_timestamp: None,
            })),
            xdnd_active,
            focus_release: xwm.map(|wm| wm.focus_release.clone()),
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
        if self.is_override_redirect() {
            return Err(X11SurfaceError::UnsupportedForOverrideRedirect);
        }

        if let Some(conn) = self.conn.upgrade() {
            if let Some(frame) = self.state.lock().unwrap().mapped_onto {
                if mapped {
                    conn.map_window(frame)?;
                } else {
                    conn.unmap_window(frame)?;
                }
                conn.flush()?;
            }
        }

        Ok(())
    }

    /// Returns if this window has the override redirect flag set or not
    pub fn is_override_redirect(&self) -> bool {
        self.state.lock().unwrap().override_redirect
    }

    /// Returns if the window is currently mapped or not
    pub fn is_mapped(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.wl_surface.is_some()
    }

    /// Returns if the window is still alive
    #[inline]
    pub fn alive(&self) -> bool {
        self.state.lock().unwrap().alive && self.conn.strong_count() != 0
    }

    /// Unconditionally configures the window and sends a configure notify.
    fn send_configure(
        &self,
        state: &mut SharedSurfaceState,
        rect: impl Into<Option<Rectangle<i32, Logical>>>,
    ) -> Result<Rectangle<i32, Logical>, X11SurfaceError> {
        let rect = rect.into();
        if let Some(conn) = self.conn.upgrade() {
            let client_scale = self
                .client_scale
                .as_ref()
                .map(|s| s.load(Ordering::Acquire))
                .unwrap_or(1.);
            let logical_rect = rect.unwrap_or(state.geometry);
            let rect = logical_rect.to_client_precise_round(client_scale);
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

            Ok(logical_rect)
        } else {
            Err(X11SurfaceError::Connection(ConnectionError::UnknownError))
        }
    }

    /// Send a configure to this window.
    ///
    /// If `rect` is provided the new state will be send to the window.
    /// If `rect` is `None` a synthetic configure event with the existing state will be send.
    ///
    /// If a pending request sync is in progress (see [`X11Surface::configure_with_sync`]), this
    /// will cancel the sync and send a configure immediately.
    pub fn configure(&self, rect: impl Into<Option<Rectangle<i32, Logical>>>) -> Result<(), X11SurfaceError> {
        let rect = rect.into();
        if self.is_override_redirect() && rect.is_some() {
            Err(X11SurfaceError::UnsupportedForOverrideRedirect)
        } else {
            let mut state = self.state.lock().unwrap();
            let rect = rect.or(state.pending_geometry);

            if state.pending_sync_wait_value.is_some() {
                self.finish_pending_sync(&mut state);
            }
            state.buffered_geometry = None;

            let new_geometry = self.send_configure(&mut state, rect)?;
            state.geometry = new_geometry;

            Ok(())
        }
    }

    /// Configure the window, syncing with the client's repaint.
    ///
    /// Sends a `_NET_WM_SYNC_REQUEST` message to this window, and configures it, sending a
    /// `ConfigureNotify` event to the client.  The client will notify us when it has finished
    /// painting the window content that corresponds with this configure.
    ///
    /// If a pending sync is already in progress, it is buffered and will automatically be sent
    /// (with a new sync request) after the in-progress sync is finished and the client has
    /// committed a new buffer.
    ///
    /// Until the client ACKs the sync request, the surface's geometry remains frozen at the
    /// previous value.  When the ACK is received,
    /// [`XwmHandler::sync_request_acked`](super::XwmHandler::sync_request_acked) is called.
    ///
    /// The `timeout` parameter specifies the amount of time before the sync request is considered
    /// missed.  If not provided, it defaults to one second.  If the sync request times out,
    /// [`XwmHandler::sync_request_timeout`](super::XwmHandler::sync_request_timeout) is called.
    ///
    /// If the client does not support the `_NET_WM_SYNC_REQUEST` protocol, or if `rect` has the
    /// same size as the current geometry and there is no sync in progress, a regular configure
    /// sequence is initiated (as if [`X11Surface::configure`] was called).
    ///
    /// This configure method is most useful during interactive window resizes, as it avoids
    /// sending configure notify events to the client faster than it can repaint, and also avoids
    /// artifacts (like black bars or old window content) along the resize edge.  However, in
    /// principle the sync protocol can be used for *every* configure the compositor does, if it
    /// wants better-looking size changes, at the expense of some overhead and complexity.
    pub fn configure_with_sync(
        &self,
        rect: impl Into<Rectangle<i32, Logical>>,
        timeout: Option<Duration>,
    ) -> Result<(), X11SurfaceError> {
        if self.is_override_redirect() {
            Err(X11SurfaceError::UnsupportedForOverrideRedirect)
        } else {
            let rect = rect.into();
            let mut state = self.state.lock().unwrap();

            if rect.size == state.geometry.size && state.pending_geometry.is_none() {
                // If the passed size is the same as our stored geometry's size, and there's no
                // in-flight sync request, we send a normal configure without a sync request.  Some
                // clients, when they get a configure-notify with the same size as their current
                // geometry, won't paint, and so won't update the sync counter, which will block
                // further configure events until the timeout.  But we still need to send the
                // configure, especially if the location has changed.
                drop(state);
                self.configure(rect)
            } else {
                match self.send_sync_request(&mut state, timeout.unwrap_or(DEFAULT_SYNC_REQUEST_TIMEOUT)) {
                    Err(SyncRequestError::NotSupported) => {
                        drop(state);
                        self.configure(rect)
                    }
                    Err(SyncRequestError::RequestPending) => {
                        state.buffered_geometry = Some(rect);
                        Ok(())
                    }
                    Ok(_) => match self.send_configure(&mut state, rect) {
                        Err(err) => {
                            self.finish_pending_sync(&mut state);
                            Err(err)
                        }
                        Ok(pending_geometry) => {
                            state.pending_geometry = Some(pending_geometry);
                            state.buffered_geometry = None;
                            Ok(())
                        }
                    },
                    Err(SyncRequestError::IdsExhausted) => {
                        self.set_allow_commits(&state, true);
                        Err(ConnectionError::UnknownError.into())
                    }
                    Err(SyncRequestError::Connection(err)) => {
                        self.set_allow_commits(&state, true);
                        Err(err.into())
                    }
                    Err(SyncRequestError::X11(err)) => {
                        self.set_allow_commits(&state, true);
                        Err(err.into())
                    }
                }
            }
        }
    }

    /// Sends a sync request to the client.
    ///
    /// This increments a counter used in the `_NET_WM_SYNC_REQUEST` protocol and sends a client
    /// message to the surface's X11 window, which will notify the client that the compositor wants
    /// to change the size of the client's window during a resize operation.
    ///
    /// When the client has finished repainting at the new size,
    /// [`XwmHandler::sync_request_acked`](super::XwmHandler::sync_request_acked) will be called.
    ///
    /// This also registers a timeout.  If the sync request times out,
    /// [`XwmHandler::sync_request_timeout`](super::XwmHandler::sync_request_timeout) will be
    /// called.
    ///
    /// This returns the value of the counter that was sent to the client.
    fn send_sync_request(
        &self,
        state: &mut SharedSurfaceState,
        timeout: Duration,
    ) -> Result<i64, SyncRequestError> {
        state.last_set_sync_timeout = timeout;

        if let Some(counter) = state
            .sync_counter
            .as_ref()
            .filter(|_| self.servertime_counter.is_some())
        {
            if state.pending_sync_wait_value.is_none() {
                let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
                let is_extended = matches!(counter, SyncRequestCounter::Extended(_));

                let next = add_xsync_value(state.next_counter_value, SYNC_REQUEST_INCREMENT);
                state.next_counter_value = next;

                self.set_allow_commits(state, false);

                let msg_data = [
                    self.atoms._NET_WM_SYNC_REQUEST,
                    x11rb::CURRENT_TIME,
                    next.lo,
                    next.hi as u32,
                    if is_extended { 1 } else { 0 },
                ];
                let event = ClientMessageEvent::new(32, self.window, self.atoms.WM_PROTOCOLS, msg_data);
                conn.send_event(false, self.window, EventMask::NO_EVENT, event)?;

                self.init_sync_timeout(state, timeout)?;

                conn.flush()?;

                state.pending_sync_wait_value = Some(next);

                Ok(xsync_value(next))
            } else {
                Err(SyncRequestError::RequestPending)
            }
        } else {
            Err(SyncRequestError::NotSupported)
        }
    }

    /// Clears pending sync after an ACK has been received.
    ///
    /// Returns `true` if `value` is valid as an ACK for the pending sync request (or if there is
    /// no pending sync, or if the protocol is not supported), `false` otherwise.
    fn take_pending_sync_ack(&self, state: &mut SharedSurfaceState, value: i64) -> bool {
        if let Some(counter) = &state.sync_counter {
            let is_extended = matches!(counter, SyncRequestCounter::Extended(_));

            if let Some(pending) = state.pending_sync_wait_value.take() {
                let pending_v = xsync_value(pending);

                let valid = value >= pending_v && (!is_extended || value % 2 == 0);
                if !valid {
                    state.pending_sync_wait_value = Some(pending);
                }
                valid
            } else {
                true
            }
        } else {
            true
        }
    }

    /// Creates a sync alarm for the specified counter.
    ///
    /// This causes the X server to send us an X event every time the client increments the
    /// counter by at least +1.
    fn init_sync_alarm(&self, state: &mut MutexGuard<'_, SharedSurfaceState>) -> Result<(), ReplyOrIdError> {
        if let Some(counter) = &state.sync_counter {
            if state.sync_alarm.is_none() {
                let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;

                let (counter, is_extended) = match counter {
                    SyncRequestCounter::Basic(counter) => (*counter, false),
                    SyncRequestCounter::Extended(counter) => (*counter, true),
                };

                if is_extended {
                    state.counter_value = conn
                        .sync_query_counter(counter)?
                        .reply_unchecked()?
                        .ok_or(ConnectionError::UnknownError)?
                        .counter_value;
                } else {
                    state.counter_value = Int64 { hi: 0, lo: 0 };
                    conn.sync_set_counter(counter, state.counter_value)?;
                }

                state.next_counter_value = state.counter_value;

                let aux = CreateAlarmAux::new()
                    .counter(counter)
                    .delta(value_to_xsync(1))
                    .value_type(VALUETYPE::RELATIVE)
                    .value(value_to_xsync(1))
                    .test_type(TESTTYPE::POSITIVE_COMPARISON)
                    .events(1);
                let alarm = conn.generate_id()?;
                conn.sync_create_alarm(alarm, &aux)?.check()?;
                state.sync_alarm = Some(alarm);
            }
        } else {
            self.destroy_sync_alarm(state);
        }

        Ok(())
    }

    /// Destroys the counter sync alarm and finishes any in-flight sync.
    fn destroy_sync_alarm(&self, state: &mut MutexGuard<'_, SharedSurfaceState>) {
        if let Some(sync_alarm) = state.sync_alarm.take() {
            if let Some(conn) = self.conn.upgrade() {
                let _ = conn.sync_destroy_alarm(sync_alarm);
                let _ = conn.flush();
            }
        }
        self.finish_pending_sync(state);
    }

    /// Handles a sync alarm for the sync counter for this surface.
    ///
    /// If `new_value` is a valid ACK for the currently in-flight sync request, the pending
    /// geometry is applied.
    ///
    /// The caller is responsible for notifying the compositor that the sync was ACKed, and of the
    /// new in-flight request, if any.
    pub(super) fn handle_sync_alarm(&self, new_value: Int64) -> bool {
        let new_value_raw = xsync_value(new_value);

        let mut state = self.state.lock().unwrap();
        state.counter_value = new_value;
        state.next_counter_value = value_to_xsync(new_value_raw.max(xsync_value(state.next_counter_value)));

        if self.take_pending_sync_ack(&mut state, new_value_raw) {
            self.finish_pending_sync(&mut state);
            true
        } else {
            false
        }
    }

    /// Fetches the X server time and sets an alarm for that plus `timeout`.
    fn init_sync_timeout(
        &self,
        state: &mut SharedSurfaceState,
        timeout: Duration,
    ) -> Result<(), SyncRequestError> {
        self.destroy_sync_timeout(state);

        if let Some(conn) = self.conn.upgrade() {
            if let Some(servertime_counter) = self.servertime_counter {
                let now = xsync_value(
                    conn.sync_query_counter(servertime_counter)?
                        .reply_unchecked()?
                        .ok_or(SyncRequestError::NotSupported)?
                        .counter_value,
                ) as u64;
                // The SERVERTIME counter is an unsigned 64-bit millisecond value.
                let timeout = value_to_xsync(now.wrapping_add(timeout.as_millis() as u64) as i64);

                let alarm = conn.generate_id()?;

                let aux = CreateAlarmAux::new()
                    .counter(self.servertime_counter)
                    .value_type(VALUETYPE::ABSOLUTE)
                    .value(timeout)
                    .test_type(TESTTYPE::POSITIVE_COMPARISON)
                    .delta(value_to_xsync(0))
                    .events(1);
                conn.sync_create_alarm(alarm, &aux)?.check()?;
                conn.flush()?;

                state.sync_timeout_alarm = Some(alarm);
                Ok(())
            } else {
                Err(SyncRequestError::NotSupported)
            }
        } else {
            Err(ConnectionError::UnknownError.into())
        }
    }

    fn destroy_sync_timeout(&self, state: &mut SharedSurfaceState) {
        if let Some(alarm) = state.sync_timeout_alarm.take() {
            if let Some(conn) = self.conn.upgrade() {
                if let Err(err) = conn.sync_destroy_alarm(alarm).and_then(|_| conn.flush()) {
                    tracing::warn!("Failed to unregister sync timeout alarm: {err}");
                }
            }
        }
    }

    /// Cleans up after a sync request times out.
    pub(super) fn handle_sync_timeout(&self) {
        self.finish_pending_sync(&mut self.state.lock().unwrap());
    }

    /// Tells the X server to enable/disable commits on the window's underlying wl_surface.
    fn set_allow_commits(&self, state: &SharedSurfaceState, allow_commits: bool) {
        let result = self
            .conn
            .upgrade()
            .ok_or(ConnectionError::UnknownError)
            .and_then(|conn| {
                let window = state.mapped_onto.unwrap_or(self.window);
                conn.change_property32(
                    PropMode::REPLACE,
                    window,
                    self.atoms._XWAYLAND_ALLOW_COMMITS,
                    AtomEnum::CARDINAL,
                    &[if allow_commits { 1 } else { 0 }],
                )?;
                conn.flush()?;
                Ok(())
            });

        if let Err(err) = result {
            tracing::warn!(
                "Failed to update _XWAYLAND_ALLOW_COMMITS to {allow_commits} on window 0x{:08x}: {err}",
                self.window,
            );
        }
    }

    /// Finishes an in-flight sync request.
    ///
    /// Promotes the pending geometry to the active geometry, unregisters the sync timeout, and
    /// tells the XWayland server to start committing buffers again.
    fn finish_pending_sync(&self, state: &mut SharedSurfaceState) {
        state.pending_sync_wait_value = None;
        if let Some(pending_geometry) = state.pending_geometry.take() {
            state.geometry = pending_geometry;
        }
        self.destroy_sync_timeout(state);
        self.set_allow_commits(state, true);
    }

    /// Returns the associated wl_surface.
    ///
    /// This will only return `Some` once:
    ///   - The `WL_SURFACE_SERIAL` has been set on the x11 window, and
    ///   - The wl_surface has been assigned the same serial using the [xwayland
    ///     shell](crate::wayland::xwayland_shell) protocol on the wayland side,
    ///     and then committed.
    #[inline]
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
    /// Otherwise, it will set [`wl_surface_id`][Self::wl_surface_id] instead.
    pub fn wl_surface_serial(&self) -> Option<u64> {
        self.state.lock().unwrap().wl_surface_serial
    }

    pub(crate) fn set_wl_surface<D: SeatHandler + 'static>(&self, data: &mut D, surface: Option<WlSurface>) {
        let mut state = self.state.lock().unwrap();

        if let Some(hook_id) = state.opaque_regions_hook_id.take() {
            if let Some(wl_surface) = state.wl_surface.as_ref() {
                compositor::remove_pre_commit_hook(wl_surface, &hook_id);
            }
        }

        if let Some(hook_id) = state.deferred_sync_hook_id.take() {
            if let Some(wl_surface) = state.wl_surface.as_ref() {
                compositor::remove_pre_commit_hook(wl_surface, &hook_id);
            }
        }

        if let (Some(surface), None) = (surface.as_ref(), state.wl_surface.as_ref()) {
            if let Some((seat, keys, mods, serial)) = state.pending_enter.take() {
                if let Ok(seat) = seat.downcast::<Seat<D>>() {
                    enter_internal(surface, &seat, data, keys.into_iter(), serial);
                    if let Some(modifiers) = mods {
                        KeyboardTarget::modifiers(surface, &seat, data, modifiers, serial);
                    }
                }
            }
        }
        state.wl_surface = surface;

        if let Some(wl_surface) = state.wl_surface.clone() {
            self.register_opaque_regions_hook::<D>(&mut state, &wl_surface);
            self.register_deferred_sync_hook::<D>(&mut state, &wl_surface);
        }
    }

    fn register_opaque_regions_hook<D: 'static>(
        &self,
        state: &mut SharedSurfaceState,
        wl_surface: &WlSurface,
    ) {
        let hook_id = {
            let state = Arc::downgrade(&self.state);
            let conn = self.conn.clone();
            let window = self.window;
            let property_atom = self.atoms._NET_WM_OPAQUE_REGION;
            let client_scale = self.client_scale.clone();

            compositor::add_pre_commit_hook::<D, _>(wl_surface, move |_, _, wl_surface| {
                if let Some(state) = state.upgrade() {
                    let update_opaque_region_if_needed = || {
                        if state.lock().unwrap().opaque_region_dirty {
                            if let Some(conn) = conn.upgrade() {
                                if let Ok(opaque_region) =
                                    fetch_opaque_regions(&conn, window, property_atom, client_scale.as_ref())
                                {
                                    let mut state = state.lock().unwrap();
                                    state.opaque_region = opaque_region;
                                    state.opaque_region_dirty = false;
                                    return Some(state.opaque_region.clone());
                                }
                            }
                        }
                        None
                    };

                    let opaque_region = update_opaque_region_if_needed()
                        .unwrap_or_else(|| state.lock().unwrap().opaque_region.clone());

                    compositor::with_states(wl_surface, |states| {
                        let mut guard = states.cached_state.get::<SurfaceAttributes>();
                        let attrs = guard.pending();
                        attrs.opaque_region = opaque_region;
                    });
                }
            })
        };
        state.opaque_regions_hook_id = Some(hook_id);
    }

    fn register_deferred_sync_hook<D: 'static>(
        &self,
        state: &mut SharedSurfaceState,
        wl_surface: &WlSurface,
    ) {
        let hook_id = {
            let surface = self.clone();

            compositor::add_pre_commit_hook::<D, _>(wl_surface, move |_, _, _| {
                let (buffered_geometry, timeout) = {
                    let mut state = surface.state.lock().unwrap();
                    let buffered_geometry = if state.pending_sync_wait_value.is_none()
                        && state.buffered_geometry.is_some_and(|geom| geom != state.geometry)
                    {
                        // We only send a new configure if:
                        //   1) there is no sync currently in progress, and
                        //   2) if the buffered geometry is not the current geometry.
                        state.buffered_geometry.take()
                    } else {
                        None
                    };
                    (buffered_geometry, state.last_set_sync_timeout)
                };

                if let Some(buffered_geometry) = buffered_geometry {
                    if let Err(err) = surface.configure_with_sync(buffered_geometry, Some(timeout)) {
                        tracing::info!(
                            "Failed to send buffered surface configure/sync for window 0x{:08x}: {err}",
                            surface.window
                        );
                    }
                }
            })
        };
        state.deferred_sync_hook_id = Some(hook_id);
    }

    /// Returns the current geometry of the underlying X11 window
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        self.state.lock().unwrap().geometry
    }

    /// Returns the pending geometry, if any
    ///
    /// The pending geometry has already been sent to the X server and client as a part of
    /// [`configure_with_sync`](X11Surface::configure_with_sync), but the client has not yet
    /// acknowledged the corresponding sync request.
    ///
    /// The pending geometry is promoted to [`geometry`](X11Surface::geometry) after the sync
    /// request has been acknowledged.
    pub fn pending_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        self.state.lock().unwrap().pending_geometry
    }

    /// Returns the buffered geometry, if any
    ///
    /// The buffered geometry is the most recent rect passed to
    /// [`configure_with_sync`](X11Surface::configure_with_sync).  It has not yet been sent to the
    /// X server or client because an older sync request is still in progress.  Once the client
    /// acknowledges the in-progress sync request and commits a buffer, the buffered geometry will
    /// be sent to the client as part of a new sync request.
    ///
    /// The buffered geometry is promoted to [`pending_geometry`](X11Surface::pending_geometry)
    /// once it has been sent to the client.
    pub fn buffered_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        self.state.lock().unwrap().buffered_geometry
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

    /// Returns the PID of the underlying X11 window
    pub fn pid(&self) -> Option<u32> {
        self.state.lock().unwrap().pid
    }

    /// Returns the opacity of the underlying X11 window
    pub fn opacity(&self) -> Option<u32> {
        self.state.lock().unwrap().opacity
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

    /// Returns the hints for the underlying X11 window
    pub fn hints(&self) -> Option<WmHints> {
        self.state.lock().unwrap().hints
    }

    /// Returns the size hints for the underlying X11 window
    pub fn size_hints(&self) -> Option<WmSizeHints> {
        self.state.lock().unwrap().normal_hints
    }

    /// Returns the suggested minimum size of the underlying X11 window
    pub fn min_size(&self) -> Option<Size<i32, Logical>> {
        let client_scale = self
            .client_scale
            .as_ref()
            .map(|s| s.load(Ordering::Acquire))
            .unwrap_or(1.);
        let state = self.state.lock().unwrap();
        state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.min_size)
            .map(Size::<i32, Client>::from)
            .map(|s| s.to_f64().to_logical(client_scale).to_i32_round())
    }

    /// Returns the suggested minimum size of the underlying X11 window
    pub fn max_size(&self) -> Option<Size<i32, Logical>> {
        let client_scale = self
            .client_scale
            .as_ref()
            .map(|s| s.load(Ordering::Acquire))
            .unwrap_or(1.);
        let state = self.state.lock().unwrap();
        state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.max_size)
            .map(Size::<i32, Client>::from)
            .map(|s| s.to_f64().to_logical(client_scale).to_i32_round())
    }

    /// Returns the suggested base size of the underlying X11 window
    pub fn base_size(&self) -> Option<Size<i32, Logical>> {
        let client_scale = self
            .client_scale
            .as_ref()
            .map(|s| s.load(Ordering::Acquire))
            .unwrap_or(1.);
        let state = self.state.lock().unwrap();
        let res = state
            .normal_hints
            .as_ref()
            .and_then(|hints| hints.base_size)
            .map(Size::<i32, Client>::from)
            .map(|s| s.to_f64().to_logical(client_scale).to_i32_round());
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

    /// Returns if the window is in the hidden state
    pub fn is_hidden(&self) -> bool {
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

    /// Returns if the window is in the "above" (always on top) state
    pub fn is_above(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_ABOVE)
    }

    /// Returns if the window is in the "below" state
    pub fn is_below(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_BELOW)
    }

    /// Returns if the window has requested to be hidden from taskbars
    pub fn is_skip_taskbar(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_SKIP_TASKBAR)
    }

    /// Returns if the window has requested to be hidden from pagers
    pub fn is_skip_pager(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_SKIP_PAGER)
    }

    /// Returns if the window is sticky.
    ///
    /// This is usually used to mean that the window should be shown on all workspaces.
    pub fn is_sticky(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_STICKY)
    }

    /// Returns if the window is shaded.
    pub fn is_shaded(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_SHADED)
    }

    /// Returns if the window demands attention.
    pub fn demands_attention(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .net_state
            .contains(&self.atoms._NET_WM_STATE_DEMANDS_ATTENTION)
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

    /// Sets the window as hidden or not.
    ///
    /// Allows the client to e.g. stop rendering.
    pub fn set_hidden(&self, suspended: bool) -> Result<(), ConnectionError> {
        if suspended {
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

    /// Sets the window as above (always on top) or not.
    ///
    /// Allows the client to reflect this state in their UI.
    pub fn set_above(&self, above: bool) -> Result<(), ConnectionError> {
        if above {
            self.change_net_state(&[self.atoms._NET_WM_STATE_ABOVE], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_ABOVE])?;
        }
        Ok(())
    }

    /// Sets the window as below or not.
    ///
    /// Allows the client to reflect this state in their UI.
    pub fn set_below(&self, below: bool) -> Result<(), ConnectionError> {
        if below {
            self.change_net_state(&[self.atoms._NET_WM_STATE_BELOW], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_BELOW])?;
        }
        Ok(())
    }

    /// Sets the window's sticky state.
    ///
    /// This is usually used to mean that the window should be shown on all workspaces.
    pub fn set_sticky(&self, sticky: bool) -> Result<(), ConnectionError> {
        if sticky {
            self.change_net_state(&[self.atoms._NET_WM_STATE_STICKY], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_STICKY])?;
        }
        Ok(())
    }

    /// Sets the window's shaded state.
    pub fn set_shaded(&self, shaded: bool) -> Result<(), ConnectionError> {
        if shaded {
            self.change_net_state(&[self.atoms._NET_WM_STATE_SHADED], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_SHADED])?;
        }
        Ok(())
    }

    /// Sets the window's demands-attention state.
    ///
    /// A client might flash the client-side decorations when demanding attention.
    pub fn set_demands_attention(&self, demands_attention: bool) -> Result<(), ConnectionError> {
        if demands_attention {
            self.change_net_state(&[self.atoms._NET_WM_STATE_DEMANDS_ATTENTION], &[])?;
        } else {
            self.change_net_state(&[], &[self.atoms._NET_WM_STATE_DEMANDS_ATTENTION])?;
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
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_COMBO => Some(WmWindowType::Combo),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_DESKTOP => Some(WmWindowType::Desktop),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_DND => Some(WmWindowType::Dnd),
                x if *x == self.atoms._NET_WM_WINDOW_TYPE_DOCK => Some(WmWindowType::Dock),
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

            let wm_state = if state.net_state.contains(&self.atoms._NET_WM_STATE_HIDDEN) {
                [3u32 /*IconicState*/, 0 /*WINDOW_NONE*/]
            } else {
                [1u32 /*NormalState*/, 0 /*WINDOW_NONE*/]
            };
            conn.change_property32(
                PropMode::REPLACE,
                self.window,
                self.atoms.WM_STATE,
                self.atoms.WM_STATE,
                &wm_state,
            )?;
        }

        Ok(())
    }

    /// Input handling model requested by the underlying X11 window.
    ///
    /// See ICCCM §4.1.7 for details.
    pub fn input_model(&self) -> WmInputModel {
        let state = self.state.lock().unwrap();
        match (
            state.hints.as_ref().and_then(|hints| hints.input).unwrap_or(true),
            state.protocols.contains(&WMProtocol::TakeFocus),
        ) {
            (false, false) => WmInputModel::None,
            (true, false) => WmInputModel::Passive, // the default
            (true, true) => WmInputModel::LocallyActive,
            (false, true) => WmInputModel::GloballyActive,
        }
    }

    pub(super) fn update_properties(&self) -> Result<(), ConnectionError> {
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
        self.update_pid()?;
        self.update_opacity()?;
        if let Some(conn) = self.conn.upgrade() {
            let mut state = self.state.lock().unwrap();
            state.opaque_region = fetch_opaque_regions(
                &conn,
                self.window,
                self.atoms._NET_WM_OPAQUE_REGION,
                self.client_scale.as_ref(),
            )?;
            state.opaque_region_dirty = false;
        }
        Ok(())
    }

    pub(super) fn update_property(&self, atom: Atom) -> Result<Option<WmWindowProperty>, ConnectionError> {
        match atom {
            atom if atom == self.atoms._NET_WM_NAME || atom == AtomEnum::WM_NAME.into() => {
                self.update_title()?;
                Ok(Some(WmWindowProperty::Title))
            }
            atom if atom == AtomEnum::WM_CLASS.into() => {
                self.update_class()?;
                Ok(Some(WmWindowProperty::Class))
            }
            atom if atom == self.atoms.WM_PROTOCOLS => {
                self.update_protocols()?;
                Ok(Some(WmWindowProperty::Protocols))
            }
            atom if atom == self.atoms.WM_HINTS => {
                self.update_hints()?;
                Ok(Some(WmWindowProperty::Hints))
            }
            atom if atom == AtomEnum::WM_NORMAL_HINTS.into() => {
                self.update_normal_hints()?;
                Ok(Some(WmWindowProperty::NormalHints))
            }
            atom if atom == AtomEnum::WM_TRANSIENT_FOR.into() => {
                self.update_transient_for()?;
                Ok(Some(WmWindowProperty::TransientFor))
            }
            atom if atom == self.atoms._NET_WM_WINDOW_TYPE => {
                self.update_net_window_type()?;
                Ok(Some(WmWindowProperty::WindowType))
            }
            atom if atom == self.atoms._MOTIF_WM_HINTS => {
                self.update_motif_hints()?;
                Ok(Some(WmWindowProperty::MotifHints))
            }
            atom if atom == self.atoms._NET_STARTUP_ID => {
                self.update_startup_id()?;
                Ok(Some(WmWindowProperty::StartupId))
            }
            atom if atom == self.atoms._NET_WM_PID => {
                self.update_pid()?;
                Ok(Some(WmWindowProperty::Pid))
            }
            atom if atom == self.atoms._NET_WM_WINDOW_OPACITY => {
                self.update_opacity()?;
                Ok(Some(WmWindowProperty::Opacity))
            }
            atom if atom == self.atoms._NET_WM_OPAQUE_REGION => {
                let mut state = self.state.lock().unwrap();
                state.opaque_region = None;
                state.opaque_region_dirty = true;
                Ok(None)
            }

            _ => Ok(None), // unknown
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

    fn update_pid(&self) -> Result<(), ConnectionError> {
        if let Some(pid) = self.read_window_property_u32(self.atoms._NET_WM_PID)? {
            let mut state = self.state.lock().unwrap();
            state.pid = Some(pid);
        }
        Ok(())
    }

    fn update_opacity(&self) -> Result<(), ConnectionError> {
        if let Some(opacity) = self.read_window_property_u32(self.atoms._NET_WM_WINDOW_OPACITY)? {
            let mut state = self.state.lock().unwrap();
            state.opacity = Some(opacity);
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
                x if x == self.atoms._NET_WM_PING => Some(WMProtocol::NetWmPing),
                x if x == self.atoms._NET_WM_SYNC_REQUEST => Some(WMProtocol::NetWmSyncRequest),
                _ => None,
            })
            .collect::<Vec<_>>();

        drop(state);
        self.init_net_wm_sync_request()?;

        Ok(())
    }

    pub(super) fn init_net_wm_sync_request(&self) -> Result<(), ConnectionError> {
        let mut state = self.state.lock().unwrap();
        if state.protocols.contains(&WMProtocol::NetWmSyncRequest) {
            let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
            let counters_reply = conn
                .get_property(
                    false,
                    self.window,
                    self.atoms._NET_WM_SYNC_REQUEST_COUNTER,
                    AtomEnum::CARDINAL,
                    0,
                    2,
                )?
                .reply_unchecked()?;

            let counter = counters_reply.and_then(|reply| {
                reply
                    .value32()
                    .and_then(|mut values| match (values.next(), values.next()) {
                        (_, Some(extended)) => Some(SyncRequestCounter::Extended(extended)),
                        (Some(basic), _) => Some(SyncRequestCounter::Basic(basic)),
                        _ => None,
                    })
            });

            if counter != state.sync_counter {
                state.sync_counter = counter;
                self.destroy_sync_alarm(&mut state);
            }

            if let Err(err) = self.init_sync_alarm(&mut state) {
                state.sync_counter = None;
                match err {
                    ReplyOrIdError::ConnectionError(err) => Err(err),
                    _ => Err(ConnectionError::UnknownError),
                }
            } else {
                Ok(())
            }
        } else {
            state.sync_counter = None;
            let _ = self.init_sync_alarm(&mut state);
            Ok(())
        }
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

    fn read_window_property_u32(&self, atom: impl Into<Atom>) -> Result<Option<u32>, ConnectionError> {
        let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
        let reply = match conn
            .get_property(false, self.window, atom, AtomEnum::CARDINAL, 0, 1)?
            .reply_unchecked()
        {
            Ok(Some(reply)) => reply,
            Ok(None) | Err(ConnectionError::ParseError(_)) => return Ok(None),
            Err(err) => return Err(err),
        };

        if let Some(mut value32) = reply.value32() {
            if let Some(value) = value32.next() {
                return Ok(Some(value));
            }
        }

        Ok(None)
    }

    /// Retrieve user_data associated with this X11 window
    pub fn user_data(&self) -> &UserDataMap {
        &self.user_data
    }

    /// Sends a `_NET_WM_PING` message to the window.
    ///
    /// The passed `timestamp` must be unique, as older clients may only send the timestamp in the
    /// ping reply for use in matching the reply to the correct window.  In particular, do not pass
    /// `x11rb::CURRENT_TIME`.
    ///
    /// You can implement the
    /// [`XwmHandler::ping_acked()`](crate::xwayland::xwm::XwmHandler::ping_acked) trait item in
    /// order to be notified when the client responds.
    pub fn send_ping(&self, timestamp: u32) -> Result<(), PingError> {
        if timestamp == x11rb::CURRENT_TIME {
            Err(PingError::InvalidTimestamp)
        } else {
            let mut state = self.state.lock().unwrap();
            if let Some(timestamp) = &state.pending_ping_timestamp {
                Err(PingError::PingAlreadyPending(*timestamp))
            } else {
                let conn = self.conn.upgrade().ok_or(ConnectionError::UnknownError)?;
                if state.protocols.contains(&WMProtocol::NetWmPing) && timestamp != x11rb::CURRENT_TIME {
                    let event = ClientMessageEvent::new(
                        32,
                        self.window,
                        self.atoms.WM_PROTOCOLS,
                        [self.atoms._NET_WM_PING, timestamp, self.window, 0, 0],
                    );
                    conn.send_event(false, self.window, EventMask::NO_EVENT, event)?;
                    conn.flush()?;
                    state.pending_ping_timestamp = Some(timestamp);
                    Ok(())
                } else {
                    Err(PingError::NotSupported)
                }
            }
        }
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

    /// Get the client PID associated with the X11 window.
    pub fn get_client_pid(&self) -> Result<u32, Box<dyn std::error::Error>> {
        if let Some(connection) = self.conn.upgrade() {
            let window = self.window;

            match query_client_ids(
                &connection,
                &[ClientIdSpec {
                    client: window,
                    mask: x11rb::protocol::res::ClientIdMask::LOCAL_CLIENT_PID,
                }],
            ) {
                Ok(cookie) => {
                    let reply = cookie.reply()?;

                    if let Some(id_value) = reply.ids.first() {
                        if let Some(pid) = id_value.value.first().copied() {
                            return Ok(pid);
                        } else {
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "No matching client ID found",
                            )));
                        }
                    }
                }
                Err(_) => {
                    return Ok(0);
                }
            }
        }
        Ok(0)
    }

    /// Returns the topmost (sub-)surface under a given position of the surface.
    ///
    /// In case the window is not mapped or is unmanaged while an XDND operation is ongoing [`None`] is returned.
    ///
    /// - `point` has to be the position to query, relative to (0, 0) of the given surface + `location`.
    /// - `location` can be used to offset the returned point.
    /// - `surface_type` can be used to filter the underlying surface tree
    #[cfg(feature = "desktop")]
    pub fn surface_under(
        &self,
        point: Point<f64, Logical>,
        location: impl Into<Point<i32, Logical>>,
        surface_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        if !surface_type.contains(WindowSurfaceType::TOPLEVEL) {
            return None;
        }
        if self.xdnd_active.load(Ordering::Acquire) && self.is_override_redirect() {
            return None;
        }
        if let Some(surface) = X11Surface::wl_surface(self).as_ref() {
            return under_from_surface_tree(surface, point, location, surface_type);
        }

        None
    }

    /// Returns whether or not the `_NET_WM_SYNC_REQUEST` protocol is supported.
    ///
    /// The `_NET_WM_SYNC_REQUEST` protocol is used to allow the compositor to synchronize
    /// frame/decorations resize with the client's window repaint during interactive resizes.
    pub fn sync_request_supported(&self) -> bool {
        self.servertime_counter.is_some() && self.state.lock().unwrap().sync_counter.is_some()
    }

    pub(super) fn handle_destroyed(&self) {
        let mut state = self.state.lock().unwrap();
        state.alive = false;
        self.destroy_sync_alarm(&mut state);
        self.destroy_sync_timeout(&mut state);
        // Counter is owned by the client; do not destroy it.
        state.sync_counter = None;

        // Break reference cycle caused by deferred_sync hook
        if let Some(hook_id) = state.deferred_sync_hook_id.take() {
            if let Some(wl_surface) = state.wl_surface.as_ref() {
                compositor::remove_pre_commit_hook(wl_surface, &hook_id);
            }
        }
    }
}

fn add_xsync_value(value: Int64, amount: i64) -> Int64 {
    let value = xsync_value(value);
    let value = value.wrapping_add(amount);
    value_to_xsync(value)
}

fn value_to_xsync(value: i64) -> Int64 {
    Int64 {
        hi: ((value >> 32) & 0xffffffff) as i32,
        lo: (value & 0xffffffff) as u32,
    }
}

fn xsync_value(value: Int64) -> i64 {
    (((value.hi as u64) << 32) | (value.lo as u64)) as i64
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
                .get::<crate::wayland::xwayland_shell::XWaylandShellCachedState>()
                .current()
                .serial
        });

        window.wl_surface_serial() == serial
    }
}

impl IsAlive for X11Surface {
    #[inline]
    fn alive(&self) -> bool {
        X11Surface::alive(self)
    }
}

impl<D: SeatHandler + 'static> KeyboardTarget<D> for X11Surface {
    fn enter(&self, seat: &Seat<D>, data: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        let (set_input_focus, send_take_focus) = match self.input_model() {
            WmInputModel::None => return,
            WmInputModel::Passive => (true, false),
            WmInputModel::LocallyActive => (true, true),
            WmInputModel::GloballyActive => (false, true),
        };

        if let Some(release) = &self.focus_release {
            release.cancel();
        }

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

        let mut state = self.state.lock().unwrap();
        if let Some(surface) = state.wl_surface.as_ref() {
            KeyboardTarget::enter(surface, seat, data, keys, serial);
        } else {
            state.pending_enter = Some((
                Box::new(seat.clone()) as Box<dyn std::any::Any + Send>,
                keys.iter().map(|x| x.raw_code()).collect(),
                None,
                serial,
            ));
        }
    }

    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial) {
        if self.input_model() == WmInputModel::None {
            return;
        }

        if let Some(release) = &self.focus_release {
            release.schedule();
        }

        let mut state = self.state.lock().unwrap();
        let _ = state.pending_enter.take();
        if let Some(surface) = state.wl_surface.as_ref() {
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
        let mut xstate = self.state.lock().unwrap();
        if let Some(surface) = xstate.wl_surface.as_ref() {
            KeyboardTarget::key(surface, seat, data, key, state, serial, time)
        } else if let Some((_, keys, _, pending_serial)) = xstate.pending_enter.as_mut() {
            let raw = key.raw_code();
            if state == KeyState::Released {
                keys.retain(|c| *c != raw);
            } else {
                keys.push(raw);
            }
            *pending_serial = serial
        }
    }

    fn modifiers(&self, seat: &Seat<D>, data: &mut D, modifiers: ModifiersState, serial: Serial) {
        let mut state = self.state.lock().unwrap();
        if let Some(surface) = state.wl_surface.as_ref() {
            KeyboardTarget::modifiers(surface, seat, data, modifiers, serial);
        } else if let Some((_, _, pending_modifiers, pending_serial)) = state.pending_enter.as_mut() {
            *pending_modifiers = Some(modifiers);
            *pending_serial = serial;
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

impl WaylandFocus for X11Surface {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        X11Surface::wl_surface(self).map(Cow::Owned)
    }
}

fn fetch_opaque_regions(
    conn: &Arc<RustConnection>,
    window: X11Window,
    property_atom: Atom,
    client_scale: Option<&Arc<AtomicF64>>,
) -> Result<Option<RegionAttributes>, ConnectionError> {
    let reply = conn
        .get_property(false, window, property_atom, AtomEnum::CARDINAL, 0, u32::MAX)?
        .reply_unchecked()?;

    let opaque_region = reply
        .and_then(|reply| {
            reply.value32().map(|values| {
                let client_scale = client_scale
                    .as_ref()
                    .map(|s| s.load(Ordering::Acquire))
                    .unwrap_or(1.);

                values
                    .collect::<Vec<_>>()
                    .chunks_exact(4)
                    .flat_map(|rect_values| {
                        let phys_rect = Rectangle::<i32, Physical>::new(
                            (rect_values[0] as i32, rect_values[1] as i32).into(),
                            ((rect_values[2] as i32).max(0), (rect_values[3] as i32).max(0)).into(),
                        );
                        if phys_rect.is_empty() {
                            None
                        } else {
                            Some((
                                RectangleKind::Add,
                                phys_rect.to_f64().to_logical(client_scale).to_i32_round::<i32>(),
                            ))
                        }
                    })
                    .collect::<Vec<_>>()
            })
        })
        .map(|rects| RegionAttributes { rects });

    Ok(opaque_region)
}
