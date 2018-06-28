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
//! information in a coherent and relatively easy to use maneer. All the actual drawing
//! and positioning logic of windows is out of its scope.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this handler, simple use the `xdg_shell_init` function provided in this
//! module. You will need to provide it the `CompositorToken` you retrieved from an
//! instanciation of the compositor global provided by smithay.
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
//! use wayland_server::{EventLoop, LoopToken};
//! # use wayland_server::protocol::{wl_seat, wl_output};
//! # #[derive(Default)] struct MySurfaceData;
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
//! # fn main() {
//! # let (mut display, event_loop) = wayland_server::Display::new();
//! # let (compositor_token, _, _) = smithay::wayland::compositor::compositor_init::<(), MyRoles, _, _>(
//! #     &mut display,
//! #     event_loop.token(),
//! #     |_, _| {},
//! #     None
//! # );
//! let (shell_state, _, _) = xdg_shell_init(
//!     &mut display,
//!     event_loop.token(),
//!     // token from the compositor implementation
//!     compositor_token,
//!     // your implementation, can also be a strucy implementing the
//!     // appropriate Implementation<(), XdgRequest<_, _, _>> trait
//!     |event: XdgRequest<_, _, MyShellData>, ()| { /* ... */ },
//!     None  // put a logger if you want
//! );
//!
//! // You're now ready to go!
//! # }
//! ```
//!
//! ### Access to shell surface and clients data
//!
//! There are mainly 3 kind of objects that you'll manipulate from this implementation:
//!
//! - `ShellClient`: This is a handle representing an isntanciation of a shell global
//!   you can associate client-wise metadata to it (this is the `MyShellData` type in
//!   the example above).
//! - `ToplevelSurface`: This is a handle representing a toplevel surface, you can
//!   retrive a list of all currently alive toplevel surface from the `ShellState`.
//! - `PopupSurface`: This is a handle representing a popup/tooltip surface. Similarly,
//!   you can get a list of all currently alive popup surface from the `ShellState`.
//!
//! You'll obtain these objects though two means: either via the callback methods of
//! the subhandler you provided, or via methods on the `ShellState` that you are given
//! (in an `Arc<Mutex<_>>`) as return value of the init function.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use utils::Rectangle;
use wayland::compositor::CompositorToken;
use wayland::compositor::roles::Role;
use wayland_protocols::xdg_shell::server::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols::unstable::xdg_shell::v6::server::{zxdg_popup_v6, zxdg_shell_v6, zxdg_surface_v6,
                                                         zxdg_toplevel_v6};
use wayland_server::{Display, Global, LoopToken, Resource};
use wayland_server::commons::Implementation;
use wayland_server::protocol::{wl_output, wl_seat, wl_surface};

// handlers for the xdg_shell protocol
mod xdg_handlers;
// compatibility handlers for the zxdg_shell_v6 protocol, its earlier version
mod zxdgv6_handlers;

/// Metadata associated with the `xdg_surface` role
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
    /// older than it are discarded as well. As such, this vec contains
    /// the serials of all the configure send to this surface that are
    /// newer than the last ack received.
    pub pending_configures: Vec<u32>,
    /// Has this surface acked at least one configure?
    ///
    /// xdg_shell defines it as illegal to commit on a surface that has
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
    /// Adjustments to do if previous criterias constraint the
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
pub enum XdgSurfacePendingState {
    /// This a regular, toplevel surface
    ///
    /// This corresponds to the `xdg_toplevel` role
    ///
    /// This is what you'll generaly interpret as "a window".
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
pub struct ToplevelState {
    /// Parent of this surface
    ///
    /// If this surface has a parent, it should be hidden
    /// or displayed, brought up at the same time as it.
    pub parent: Option<Resource<wl_surface::WlSurface>>,
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
pub struct PopupState {
    /// Parent of this popup surface
    pub parent: Option<Resource<wl_surface::WlSurface>>,
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

pub(crate) struct ShellImplementation<U, R, SD> {
    log: ::slog::Logger,
    compositor_token: CompositorToken<U, R>,
    loop_token: LoopToken,
    user_impl: Rc<RefCell<Implementation<(), XdgRequest<U, R, SD>>>>,
    shell_state: Arc<Mutex<ShellState<U, R, SD>>>,
}

impl<U, R, SD> Clone for ShellImplementation<U, R, SD> {
    fn clone(&self) -> Self {
        ShellImplementation {
            log: self.log.clone(),
            compositor_token: self.compositor_token,
            loop_token: self.loop_token.clone(),
            user_impl: self.user_impl.clone(),
            shell_state: self.shell_state.clone(),
        }
    }
}

/// Create a new `xdg_shell` globals
pub fn xdg_shell_init<U, R, SD, L, Impl>(
    display: &mut Display,
    ltoken: LoopToken,
    ctoken: CompositorToken<U, R>,
    implementation: Impl,
    logger: L,
) -> (
    Arc<Mutex<ShellState<U, R, SD>>>,
    Global<xdg_wm_base::XdgWmBase>,
    Global<zxdg_shell_v6::ZxdgShellV6>,
)
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: Default + 'static,
    L: Into<Option<::slog::Logger>>,
    Impl: Implementation<(), XdgRequest<U, R, SD>>,
{
    let log = ::slog_or_stdlog(logger);
    let shell_state = Arc::new(Mutex::new(ShellState {
        known_toplevels: Vec::new(),
        known_popups: Vec::new(),
    }));

    let shell_impl = ShellImplementation {
        log: log.new(o!("smithay_module" => "xdg_shell_handler")),
        loop_token: ltoken.clone(),
        compositor_token: ctoken,
        user_impl: Rc::new(RefCell::new(implementation)),
        shell_state: shell_state.clone(),
    };

    let shell_impl_z = shell_impl.clone();

    let xdg_shell_global = display.create_global(&ltoken, 1, move |_version, shell| {
        self::xdg_handlers::implement_wm_base(shell, &shell_impl);
    });

    let zxdgv6_shell_global = display.create_global(&ltoken, 1, move |_version, shell| {
        self::zxdgv6_handlers::implement_shell(shell, &shell_impl_z);
    });

    (shell_state, xdg_shell_global, zxdgv6_shell_global)
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
pub struct ShellState<U, R, SD> {
    known_toplevels: Vec<ToplevelSurface<U, R, SD>>,
    known_popups: Vec<PopupSurface<U, R, SD>>,
}

impl<U, R, SD> ShellState<U, R, SD>
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    /// Access all the shell surfaces known by this handler
    pub fn toplevel_surfaces(&self) -> &[ToplevelSurface<U, R, SD>] {
        &self.known_toplevels[..]
    }

    /// Access all the popup surfaces known by this handler
    pub fn popup_surfaces(&self) -> &[PopupSurface<U, R, SD>] {
        &self.known_popups[..]
    }
}

/*
 * User interaction
 */

enum ShellClientKind {
    Xdg(Resource<xdg_wm_base::XdgWmBase>),
    ZxdgV6(Resource<zxdg_shell_v6::ZxdgShellV6>),
}

pub(crate) struct ShellClientData<SD> {
    pending_ping: u32,
    data: SD,
}

fn make_shell_client_data<SD: Default>() -> ShellClientData<SD> {
    ShellClientData {
        pending_ping: 0,
        data: Default::default(),
    }
}

/// A shell client
///
/// This represents an instanciation of a shell
/// global (be it `wl_shell` or `xdg_shell`).
///
/// Most of the time, you can consider that a
/// wayland client will be a single shell client.
///
/// You can use this handle to access a storage for any
/// client-specific data you wish to associate with it.
pub struct ShellClient<SD> {
    kind: ShellClientKind,
    _data: ::std::marker::PhantomData<*mut SD>,
}

impl<SD> ShellClient<SD> {
    /// Is the shell client represented by this handle still connected?
    pub fn alive(&self) -> bool {
        match self.kind {
            ShellClientKind::Xdg(ref s) => s.is_alive(),
            ShellClientKind::ZxdgV6(ref s) => s.is_alive(),
        }
    }

    /// Checks if this handle and the other one actually refer to the
    /// same shell client
    pub fn equals(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (&ShellClientKind::Xdg(ref s1), &ShellClientKind::Xdg(ref s2)) => s1.equals(s2),
            (&ShellClientKind::ZxdgV6(ref s1), &ShellClientKind::ZxdgV6(ref s2)) => s1.equals(s2),
            _ => false,
        }
    }

    /// Send a ping request to this shell client
    ///
    /// You'll receive the reply as a `XdgRequest::ClientPong` request.
    ///
    /// A typical use is to start a timer at the same time you send this ping
    /// request, and cancel it when you receive the pong. If the timer runs
    /// down to 0 before a pong is received, mark the client as unresponsive.
    ///
    /// Fails if this shell client already has a pending ping or is already dead.
    pub fn send_ping(&self, serial: u32) -> Result<(), ()> {
        if !self.alive() {
            return Err(());
        }
        match self.kind {
            ShellClientKind::Xdg(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::xdg_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.pending_ping == 0 {
                    return Err(());
                }
                guard.pending_ping = serial;
                shell.send(xdg_wm_base::Event::Ping { serial });
            }
            ShellClientKind::ZxdgV6(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::zxdgv6_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.pending_ping == 0 {
                    return Err(());
                }
                guard.pending_ping = serial;
                shell.send(zxdg_shell_v6::Event::Ping { serial });
            }
        }
        Ok(())
    }

    /// Access the user data associated with this shell client
    pub fn with_data<F, T>(&self, f: F) -> Result<T, ()>
    where
        F: FnOnce(&mut SD) -> T,
    {
        if !self.alive() {
            return Err(());
        }
        match self.kind {
            ShellClientKind::Xdg(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::xdg_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                Ok(f(&mut guard.data))
            }
            ShellClientKind::ZxdgV6(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::zxdgv6_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                Ok(f(&mut guard.data))
            }
        }
    }
}

pub(crate) enum ToplevelKind {
    Xdg(Resource<xdg_toplevel::XdgToplevel>),
    ZxdgV6(Resource<zxdg_toplevel_v6::ZxdgToplevelV6>),
}

/// A handle to a toplevel surface
pub struct ToplevelSurface<U, R, SD> {
    wl_surface: Resource<wl_surface::WlSurface>,
    shell_surface: ToplevelKind,
    token: CompositorToken<U, R>,
    _shell_data: ::std::marker::PhantomData<SD>,
}

impl<U, R, SD> ToplevelSurface<U, R, SD>
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    /// Is the toplevel surface refered by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_alive = match self.shell_surface {
            ToplevelKind::Xdg(ref s) => s.is_alive(),
            ToplevelKind::ZxdgV6(ref s) => s.is_alive(),
        };
        shell_alive && self.wl_surface.is_alive()
    }

    /// Do this handle and the other one actually refer to the same toplevel surface?
    pub fn equals(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface.equals(&other.wl_surface)
    }

    /// Retrieve the shell client owning this toplevel surface
    ///
    /// Returns `None` if the surface does actually no longer exist.
    pub fn client(&self) -> Option<ShellClient<SD>> {
        if !self.alive() {
            return None;
        }

        let shell = match self.shell_surface {
            ToplevelKind::Xdg(ref s) => {
                let &(_, ref shell, _) =
                    unsafe { &*(s.get_user_data() as *mut self::xdg_handlers::ShellSurfaceUserData) };
                ShellClientKind::Xdg(shell.clone())
            }
            ToplevelKind::ZxdgV6(ref s) => {
                let &(_, ref shell, _) =
                    unsafe { &*(s.get_user_data() as *mut self::zxdgv6_handlers::ShellSurfaceUserData) };
                ShellClientKind::ZxdgV6(shell.clone())
            }
        };

        Some(ShellClient {
            kind: shell,
            _data: ::std::marker::PhantomData,
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
            ToplevelKind::Xdg(ref s) => self::xdg_handlers::send_toplevel_configure(self.token, s, cfg),
            ToplevelKind::ZxdgV6(ref s) => self::zxdgv6_handlers::send_toplevel_configure(self.token, s, cfg),
        }
    }

    /// Make sure this surface was configured
    ///
    /// Returns `true` if it was, if not, returns `false` and raise
    /// a protocol error to the associated client. Also returns `false`
    /// if the surface is already destroyed.
    ///
    /// xdg_shell mandates that a client acks a configure before commiting
    /// anything.
    pub fn ensure_configured(&self) -> bool {
        if !self.alive() {
            return false;
        }
        let configured = self.token
            .with_role_data::<XdgSurfaceRole, _, _>(&self.wl_surface, |data| data.configured)
            .expect("A shell surface object exists but the surface does not have the shell_surface role ?!");
        if !configured {
            match self.shell_surface {
                ToplevelKind::Xdg(ref s) => {
                    let ptr = s.get_user_data();
                    let &(_, _, ref xdg_surface) =
                        unsafe { &*(ptr as *mut self::xdg_handlers::ShellSurfaceUserData) };
                    xdg_surface.post_error(
                        xdg_surface::Error::NotConstructed as u32,
                        "Surface has not been configured yet.".into(),
                    );
                }
                ToplevelKind::ZxdgV6(ref s) => {
                    let ptr = s.get_user_data();
                    let &(_, _, ref xdg_surface) =
                        unsafe { &*(ptr as *mut self::zxdgv6_handlers::ShellSurfaceUserData) };
                    xdg_surface.post_error(
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
            ToplevelKind::Xdg(ref s) => s.send(xdg_toplevel::Event::Close),
            ToplevelKind::ZxdgV6(ref s) => s.send(zxdg_toplevel_v6::Event::Close),
        }
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    ///
    /// Returns `None` if the toplevel surface actually no longer exists.
    pub fn get_surface(&self) -> Option<&Resource<wl_surface::WlSurface>> {
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

pub(crate) enum PopupKind {
    Xdg(Resource<xdg_popup::XdgPopup>),
    ZxdgV6(Resource<zxdg_popup_v6::ZxdgPopupV6>),
}

/// A handle to a popup surface
///
/// This is an unified abstraction over the popup surfaces
/// of both `wl_shell` and `xdg_shell`.
pub struct PopupSurface<U, R, SD> {
    wl_surface: Resource<wl_surface::WlSurface>,
    shell_surface: PopupKind,
    token: CompositorToken<U, R>,
    _shell_data: ::std::marker::PhantomData<SD>,
}

impl<U, R, SD> PopupSurface<U, R, SD>
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    /// Is the popup surface refered by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_alive = match self.shell_surface {
            PopupKind::Xdg(ref p) => p.is_alive(),
            PopupKind::ZxdgV6(ref p) => p.is_alive(),
        };
        shell_alive && self.wl_surface.is_alive()
    }

    /// Do this handle and the other one actually refer to the same popup surface?
    pub fn equals(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface.equals(&other.wl_surface)
    }

    /// Retrieve the shell client owning this popup surface
    ///
    /// Returns `None` if the surface does actually no longer exist.
    pub fn client(&self) -> Option<ShellClient<SD>> {
        if !self.alive() {
            return None;
        }

        let shell = match self.shell_surface {
            PopupKind::Xdg(ref p) => {
                let &(_, ref shell, _) =
                    unsafe { &*(p.get_user_data() as *mut self::xdg_handlers::ShellSurfaceUserData) };
                ShellClientKind::Xdg(shell.clone())
            }
            PopupKind::ZxdgV6(ref p) => {
                let &(_, ref shell, _) =
                    unsafe { &*(p.get_user_data() as *mut self::zxdgv6_handlers::ShellSurfaceUserData) };
                ShellClientKind::ZxdgV6(shell.clone())
            }
        };

        Some(ShellClient {
            kind: shell,
            _data: ::std::marker::PhantomData,
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
                self::xdg_handlers::send_popup_configure(self.token, p, cfg);
            }
            PopupKind::ZxdgV6(ref p) => {
                self::zxdgv6_handlers::send_popup_configure(self.token, p, cfg);
            }
        }
    }

    /// Make sure this surface was configured
    ///
    /// Returns `true` if it was, if not, returns `false` and raise
    /// a protocol error to the associated client. Also returns `false`
    /// if the surface is already destroyed.
    ///
    /// xdg_shell mandates that a client acks a configure before commiting
    /// anything.
    pub fn ensure_configured(&self) -> bool {
        if !self.alive() {
            return false;
        }
        let configured = self.token
            .with_role_data::<XdgSurfaceRole, _, _>(&self.wl_surface, |data| data.configured)
            .expect("A shell surface object exists but the surface does not have the shell_surface role ?!");
        if !configured {
            match self.shell_surface {
                PopupKind::Xdg(ref s) => {
                    let ptr = s.get_user_data();
                    let &(_, _, ref xdg_surface) =
                        unsafe { &*(ptr as *mut self::xdg_handlers::ShellSurfaceUserData) };
                    xdg_surface.post_error(
                        xdg_surface::Error::NotConstructed as u32,
                        "Surface has not been confgured yet.".into(),
                    );
                }
                PopupKind::ZxdgV6(ref s) => {
                    let ptr = s.get_user_data();
                    let &(_, _, ref xdg_surface) =
                        unsafe { &*(ptr as *mut self::zxdgv6_handlers::ShellSurfaceUserData) };
                    xdg_surface.post_error(
                        zxdg_surface_v6::Error::NotConstructed as u32,
                        "Surface has not been confgured yet.".into(),
                    );
                }
            }
        }
        configured
    }

    /// Send a 'popup_done' event to the popup surface
    ///
    /// It means that the use has dismissed the popup surface, or that
    /// the pointer has left the area of popup grab if there was a grab.
    pub fn send_popup_done(&self) {
        match self.shell_surface {
            PopupKind::Xdg(ref p) => p.send(xdg_popup::Event::PopupDone),
            PopupKind::ZxdgV6(ref p) => p.send(zxdg_popup_v6::Event::PopupDone),
        }
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    ///
    /// Returns `None` if the toplevel surface actually no longer exists.
    pub fn get_surface(&self) -> Option<&Resource<wl_surface::WlSurface>> {
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
    pub serial: u32,
}

/// A configure message for popup surface
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
    pub serial: u32,
}

/// Events generated by xdg shell surfaces
///
/// These are events that the provided implementation cannot process
/// for you directly.
///
/// Depending on what you want to do, you might ignore some of them
pub enum XdgRequest<U, R, SD> {
    /// A new shell client was instanciated
    NewClient {
        /// the client
        client: ShellClient<SD>,
    },
    /// The pong for a pending ping of this shell client was received
    ///
    /// The ShellHandler already checked for you that the serial matches the one
    /// from the pending ping.
    ClientPong {
        /// the client
        client: ShellClient<SD>,
    },
    /// A new toplevel surface was created
    ///
    /// You likely need to send a `ToplevelConfigure` to the surface, to hint the
    /// client as to how its window should be
    NewToplevel {
        /// the surface
        surface: ToplevelSurface<U, R, SD>,
    },
    /// A new popup surface was created
    ///
    /// You likely need to send a `PopupConfigure` to the surface, to hint the
    /// client as to how its popup should be
    NewPopup {
        /// the surface
        surface: PopupSurface<U, R, SD>,
    },
    /// The client requested the start of an interactive move for this surface
    Move {
        /// the surface
        surface: ToplevelSurface<U, R, SD>,
        /// the seat associated to this move
        seat: Resource<wl_seat::WlSeat>,
        /// the grab serial
        serial: u32,
    },
    /// The client requested the start of an interactive resize for this surface
    Resize {
        /// The surface
        surface: ToplevelSurface<U, R, SD>,
        /// The seat associated with this resize
        seat: Resource<wl_seat::WlSeat>,
        /// The grab serial
        serial: u32,
        /// Specification of which part of the window's border is being dragged
        edges: xdg_toplevel::ResizeEdge,
    },
    /// This popup requests a grab of the pointer
    ///
    /// This means it requests to be sent a `popup_done` event when the pointer leaves
    /// the grab area.
    Grab {
        /// The surface
        surface: PopupSurface<U, R, SD>,
        /// The seat to grab
        seat: Resource<wl_seat::WlSeat>,
        /// The grab serial
        serial: u32,
    },
    /// A toplevel surface requested to be maximized
    Maximize {
        /// The surface
        surface: ToplevelSurface<U, R, SD>,
    },
    /// A toplevel surface requested to stop being maximized
    UnMaximize {
        /// The surface
        surface: ToplevelSurface<U, R, SD>,
    },
    /// A toplevel surface requested to be set fullscreen
    Fullscreen {
        /// The surface
        surface: ToplevelSurface<U, R, SD>,
        /// The output (if any) on which the fullscreen is requested
        output: Option<Resource<wl_output::WlOutput>>,
    },
    /// A toplevel surface request to stop being fullscreen
    UnFullscreen {
        /// The surface
        surface: ToplevelSurface<U, R, SD>,
    },
    /// A toplevel surface requested to be minimized
    Minimize {
        /// The surface
        surface: ToplevelSurface<U, R, SD>,
    },
    /// The client requests the window menu to be displayed on this surface at this location
    ///
    /// This menu belongs to the compositor. It is typically expected to contain options for
    /// control of the window (maximize/minimize/close/move/etc...).
    ShowWindowMenu {
        /// The surface
        surface: ToplevelSurface<U, R, SD>,
        /// The seat associated with this input grab
        seat: Resource<wl_seat::WlSeat>,
        /// the grab serial
        serial: u32,
        /// location of the menu request
        location: (i32, i32),
    },
}
