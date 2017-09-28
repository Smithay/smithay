//! Utilities for handling shell surfaces, toplevel and popups
//!
//! This module provides automatic handling of shell surfaces objects, by being registered
//! as a global handler for `wl_shell` and `xdg_shell`.
//!
//! ## Why use this implementation
//!
//! This implementation can track for you the various shell surfaces defined by the
//! clients by handling the `xdg_shell` protocol. It also includes a compatibility
//! layer for the deprecated `wl_shell` global.
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
//! To initialize this handler, simple use the `shell_init` function provided in this
//! module. You will need to provide it the `CompositorToken` you retrieved from an
//! instanciation of the `CompositorHandler` provided by smithay.
//!
//! ```no_run
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! # extern crate wayland_protocols;
//! #
//! use smithay::wayland::compositor::roles::*;
//! use smithay::wayland::compositor::CompositorToken;
//! use smithay::wayland::shell::{shell_init, ShellSurfaceRole, ShellSurfaceUserImplementation};
//! use wayland_server::protocol::wl_shell::WlShell;
//! use wayland_protocols::unstable::xdg_shell::server::zxdg_shell_v6::ZxdgShellV6;
//! use wayland_server::{EventLoop, EventLoopHandle};
//! # use wayland_server::protocol::{wl_seat, wl_output};
//! # use wayland_protocols::unstable::xdg_shell::server::zxdg_toplevel_v6;
//! # #[derive(Default)] struct MySurfaceData;
//!
//! // define the roles type. You need to integrate the ShellSurface role:
//! define_roles!(MyRoles =>
//!     [ShellSurface, ShellSurfaceRole]
//! );
//!
//! // define the metadata you want associated with the shell clients
//! #[derive(Default)]
//! struct MyShellData {
//!     /* ... */
//! }
//!
//! # fn main() {
//! # let (_display, mut event_loop) = wayland_server::create_display();
//! # let (compositor_token, _, _) = smithay::wayland::compositor::compositor_init::<(), MyRoles, _, _>(
//! #     &mut event_loop,
//! #     unimplemented!(),
//! #     (),
//! #     None
//! # );
//! // define your implementation for shell
//! let my_shell_implementation = ShellSurfaceUserImplementation {
//!     new_client: |evlh, idata, client| { unimplemented!() },
//!     client_pong: |evlh, idata, client| { unimplemented!() },
//!     new_toplevel: |evlh, idata, toplevel| { unimplemented!() },
//!     new_popup: |evlh, idata, popup| { unimplemented!() },
//!     move_: |evlh, idata, toplevel, seat, serial| { unimplemented!() },
//!     resize: |evlh, idata, toplevel, seat, serial, edges| { unimplemented!() },
//!     grab: |evlh, idata, popup, seat, serial| { unimplemented!() },
//!     change_display_state: |evlh, idata, toplevel, maximized, minimized, fullscreen, output| {
//!         unimplemented!()
//!     },
//!     show_window_menu: |evlh, idata, toplevel, seat, serial, x, y| { unimplemented!() },
//! };
//!
//! // define your implementation data
//! let my_shell_implementation_data = ();
//!
//! let (shell_state_token, _, _) = shell_init::<_, _, _, _, MyShellData, _>(
//!     &mut event_loop,
//!     compositor_token,             // token from the compositor implementation
//!     my_shell_implementation,      // instance of shell::ShellSurfaceUserImplementation
//!     my_shell_implementation_data, // whatever data you need here
//!     None                          // put a logger if you want
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
//! the subhandler you provided, or via methods on the `ShellState` that you can
//! access from the `state()` of the event loop and the token returned by the init
//! function.

use wayland::compositor::CompositorToken;
use wayland::compositor::roles::Role;
use std::cell::RefCell;
use std::rc::Rc;
use utils::Rectangle;
use wayland_protocols::unstable::xdg_shell::server::{zxdg_popup_v6, zxdg_positioner_v6 as xdg_positioner,
                                                     zxdg_shell_v6, zxdg_surface_v6, zxdg_toplevel_v6};
use wayland_server::{EventLoop, EventLoopHandle, EventResult, Global, Liveness, Resource, StateToken};
use wayland_server::protocol::{wl_output, wl_seat, wl_shell, wl_shell_surface, wl_surface};

mod wl_handlers;
mod xdg_handlers;

/// Metadata associated with the `shell_surface` role
pub struct ShellSurfaceRole {
    /// Pending state as requested by the client
    ///
    /// The data in this field are double-buffered, you should
    /// apply them on a surface commit.
    pub pending_state: ShellSurfacePendingState,
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

#[derive(Copy, Clone, Debug)]
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

/// Contents of the pending state of a shell surface, depending on its role
pub enum ShellSurfacePendingState {
    /// This a regular, toplevel surface
    ///
    /// This corresponds to either the `xdg_toplevel` role from the
    /// `xdg_shell` protocol, or the result of `set_toplevel` using the
    /// `wl_shell` protocol.
    ///
    /// This is what you'll generaly interpret as "a window".
    Toplevel(ToplevelState),
    /// This is a popup surface
    ///
    /// This corresponds to either the `xdg_popup` role from the
    /// `xdg_shell` protocol, or the result of `set_popup` using the
    /// `wl_shell` protocol.
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

impl ToplevelState {
    /// Clone this ToplevelState
    ///
    /// If the parent surface refers to a surface that no longer
    /// exists, it is replaced by `None` in the process.
    pub fn clone(&self) -> ToplevelState {
        ToplevelState {
            parent: self.parent.as_ref().and_then(|p| p.clone()),
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
    pub parent: wl_surface::WlSurface,
    /// The positioner specifying how this tooltip should
    /// be placed relative to its parent.
    pub positioner: PositionerState,
}

impl PopupState {
    /// Clone this PopupState
    ///
    /// If the parent surface refers to a surface that no longer
    /// exists, this will return `None`, as the popup can no
    /// longer be meaningfully displayed.
    pub fn clone(&self) -> Option<PopupState> {
        if let Some(p) = self.parent.clone() {
            Some(PopupState {
                parent: p,
                positioner: self.positioner.clone(),
            })
        } else {
            // the parent surface does no exist any longer,
            // this popup does not make any sense now
            None
        }
    }
}

impl Default for ShellSurfacePendingState {
    fn default() -> ShellSurfacePendingState {
        ShellSurfacePendingState::None
    }
}

/// Internal implementation data of shell surfaces
///
/// This type is only visible as type parameter of
/// the `Global` handle you are provided.
pub struct ShellSurfaceIData<U, R, CID, SID, SD> {
    log: ::slog::Logger,
    compositor_token: CompositorToken<U, R, CID>,
    implementation: ShellSurfaceUserImplementation<U, R, CID, SID, SD>,
    idata: Rc<RefCell<SID>>,
    state_token: StateToken<ShellState<U, R, CID, SD>>,
}

impl<U, R, CID, SID, SD> Clone for ShellSurfaceIData<U, R, CID, SID, SD> {
    fn clone(&self) -> ShellSurfaceIData<U, R, CID, SID, SD> {
        ShellSurfaceIData {
            log: self.log.clone(),
            compositor_token: self.compositor_token.clone(),
            implementation: self.implementation.clone(),
            idata: self.idata.clone(),
            state_token: self.state_token.clone(),
        }
    }
}

/// Create new xdg_shell and wl_shell globals.
///
/// The globals are directly registered into the eventloop, and this function
/// returns a `StateToken<_>` which you'll need access the list of shell
/// surfaces created by your clients.
///
/// It also returns the two global handles, in case you whish to remove these
/// globals from the event loop in the future.
pub fn shell_init<U, R, CID, SID, SD, L>(
    evl: &mut EventLoop, token: CompositorToken<U, R, CID>,
    implementation: ShellSurfaceUserImplementation<U, R, CID, SID, SD>, idata: SID, logger: L)
    -> (
        StateToken<ShellState<U, R, CID, SD>>,
        Global<wl_shell::WlShell, ShellSurfaceIData<U, R, CID, SID, SD>>,
        Global<zxdg_shell_v6::ZxdgShellV6, ShellSurfaceIData<U, R, CID, SID, SD>>,
    )
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: Default + 'static,
    L: Into<Option<::slog::Logger>>,
{
    let log = ::slog_or_stdlog(logger);
    let shell_state = ShellState {
        known_toplevels: Vec::new(),
        known_popups: Vec::new(),
    };
    let shell_state_token = evl.state().insert(shell_state);

    let shell_surface_idata = ShellSurfaceIData {
        log: log.new(o!("smithay_module" => "shell_handler")),
        compositor_token: token,
        implementation: implementation,
        idata: Rc::new(RefCell::new(idata)),
        state_token: shell_state_token.clone(),
    };

    // TODO: init globals
    let wl_shell_global = evl.register_global(
        1,
        self::wl_handlers::wl_shell_bind::<U, R, CID, SID, SD>,
        shell_surface_idata.clone(),
    );
    let xdg_shell_global = evl.register_global(
        1,
        self::xdg_handlers::xdg_shell_bind::<U, R, CID, SID, SD>,
        shell_surface_idata.clone(),
    );

    (shell_state_token, wl_shell_global, xdg_shell_global)
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
pub struct ShellState<U, R, CID, SD> {
    known_toplevels: Vec<ToplevelSurface<U, R, CID, SD>>,
    known_popups: Vec<PopupSurface<U, R, CID, SD>>,
}

impl<U, R, CID, SD> ShellState<U, R, CID, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SD: 'static,
{
    /// Cleans the internal surface storage by removing all dead surfaces
    pub fn cleanup_surfaces(&mut self) {
        self.known_toplevels.retain(|s| s.alive());
        self.known_popups.retain(|s| s.alive());
    }

    /// Access all the shell surfaces known by this handler
    pub fn toplevel_surfaces(&self) -> &[ToplevelSurface<U, R, CID, SD>] {
        &self.known_toplevels[..]
    }

    /// Access all the popup surfaces known by this handler
    pub fn popup_surfaces(&self) -> &[PopupSurface<U, R, CID, SD>] {
        &self.known_popups[..]
    }
}

/*
 * User interaction
 */

enum ShellClientKind {
    Wl(wl_shell::WlShell),
    Xdg(zxdg_shell_v6::ZxdgShellV6),
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
            ShellClientKind::Wl(ref s) => s.status() == Liveness::Alive,
            ShellClientKind::Xdg(ref s) => s.status() == Liveness::Alive,
        }
    }

    /// Checks if this handle and the other one actually refer to the
    /// same shell client
    pub fn equals(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (&ShellClientKind::Wl(ref s1), &ShellClientKind::Wl(ref s2)) => s1.equals(s2),
            (&ShellClientKind::Xdg(ref s1), &ShellClientKind::Xdg(ref s2)) => s1.equals(s2),
            _ => false,
        }
    }

    /// Send a ping request to this shell client
    ///
    /// You'll receive the reply in the `Handler::cient_pong()` method.
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
            ShellClientKind::Wl(ref shell) => {
                let mutex = unsafe { &*(shell.get_user_data() as *mut self::wl_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.0.pending_ping == 0 {
                    return Err(());
                }
                guard.0.pending_ping = serial;
                if let Some(surface) = guard.1.first() {
                    // there is at least one surface, send the ping
                    // if there is no surface, the ping will remain pending
                    // and will be sent when the client creates a surface
                    surface.ping(serial);
                }
            }
            ShellClientKind::Xdg(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::xdg_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.pending_ping == 0 {
                    return Err(());
                }
                guard.pending_ping = serial;
                shell.ping(serial);
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
            ShellClientKind::Wl(ref shell) => {
                let mutex = unsafe { &*(shell.get_user_data() as *mut self::wl_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                Ok(f(&mut guard.0.data))
            }
            ShellClientKind::Xdg(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::xdg_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                Ok(f(&mut guard.data))
            }
        }
    }
}

enum SurfaceKind {
    Wl(wl_shell_surface::WlShellSurface),
    XdgToplevel(zxdg_toplevel_v6::ZxdgToplevelV6),
    XdgPopup(zxdg_popup_v6::ZxdgPopupV6),
}

/// A handle to a toplevel surface
///
/// This is an unified abstraction over the toplevel surfaces
/// of both `wl_shell` and `xdg_shell`.
pub struct ToplevelSurface<U, R, CID, SD> {
    wl_surface: wl_surface::WlSurface,
    shell_surface: SurfaceKind,
    token: CompositorToken<U, R, CID>,
    _shell_data: ::std::marker::PhantomData<SD>,
}

impl<U, R, CID, SD> ToplevelSurface<U, R, CID, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SD: 'static,
{
    /// Is the toplevel surface refered by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_surface_alive = match self.shell_surface {
            SurfaceKind::Wl(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgToplevel(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgPopup(_) => unreachable!(),
        };
        shell_surface_alive && self.wl_surface.status() == Liveness::Alive
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
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => {
                let &(_, ref shell) =
                    unsafe { &*(s.get_user_data() as *mut self::wl_handlers::ShellSurfaceUserData) };
                Some(ShellClient {
                    kind: ShellClientKind::Wl(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                })
            }
            SurfaceKind::XdgToplevel(ref s) => {
                let &(_, ref shell, _) =
                    unsafe { &*(s.get_user_data() as *mut self::xdg_handlers::ShellSurfaceUserData) };
                Some(ShellClient {
                    kind: ShellClientKind::Xdg(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                })
            }
            SurfaceKind::XdgPopup(_) => unreachable!(),
        }
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    pub fn send_configure(&self, cfg: ToplevelConfigure) -> EventResult<()> {
        if !self.alive() {
            return EventResult::Destroyed;
        }
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => self::wl_handlers::send_toplevel_configure(s, cfg),
            SurfaceKind::XdgToplevel(ref s) => {
                self::xdg_handlers::send_toplevel_configure(self.token, s, cfg)
            }
            SurfaceKind::XdgPopup(_) => unreachable!(),
        }
        EventResult::Sent(())
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
            .with_role_data::<ShellSurfaceRole, _, _>(&self.wl_surface, |data| data.configured)
            .expect("A shell surface object exists but the surface does not have the shell_surface role ?!");
        if !configured {
            if let SurfaceKind::XdgToplevel(ref s) = self.shell_surface {
                let ptr = s.get_user_data();
                let &(_, _, ref xdg_surface) =
                    unsafe { &*(ptr as *mut self::xdg_handlers::ShellSurfaceUserData) };
                xdg_surface.post_error(
                    zxdg_surface_v6::Error::NotConstructed as u32,
                    "Surface has not been confgured yet.".into(),
                );
            } else {
                unreachable!();
            }
        }
        configured
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
            .with_role_data::<ShellSurfaceRole, _, _>(&self.wl_surface, |data| match data.pending_state {
                ShellSurfacePendingState::Toplevel(ref state) => Some(state.clone()),
                _ => None,
            })
            .ok()
            .and_then(|x| x)
    }
}

/// A handle to a popup surface
///
/// This is an unified abstraction over the popup surfaces
/// of both `wl_shell` and `xdg_shell`.
pub struct PopupSurface<U, R, CID, SD> {
    wl_surface: wl_surface::WlSurface,
    shell_surface: SurfaceKind,
    token: CompositorToken<U, R, CID>,
    _shell_data: ::std::marker::PhantomData<SD>,
}

impl<U, R, CID, SD> PopupSurface<U, R, CID, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SD: 'static,
{
    /// Is the popup surface refered by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_surface_alive = match self.shell_surface {
            SurfaceKind::Wl(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgPopup(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgToplevel(_) => unreachable!(),
        };
        shell_surface_alive && self.wl_surface.status() == Liveness::Alive
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
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => {
                let &(_, ref shell) =
                    unsafe { &*(s.get_user_data() as *mut self::wl_handlers::ShellSurfaceUserData) };
                Some(ShellClient {
                    kind: ShellClientKind::Wl(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                })
            }
            SurfaceKind::XdgPopup(ref s) => {
                let &(_, ref shell, _) =
                    unsafe { &*(s.get_user_data() as *mut self::xdg_handlers::ShellSurfaceUserData) };
                Some(ShellClient {
                    kind: ShellClientKind::Xdg(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                })
            }
            SurfaceKind::XdgToplevel(_) => unreachable!(),
        }
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    pub fn send_configure(&self, cfg: PopupConfigure) -> EventResult<()> {
        if !self.alive() {
            return EventResult::Destroyed;
        }
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => self::wl_handlers::send_popup_configure(s, cfg),
            SurfaceKind::XdgPopup(ref s) => self::xdg_handlers::send_popup_configure(self.token, s, cfg),
            SurfaceKind::XdgToplevel(_) => unreachable!(),
        }
        EventResult::Sent(())
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
            .with_role_data::<ShellSurfaceRole, _, _>(&self.wl_surface, |data| data.configured)
            .expect("A shell surface object exists but the surface does not have the shell_surface role ?!");
        if !configured {
            if let SurfaceKind::XdgPopup(ref s) = self.shell_surface {
                let ptr = s.get_user_data();
                let &(_, _, ref xdg_surface) =
                    unsafe { &*(ptr as *mut self::xdg_handlers::ShellSurfaceUserData) };
                xdg_surface.post_error(
                    zxdg_surface_v6::Error::NotConstructed as u32,
                    "Surface has not been confgured yet.".into(),
                );
            } else {
                unreachable!();
            }
        }
        configured
    }

    /// Send a 'popup_done' event to the popup surface
    ///
    /// It means that the use has dismissed the popup surface, or that
    /// the pointer has left the area of popup grab if there was a grab.
    pub fn send_popup_done(&self) -> EventResult<()> {
        if !self.alive() {
            return EventResult::Destroyed;
        }
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => s.popup_done(),
            SurfaceKind::XdgPopup(ref s) => s.popup_done(),
            SurfaceKind::XdgToplevel(_) => unreachable!(),
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
            .with_role_data::<ShellSurfaceRole, _, _>(&self.wl_surface, |data| match data.pending_state {
                ShellSurfacePendingState::Popup(ref state) => state.clone(),
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
    pub states: Vec<zxdg_toplevel_v6::State>,
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

/// A sub-implementation for the shell
///
/// You need to provide this to handle events that the provided implementation
/// cannot process for you directly.
///
/// Depending on what you want to do, you might implement some of these functions
/// as doing nothing.
pub struct ShellSurfaceUserImplementation<U, R, CID, SID, SD> {
    /// A new shell client was instanciated
    pub new_client: fn(evlh: &mut EventLoopHandle, idata: &mut SID, client: ShellClient<SD>),
    /// The pong for a pending ping of this shell client was received
    ///
    /// The ShellHandler already checked for you that the serial matches the one
    /// from the pending ping.
    pub client_pong: fn(evlh: &mut EventLoopHandle, idata: &mut SID, client: ShellClient<SD>),
    /// A new toplevel surface was created
    ///
    /// You need to return a `ToplevelConfigure` from this function, which will be sent
    /// to the client to configure this surface
    pub new_toplevel: fn(
     evlh: &mut EventLoopHandle,
     idata: &mut SID,
     surface: ToplevelSurface<U, R, CID, SD>,
    ) -> ToplevelConfigure,
    /// A new popup surface was created
    ///
    /// You need to return a `PopupConfigure` from this function, which will be sent
    /// to the client to configure this surface
    pub new_popup: fn(evlh: &mut EventLoopHandle, idata: &mut SID, surface: PopupSurface<U, R, CID, SD>)
     -> PopupConfigure,
    /// The client requested the start of an interactive move for this surface
    pub move_: fn(
     evlh: &mut EventLoopHandle,
     idata: &mut SID,
     surface: ToplevelSurface<U, R, CID, SD>,
     seat: &wl_seat::WlSeat,
     serial: u32,
    ),
    /// The client requested the start of an interactive resize for this surface
    ///
    /// The `edges` argument specifies which part of the window's border is being dragged.
    pub resize: fn(
     evlh: &mut EventLoopHandle,
     idata: &mut SID,
     surface: ToplevelSurface<U, R, CID, SD>,
     seat: &wl_seat::WlSeat,
     serial: u32,
     edges: zxdg_toplevel_v6::ResizeEdge,
    ),
    /// This popup requests a grab of the pointer
    ///
    /// This means it requests to be sent a `popup_done` event when the pointer leaves
    /// the grab area.
    pub grab: fn(
     evlh: &mut EventLoopHandle,
     idata: &mut SID,
     surface: PopupSurface<U, R, CID, SD>,
     seat: &wl_seat::WlSeat,
     serial: u32,
    ),
    /// A toplevel surface requested its display state to be changed
    ///
    /// Each field represents the request of the client for a specific property:
    ///
    /// - `None`: no request is made to change this property
    /// - `Some(true)`: this property should be enabled
    /// - `Some(false)`: this property should be disabled
    ///
    /// For fullscreen/maximization, the client can also optionnaly request a specific
    /// output.
    ///
    /// You are to answer with a `ToplevelConfigure` that will be sent to the client in
    /// response.
    pub change_display_state: fn(
     evlh: &mut EventLoopHandle,
     idata: &mut SID,
     surface: ToplevelSurface<U, R, CID, SD>,
     maximized: Option<bool>,
     minimized: Option<bool>,
     fullscreen: Option<bool>,
     output: Option<&wl_output::WlOutput>,
    ) -> ToplevelConfigure,
    /// The client requests the window menu to be displayed on this surface at this location
    ///
    /// This menu belongs to the compositor. It is typically expected to contain options for
    /// control of the window (maximize/minimize/close/move/etc...).
    pub show_window_menu: fn(
     evlh: &mut EventLoopHandle,
     idata: &mut SID,
     surface: ToplevelSurface<U, R, CID, SD>,
     seat: &wl_seat::WlSeat,
     serial: u32,
     x: i32,
     y: i32,
    ),
}

impl<U, R, CID, SID, SD> Copy for ShellSurfaceUserImplementation<U, R, CID, SID, SD> {}
impl<U, R, CID, SID, SD> Clone for ShellSurfaceUserImplementation<U, R, CID, SID, SD> {
    fn clone(&self) -> ShellSurfaceUserImplementation<U, R, CID, SID, SD> {
        *self
    }
}
