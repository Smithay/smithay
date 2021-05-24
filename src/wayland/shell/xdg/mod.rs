//! Utilities for handling shell surfaces with the `xdg_shell` protocol
//!
//! This module provides automatic handling of shell surfaces objects, by being registered
//! as a global handler for `xdg_shell`.
//!
//! ## Why use this implementation
//!
//! This implementation can track for you the various shell surfaces defined by the
//! clients by handling the `xdg_shell` protocol. It also contains a compatibility
//! layer handling its precursor, the unstable `zxdg_shell_v6` protocol, which is
//! mostly identical.
//!
//! It allows you to easily access a list of all shell surfaces defined by your clients
//! access their associated metadata and underlying `wl_surface`s.
//!
//! This handler only handles the protocol exchanges with the client to present you the
//! information in a coherent and relatively easy to use manner. All the actual drawing
//! and positioning logic of windows is out of its scope.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this handler, simple use the [`xdg_shell_init`] function provided in this
//! module. You will need to provide it the [`CompositorToken`](crate::wayland::compositor::CompositorToken)
//! you retrieved from an instantiation of the compositor global provided by smithay.
//!
//! ```no_run
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! # extern crate wayland_protocols;
//! #
//! use smithay::wayland::compositor::roles::*;
//! use smithay::wayland::compositor::CompositorToken;
//! use smithay::wayland::shell::xdg::{xdg_shell_init, XdgSurfaceRole, XdgRequest};
//! use wayland_protocols::unstable::xdg_shell::v6::server::zxdg_shell_v6::ZxdgShellV6;
//! # use wayland_server::protocol::{wl_seat, wl_output};
//!
//! // define the roles type. You need to integrate the XdgSurface role:
//! define_roles!(MyRoles =>
//!     [XdgSurface, XdgSurfaceRole]
//! );
//!
//! // define the metadata you want associated with the shell clients
//! #[derive(Default)]
//! struct MyShellData {
//!     /* ... */
//! }
//!
//! # let mut display = wayland_server::Display::new();
//! # let (compositor_token, _, _) = smithay::wayland::compositor::compositor_init::<MyRoles, _, _>(
//! #     &mut display,
//! #     |_, _, _| {},
//! #     None
//! # );
//! let (shell_state, _, _) = xdg_shell_init(
//!     &mut display,
//!     // token from the compositor implementation
//!     compositor_token,
//!     // your implementation
//!     |event: XdgRequest<_>| { /* ... */ },
//!     None  // put a logger if you want
//! );
//!
//! // You're now ready to go!
//! ```
//!
//! ### Access to shell surface and clients data
//!
//! There are mainly 3 kind of objects that you'll manipulate from this implementation:
//!
//! - [`ShellClient`]:
//!   This is a handle representing an instantiation of a shell global
//!   you can associate client-wise metadata to it (this is the `MyShellData` type in
//!   the example above).
//! - [`ToplevelSurface`]:
//!   This is a handle representing a toplevel surface, you can
//!   retrieve a list of all currently alive toplevel surface from the
//!   [`ShellState`].
//! - [`PopupSurface`]:
//!   This is a handle representing a popup/tooltip surface. Similarly,
//!   you can get a list of all currently alive popup surface from the
//!   [`ShellState`].
//!
//! You'll obtain these objects though two means: either via the callback methods of
//! the subhandler you provided, or via methods on the [`ShellState`]
//! that you are given (in an `Arc<Mutex<_>>`) as return value of the `init` function.

use crate::utils::Rectangle;
use crate::wayland::compositor::{roles::Role, CompositorToken};
use crate::wayland::Serial;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use wayland_protocols::{
    unstable::xdg_shell::v6::server::{zxdg_popup_v6, zxdg_shell_v6, zxdg_surface_v6, zxdg_toplevel_v6},
    xdg_shell::server::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base},
};
use wayland_server::{
    protocol::{wl_output, wl_seat, wl_surface},
    Display, Filter, Global, UserDataMap,
};

// handlers for the xdg_shell protocol
mod xdg_handlers;
// compatibility handlers for the zxdg_shell_v6 protocol, its earlier version
mod zxdgv6_handlers;

/// Metadata associated with the `xdg_surface` role
#[derive(Debug)]
pub struct XdgSurfaceRole {
    /// Pending state as requested by the client
    ///
    /// The data in this field are double-buffered, you should
    /// apply them on a surface commit.
    pub pending_state: XdgSurfacePendingState,
    /// Geometry of the surface
    ///
    /// Defines, in surface relative coordinates, what should
    /// be considered as "the surface itself", regarding focus,
    /// window alignment, etc...
    ///
    /// By default, you should consider the full contents of the
    /// buffers of this surface and its subsurfaces.
    pub window_geometry: Option<Rectangle>,
    /// List of non-acked configures pending
    ///
    /// Whenever a configure is acked by the client, all configure
    /// older than it are discarded as well. As such, this `Vec` contains
    /// the serials of all the configure send to this surface that are
    /// newer than the last ack received.
    pub pending_configures: Vec<u32>,
    /// Has this surface acked at least one configure?
    ///
    /// `xdg_shell` defines it as illegal to commit on a surface that has
    /// not yet acked a configure.
    pub configured: bool,
}

#[derive(Clone, Debug)]
/// The state of a positioner, as set by the client
pub struct PositionerState {
    /// Size of the rectangle that needs to be positioned
    pub rect_size: (i32, i32),
    /// Anchor rectangle in the parent surface coordinates
    /// relative to which the surface must be positioned
    pub anchor_rect: Rectangle,
    /// Edges defining the anchor point
    pub anchor_edges: xdg_positioner::Anchor,
    /// Gravity direction for positioning the child surface
    /// relative to its anchor point
    pub gravity: xdg_positioner::Gravity,
    /// Adjustments to do if previous criteria constrain the
    /// surface
    pub constraint_adjustment: xdg_positioner::ConstraintAdjustment,
    /// Offset placement relative to the anchor point
    pub offset: (i32, i32),
}

impl PositionerState {
    pub(crate) fn new() -> PositionerState {
        PositionerState {
            rect_size: (0, 0),
            anchor_rect: Rectangle {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            anchor_edges: xdg_positioner::Anchor::None,
            gravity: xdg_positioner::Gravity::None,
            constraint_adjustment: xdg_positioner::ConstraintAdjustment::None,
            offset: (0, 0),
        }
    }
}

/// Contents of the pending state of a shell surface, depending on its role
#[derive(Debug)]
pub enum XdgSurfacePendingState {
    /// This a regular, toplevel surface
    ///
    /// This corresponds to the `xdg_toplevel` role
    ///
    /// This is what you'll generally interpret as "a window".
    Toplevel(ToplevelState),
    /// This is a popup surface
    ///
    /// This corresponds to the `xdg_popup` role
    ///
    /// This are mostly for small tooltips and similar short-lived
    /// surfaces.
    Popup(PopupState),
    /// This surface was not yet assigned a kind
    None,
}

/// State of a regular toplevel surface
#[derive(Debug)]
pub struct ToplevelState {
    /// Parent of this surface
    ///
    /// If this surface has a parent, it should be hidden
    /// or displayed, brought up at the same time as it.
    pub parent: Option<wl_surface::WlSurface>,
    /// Title of this shell surface
    pub title: String,
    /// App id for this shell surface
    ///
    /// This identifier can be used to group surface together
    /// as being several instance of the same app. This can
    /// also be used as the D-Bus name for the app.
    pub app_id: String,
    /// Minimum size requested for this surface
    ///
    /// A value of 0 on an axis means this axis is not constrained
    pub min_size: (i32, i32),
    /// Maximum size requested for this surface
    ///
    /// A value of 0 on an axis means this axis is not constrained
    pub max_size: (i32, i32),
}

impl Clone for ToplevelState {
    fn clone(&self) -> ToplevelState {
        ToplevelState {
            parent: self.parent.as_ref().cloned(),
            title: self.title.clone(),
            app_id: self.app_id.clone(),
            min_size: self.min_size,
            max_size: self.max_size,
        }
    }
}

/// The pending state of a popup surface
#[derive(Debug)]
pub struct PopupState {
    /// Parent of this popup surface
    pub parent: Option<wl_surface::WlSurface>,
    /// The positioner specifying how this tooltip should
    /// be placed relative to its parent.
    pub positioner: PositionerState,
}

impl Clone for PopupState {
    fn clone(&self) -> PopupState {
        PopupState {
            parent: self.parent.as_ref().cloned(),
            positioner: self.positioner.clone(),
        }
    }
}

impl Default for XdgSurfacePendingState {
    fn default() -> XdgSurfacePendingState {
        XdgSurfacePendingState::None
    }
}

pub(crate) struct ShellData<R> {
    log: ::slog::Logger,
    compositor_token: CompositorToken<R>,
    user_impl: Rc<RefCell<dyn FnMut(XdgRequest<R>)>>,
    shell_state: Arc<Mutex<ShellState<R>>>,
}

impl<R> Clone for ShellData<R> {
    fn clone(&self) -> Self {
        ShellData {
            log: self.log.clone(),
            compositor_token: self.compositor_token,
            user_impl: self.user_impl.clone(),
            shell_state: self.shell_state.clone(),
        }
    }
}

/// Create a new `xdg_shell` globals
pub fn xdg_shell_init<R, L, Impl>(
    display: &mut Display,
    ctoken: CompositorToken<R>,
    implementation: Impl,
    logger: L,
) -> (
    Arc<Mutex<ShellState<R>>>,
    Global<xdg_wm_base::XdgWmBase>,
    Global<zxdg_shell_v6::ZxdgShellV6>,
)
where
    R: Role<XdgSurfaceRole> + 'static,
    L: Into<Option<::slog::Logger>>,
    Impl: FnMut(XdgRequest<R>) + 'static,
{
    let log = crate::slog_or_fallback(logger);
    let shell_state = Arc::new(Mutex::new(ShellState {
        known_toplevels: Vec::new(),
        known_popups: Vec::new(),
    }));

    let shell_data = ShellData {
        log: log.new(o!("smithay_module" => "xdg_shell_handler")),
        compositor_token: ctoken,
        user_impl: Rc::new(RefCell::new(implementation)),
        shell_state: shell_state.clone(),
    };

    let shell_data_z = shell_data.clone();

    let xdg_shell_global = display.create_global(
        1,
        Filter::new(move |(shell, _version), _, _data| {
            self::xdg_handlers::implement_wm_base(shell, &shell_data);
        }),
    );

    let zxdgv6_shell_global = display.create_global(
        1,
        Filter::new(move |(shell, _version), _, _data| {
            self::zxdgv6_handlers::implement_shell(shell, &shell_data_z);
        }),
    );

    (shell_state, xdg_shell_global, zxdgv6_shell_global)
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
#[derive(Debug)]
pub struct ShellState<R> {
    known_toplevels: Vec<ToplevelSurface<R>>,
    known_popups: Vec<PopupSurface<R>>,
}

impl<R> ShellState<R>
where
    R: Role<XdgSurfaceRole> + 'static,
{
    /// Access all the shell surfaces known by this handler
    pub fn toplevel_surfaces(&self) -> &[ToplevelSurface<R>] {
        &self.known_toplevels[..]
    }

    /// Access all the popup surfaces known by this handler
    pub fn popup_surfaces(&self) -> &[PopupSurface<R>] {
        &self.known_popups[..]
    }
}

/*
 * User interaction
 */

#[derive(Debug)]
enum ShellClientKind {
    Xdg(xdg_wm_base::XdgWmBase),
    ZxdgV6(zxdg_shell_v6::ZxdgShellV6),
}

pub(crate) struct ShellClientData {
    pending_ping: Serial,
    data: UserDataMap,
}

fn make_shell_client_data() -> ShellClientData {
    ShellClientData {
        pending_ping: Serial::from(0),
        data: UserDataMap::new(),
    }
}

/// A shell client
///
/// This represents an instantiation of a shell
/// global (be it `wl_shell` or `xdg_shell`).
///
/// Most of the time, you can consider that a
/// Wayland client will be a single shell client.
///
/// You can use this handle to access a storage for any
/// client-specific data you wish to associate with it.
#[derive(Debug)]
pub struct ShellClient<R> {
    kind: ShellClientKind,
    _token: CompositorToken<R>,
}

impl<R> ShellClient<R>
where
    R: Role<XdgSurfaceRole> + 'static,
{
    /// Is the shell client represented by this handle still connected?
    pub fn alive(&self) -> bool {
        match self.kind {
            ShellClientKind::Xdg(ref s) => s.as_ref().is_alive(),
            ShellClientKind::ZxdgV6(ref s) => s.as_ref().is_alive(),
        }
    }

    /// Checks if this handle and the other one actually refer to the
    /// same shell client
    pub fn equals(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (&ShellClientKind::Xdg(ref s1), &ShellClientKind::Xdg(ref s2)) => s1.as_ref().equals(s2.as_ref()),
            (&ShellClientKind::ZxdgV6(ref s1), &ShellClientKind::ZxdgV6(ref s2)) => {
                s1.as_ref().equals(s2.as_ref())
            }
            _ => false,
        }
    }

    /// Send a ping request to this shell client
    ///
    /// You'll receive the reply as a [`XdgRequest::ClientPong`] request.
    ///
    /// A typical use is to start a timer at the same time you send this ping
    /// request, and cancel it when you receive the pong. If the timer runs
    /// down to 0 before a pong is received, mark the client as unresponsive.
    ///
    /// Fails if this shell client already has a pending ping or is already dead.
    pub fn send_ping(&self, serial: Serial) -> Result<(), ()> {
        if !self.alive() {
            return Err(());
        }
        match self.kind {
            ShellClientKind::Xdg(ref shell) => {
                let user_data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellUserData<R>>()
                    .unwrap();
                let mut guard = user_data.client_data.lock().unwrap();
                if guard.pending_ping == Serial::from(0) {
                    return Err(());
                }
                guard.pending_ping = serial;
                shell.ping(serial.into());
            }
            ShellClientKind::ZxdgV6(ref shell) => {
                let user_data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellUserData<R>>()
                    .unwrap();
                let mut guard = user_data.client_data.lock().unwrap();
                if guard.pending_ping == Serial::from(0) {
                    return Err(());
                }
                guard.pending_ping = serial;
                shell.ping(serial.into());
            }
        }
        Ok(())
    }

    /// Access the user data associated with this shell client
    pub fn with_data<F, T>(&self, f: F) -> Result<T, ()>
    where
        F: FnOnce(&mut UserDataMap) -> T,
    {
        if !self.alive() {
            return Err(());
        }
        match self.kind {
            ShellClientKind::Xdg(ref shell) => {
                let data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellUserData<R>>()
                    .unwrap();
                let mut guard = data.client_data.lock().unwrap();
                Ok(f(&mut guard.data))
            }
            ShellClientKind::ZxdgV6(ref shell) => {
                let data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellUserData<R>>()
                    .unwrap();
                let mut guard = data.client_data.lock().unwrap();
                Ok(f(&mut guard.data))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ToplevelKind {
    Xdg(xdg_toplevel::XdgToplevel),
    ZxdgV6(zxdg_toplevel_v6::ZxdgToplevelV6),
}

/// A handle to a toplevel surface
#[derive(Debug)]
pub struct ToplevelSurface<R> {
    wl_surface: wl_surface::WlSurface,
    shell_surface: ToplevelKind,
    token: CompositorToken<R>,
}

// We implement Clone manually because #[derive(..)] would require R: Clone.
impl<R> Clone for ToplevelSurface<R> {
    fn clone(&self) -> Self {
        Self {
            wl_surface: self.wl_surface.clone(),
            shell_surface: self.shell_surface.clone(),
            token: self.token,
        }
    }
}

impl<R> ToplevelSurface<R>
where
    R: Role<XdgSurfaceRole> + 'static,
{
    /// Is the toplevel surface referred by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_alive = match self.shell_surface {
            ToplevelKind::Xdg(ref s) => s.as_ref().is_alive(),
            ToplevelKind::ZxdgV6(ref s) => s.as_ref().is_alive(),
        };
        shell_alive && self.wl_surface.as_ref().is_alive()
    }

    /// Do this handle and the other one actually refer to the same toplevel surface?
    pub fn equals(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface.as_ref().equals(&other.wl_surface.as_ref())
    }

    /// Retrieve the shell client owning this toplevel surface
    ///
    /// Returns `None` if the surface does actually no longer exist.
    pub fn client(&self) -> Option<ShellClient<R>> {
        if !self.alive() {
            return None;
        }

        let shell = match self.shell_surface {
            ToplevelKind::Xdg(ref s) => {
                let data = s
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellSurfaceUserData<R>>()
                    .unwrap();
                ShellClientKind::Xdg(data.wm_base.clone())
            }
            ToplevelKind::ZxdgV6(ref s) => {
                let data = s
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellSurfaceUserData<R>>()
                    .unwrap();
                ShellClientKind::ZxdgV6(data.shell.clone())
            }
        };

        Some(ShellClient {
            kind: shell,
            _token: self.token,
        })
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    pub fn send_configure(&self, cfg: ToplevelConfigure) {
        if !self.alive() {
            return;
        }
        match self.shell_surface {
            ToplevelKind::Xdg(ref s) => self::xdg_handlers::send_toplevel_configure::<R>(s, cfg),
            ToplevelKind::ZxdgV6(ref s) => self::zxdgv6_handlers::send_toplevel_configure::<R>(s, cfg),
        }
    }

    /// Make sure this surface was configured
    ///
    /// Returns `true` if it was, if not, returns `false` and raise
    /// a protocol error to the associated client. Also returns `false`
    /// if the surface is already destroyed.
    ///
    /// `xdg_shell` mandates that a client acks a configure before committing
    /// anything.
    pub fn ensure_configured(&self) -> bool {
        if !self.alive() {
            return false;
        }
        let configured = self
            .token
            .with_role_data::<XdgSurfaceRole, _, _>(&self.wl_surface, |data| data.configured)
            .expect("A shell surface object exists but the surface does not have the shell_surface role ?!");
        if !configured {
            match self.shell_surface {
                ToplevelKind::Xdg(ref s) => {
                    let data = s
                        .as_ref()
                        .user_data()
                        .get::<self::xdg_handlers::ShellSurfaceUserData<R>>()
                        .unwrap();
                    data.xdg_surface.as_ref().post_error(
                        xdg_surface::Error::NotConstructed as u32,
                        "Surface has not been configured yet.".into(),
                    );
                }
                ToplevelKind::ZxdgV6(ref s) => {
                    let data = s
                        .as_ref()
                        .user_data()
                        .get::<self::zxdgv6_handlers::ShellSurfaceUserData<R>>()
                        .unwrap();
                    data.xdg_surface.as_ref().post_error(
                        zxdg_surface_v6::Error::NotConstructed as u32,
                        "Surface has not been configured yet.".into(),
                    );
                }
            }
        }
        configured
    }

    /// Send a "close" event to the client
    pub fn send_close(&self) {
        match self.shell_surface {
            ToplevelKind::Xdg(ref s) => s.close(),
            ToplevelKind::ZxdgV6(ref s) => s.close(),
        }
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    ///
    /// Returns `None` if the toplevel surface actually no longer exists.
    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        if self.alive() {
            Some(&self.wl_surface)
        } else {
            None
        }
    }

    /// Retrieve a copy of the pending state of this toplevel surface
    ///
    /// Returns `None` of the toplevel surface actually no longer exists.
    pub fn get_pending_state(&self) -> Option<ToplevelState> {
        if !self.alive() {
            return None;
        }
        self.token
            .with_role_data::<XdgSurfaceRole, _, _>(&self.wl_surface, |data| match data.pending_state {
                XdgSurfacePendingState::Toplevel(ref state) => Some(state.clone()),
                _ => None,
            })
            .ok()
            .and_then(|x| x)
    }
}

#[derive(Debug)]
pub(crate) enum PopupKind {
    Xdg(xdg_popup::XdgPopup),
    ZxdgV6(zxdg_popup_v6::ZxdgPopupV6),
}

/// A handle to a popup surface
///
/// This is an unified abstraction over the popup surfaces
/// of both `wl_shell` and `xdg_shell`.
#[derive(Debug)]
pub struct PopupSurface<R> {
    wl_surface: wl_surface::WlSurface,
    shell_surface: PopupKind,
    token: CompositorToken<R>,
}

impl<R> PopupSurface<R>
where
    R: Role<XdgSurfaceRole> + 'static,
{
    /// Is the popup surface referred by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_alive = match self.shell_surface {
            PopupKind::Xdg(ref p) => p.as_ref().is_alive(),
            PopupKind::ZxdgV6(ref p) => p.as_ref().is_alive(),
        };
        shell_alive && self.wl_surface.as_ref().is_alive()
    }

    /// Do this handle and the other one actually refer to the same popup surface?
    pub fn equals(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface.as_ref().equals(&other.wl_surface.as_ref())
    }

    /// Retrieve the shell client owning this popup surface
    ///
    /// Returns `None` if the surface does actually no longer exist.
    pub fn client(&self) -> Option<ShellClient<R>> {
        if !self.alive() {
            return None;
        }

        let shell = match self.shell_surface {
            PopupKind::Xdg(ref p) => {
                let data = p
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellSurfaceUserData<R>>()
                    .unwrap();
                ShellClientKind::Xdg(data.wm_base.clone())
            }
            PopupKind::ZxdgV6(ref p) => {
                let data = p
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellSurfaceUserData<R>>()
                    .unwrap();
                ShellClientKind::ZxdgV6(data.shell.clone())
            }
        };

        Some(ShellClient {
            kind: shell,
            _token: self.token,
        })
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    pub fn send_configure(&self, cfg: PopupConfigure) {
        if !self.alive() {
            return;
        }
        match self.shell_surface {
            PopupKind::Xdg(ref p) => {
                self::xdg_handlers::send_popup_configure::<R>(p, cfg);
            }
            PopupKind::ZxdgV6(ref p) => {
                self::zxdgv6_handlers::send_popup_configure::<R>(p, cfg);
            }
        }
    }

    /// Make sure this surface was configured
    ///
    /// Returns `true` if it was, if not, returns `false` and raise
    /// a protocol error to the associated client. Also returns `false`
    /// if the surface is already destroyed.
    ///
    /// xdg_shell mandates that a client acks a configure before committing
    /// anything.
    pub fn ensure_configured(&self) -> bool {
        if !self.alive() {
            return false;
        }
        let configured = self
            .token
            .with_role_data::<XdgSurfaceRole, _, _>(&self.wl_surface, |data| data.configured)
            .expect("A shell surface object exists but the surface does not have the shell_surface role ?!");
        if !configured {
            match self.shell_surface {
                PopupKind::Xdg(ref s) => {
                    let data = s
                        .as_ref()
                        .user_data()
                        .get::<self::xdg_handlers::ShellSurfaceUserData<R>>()
                        .unwrap();
                    data.xdg_surface.as_ref().post_error(
                        xdg_surface::Error::NotConstructed as u32,
                        "Surface has not been configured yet.".into(),
                    );
                }
                PopupKind::ZxdgV6(ref s) => {
                    let data = s
                        .as_ref()
                        .user_data()
                        .get::<self::zxdgv6_handlers::ShellSurfaceUserData<R>>()
                        .unwrap();
                    data.xdg_surface.as_ref().post_error(
                        zxdg_surface_v6::Error::NotConstructed as u32,
                        "Surface has not been configured yet.".into(),
                    );
                }
            }
        }
        configured
    }

    /// Send a `popup_done` event to the popup surface
    ///
    /// It means that the use has dismissed the popup surface, or that
    /// the pointer has left the area of popup grab if there was a grab.
    pub fn send_popup_done(&self) {
        match self.shell_surface {
            PopupKind::Xdg(ref p) => p.popup_done(),
            PopupKind::ZxdgV6(ref p) => p.popup_done(),
        }
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    ///
    /// Returns `None` if the toplevel surface actually no longer exists.
    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        if self.alive() {
            Some(&self.wl_surface)
        } else {
            None
        }
    }

    /// Retrieve a copy of the pending state of this popup surface
    ///
    /// Returns `None` of the popup surface actually no longer exists.
    pub fn get_pending_state(&self) -> Option<PopupState> {
        if !self.alive() {
            return None;
        }
        self.token
            .with_role_data::<XdgSurfaceRole, _, _>(&self.wl_surface, |data| match data.pending_state {
                XdgSurfacePendingState::Popup(ref state) => Some(state.clone()),
                _ => None,
            })
            .ok()
            .and_then(|x| x)
    }
}

/// A configure message for toplevel surfaces
#[derive(Debug)]
pub struct ToplevelConfigure {
    /// A suggestion for a new size for the surface
    pub size: Option<(i32, i32)>,
    /// A notification of what are the current states of this surface
    ///
    /// A surface can be any combination of these possible states
    /// at the same time.
    pub states: Vec<xdg_toplevel::State>,
    /// A serial number to track ACK from the client
    ///
    /// This should be an ever increasing number, as the ACK-ing
    /// from a client for a serial will validate all pending lower
    /// serials.
    pub serial: Serial,
}

/// A configure message for popup surface
#[derive(Debug)]
pub struct PopupConfigure {
    /// The position chosen for this popup relative to
    /// its parent
    pub position: (i32, i32),
    /// A suggested size for the popup
    pub size: (i32, i32),
    /// A serial number to track ACK from the client
    ///
    /// This should be an ever increasing number, as the ACK-ing
    /// from a client for a serial will validate all pending lower
    /// serials.
    pub serial: Serial,
}

/// Events generated by xdg shell surfaces
///
/// These are events that the provided implementation cannot process
/// for you directly.
///
/// Depending on what you want to do, you might ignore some of them
#[derive(Debug)]
pub enum XdgRequest<R> {
    /// A new shell client was instantiated
    NewClient {
        /// the client
        client: ShellClient<R>,
    },
    /// The pong for a pending ping of this shell client was received
    ///
    /// The `ShellHandler` already checked for you that the serial matches the one
    /// from the pending ping.
    ClientPong {
        /// the client
        client: ShellClient<R>,
    },
    /// A new toplevel surface was created
    ///
    /// You likely need to send a [`ToplevelConfigure`] to the surface, to hint the
    /// client as to how its window should be
    NewToplevel {
        /// the surface
        surface: ToplevelSurface<R>,
    },
    /// A new popup surface was created
    ///
    /// You likely need to send a [`PopupConfigure`] to the surface, to hint the
    /// client as to how its popup should be
    NewPopup {
        /// the surface
        surface: PopupSurface<R>,
    },
    /// The client requested the start of an interactive move for this surface
    Move {
        /// the surface
        surface: ToplevelSurface<R>,
        /// the seat associated to this move
        seat: wl_seat::WlSeat,
        /// the grab serial
        serial: Serial,
    },
    /// The client requested the start of an interactive resize for this surface
    Resize {
        /// The surface
        surface: ToplevelSurface<R>,
        /// The seat associated with this resize
        seat: wl_seat::WlSeat,
        /// The grab serial
        serial: Serial,
        /// Specification of which part of the window's border is being dragged
        edges: xdg_toplevel::ResizeEdge,
    },
    /// This popup requests a grab of the pointer
    ///
    /// This means it requests to be sent a `popup_done` event when the pointer leaves
    /// the grab area.
    Grab {
        /// The surface
        surface: PopupSurface<R>,
        /// The seat to grab
        seat: wl_seat::WlSeat,
        /// The grab serial
        serial: Serial,
    },
    /// A toplevel surface requested to be maximized
    Maximize {
        /// The surface
        surface: ToplevelSurface<R>,
    },
    /// A toplevel surface requested to stop being maximized
    UnMaximize {
        /// The surface
        surface: ToplevelSurface<R>,
    },
    /// A toplevel surface requested to be set fullscreen
    Fullscreen {
        /// The surface
        surface: ToplevelSurface<R>,
        /// The output (if any) on which the fullscreen is requested
        output: Option<wl_output::WlOutput>,
    },
    /// A toplevel surface request to stop being fullscreen
    UnFullscreen {
        /// The surface
        surface: ToplevelSurface<R>,
    },
    /// A toplevel surface requested to be minimized
    Minimize {
        /// The surface
        surface: ToplevelSurface<R>,
    },
    /// The client requests the window menu to be displayed on this surface at this location
    ///
    /// This menu belongs to the compositor. It is typically expected to contain options for
    /// control of the window (maximize/minimize/close/move/etc...).
    ShowWindowMenu {
        /// The surface
        surface: ToplevelSurface<R>,
        /// The seat associated with this input grab
        seat: wl_seat::WlSeat,
        /// the grab serial
        serial: Serial,
        /// location of the menu request
        location: (i32, i32),
    },
    /// A surface has acknowledged a configure serial.
    AckConfigure {
        /// The surface.
        surface: wl_surface::WlSurface,
        /// The configure serial.
        serial: Serial,
    },
}
