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
//! To initialize this handler, simple use the [`xdg_shell_init`] function provided in this module.
//! You need to provide a closure that will be invoked whenever some action is required from you,
//! are represented by the [`XdgRequest`] enum.
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::wayland::shell::xdg::{xdg_shell_init, XdgRequest};
//!
//! # let mut display = wayland_server::Display::new();
//! let (shell_state, _, _) = xdg_shell_init(
//!     &mut display,
//!     // your implementation
//!     |event: XdgRequest, dispatch_data| { /* handle the shell requests here */ },
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
//!   you can associate client-wise metadata to it through an [`UserDataMap`].
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

use crate::utils::DeadResource;
use crate::utils::Rectangle;
use crate::wayland::compositor;
use crate::wayland::compositor::Cacheable;
use crate::wayland::{Serial, SERIAL_COUNTER};
use std::fmt::Debug;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use wayland_protocols::unstable::xdg_shell::v6::server::zxdg_surface_v6;
use wayland_protocols::xdg_shell::server::xdg_surface;
use wayland_protocols::{
    unstable::xdg_shell::v6::server::{zxdg_popup_v6, zxdg_shell_v6, zxdg_toplevel_v6},
    xdg_shell::server::{xdg_popup, xdg_positioner, xdg_toplevel, xdg_wm_base},
};
use wayland_server::DispatchData;
use wayland_server::{
    protocol::{wl_output, wl_seat, wl_surface},
    Display, Filter, Global, UserDataMap,
};

use super::PingError;

// handlers for the xdg_shell protocol
mod xdg_handlers;
// compatibility handlers for the zxdg_shell_v6 protocol, its earlier version
mod zxdgv6_handlers;

macro_rules! xdg_role {
    ($configure:ty,
     $(#[$attr:meta])* $element:ident {$($(#[$field_attr:meta])* $vis:vis$field:ident:$type:ty),*},
     $role_ack_configure:expr) => {

        $(#[$attr])*
        pub struct $element {
                /// Defines if the surface has received at least one
                /// xdg_surface.ack_configure from the client
                pub configured: bool,

                /// The serial of the last acked configure
                pub configure_serial: Option<Serial>,

                /// Holds the state if the surface has sent the initial
                /// configure event to the client. It is expected that
                /// during the first commit a initial
                /// configure event is sent to the client
                pub initial_configure_sent: bool,

                /// Holds the configures the server has sent out
                /// to the client waiting to be acknowledged by
                /// the client. All pending configures that are older
                /// than the acknowledged one will be discarded during
                /// processing xdg_surface.ack_configure.
                pending_configures: Vec<$configure>,

                $(
                    $(#[$field_attr])*
                    $vis $field: $type,
                )*
        }

        impl $element {
            fn ack_configure(&mut self, serial: Serial) -> Option<Configure> {
                let configure = match self
                    .pending_configures
                    .iter()
                    .find(|configure| configure.serial == serial)
                {
                    Some(configure) => (*configure).clone(),
                    None => {
                        return None;
                    }
                };

                // Role specific ack_configure
                let role_ack_configure: &dyn Fn(&mut Self, $configure) = &$role_ack_configure;
                role_ack_configure(self, configure.clone());

                // Set the xdg_surface to configured
                self.configured = true;

                // Save the last configure serial as a reference
                self.configure_serial = Some(Serial::from(serial));

                // Clean old configures
                self.pending_configures.retain(|c| c.serial > serial);

                Some(configure.into())
            }
        }

        impl Default for $element {
            fn default() -> Self {
                Self {
                    configured: false,
                    configure_serial: None,
                    pending_configures: Vec::new(),
                    initial_configure_sent: false,

                    $(
                        $field: Default::default(),
                    )*
                }
            }
        }
    };
}

xdg_role!(
    ToplevelConfigure,
    /// Role specific attributes for xdg_toplevel
    ///
    /// This interface defines an xdg_surface role which allows a surface to,
    /// among other things, set window-like properties such as maximize,
    /// fullscreen, and minimize, set application-specific metadata like title and
    /// id, and well as trigger user interactive operations such as interactive
    /// resize and move.
    ///
    /// Unmapping an xdg_toplevel means that the surface cannot be shown
    /// by the compositor until it is explicitly mapped again.
    /// All active operations (e.g., move, resize) are canceled and all
    /// attributes (e.g. title, state, stacking, ...) are discarded for
    /// an xdg_toplevel surface when it is unmapped. The xdg_toplevel returns to
    /// the state it had right after xdg_surface.get_toplevel. The client
    /// can re-map the toplevel by perfoming a commit without any buffer
    /// attached, waiting for a configure event and handling it as usual (see
    /// xdg_surface description).
    ///
    /// Attaching a null buffer to a toplevel unmaps the surface.
    #[derive(Debug)]
    XdgToplevelSurfaceRoleAttributes {
        /// The parent field of a toplevel should be used
        /// by the compositor to determine which toplevel
        /// should be brought to front. If the parent is focused
        /// all of it's child should be brought to front.
        pub parent: Option<wl_surface::WlSurface>,
        /// Holds the optional title the client has set for
        /// this toplevel. For example a web-browser will most likely
        /// set this to include the current uri.
        ///
        /// This string may be used to identify the surface in a task bar,
        /// window list, or other user interface elements provided by the
        /// compositor.
        pub title: Option<String>,
        /// Holds the optional app ID the client has set for
        /// this toplevel.
        ///
        /// The app ID identifies the general class of applications to which
        /// the surface belongs. The compositor can use this to group multiple
        /// surfaces together, or to determine how to launch a new application.
        ///
        /// For D-Bus activatable applications, the app ID is used as the D-Bus
        /// service name.
        pub app_id: Option<String>,
        /// Minimum size requested for this surface
        ///
        /// A value of 0 on an axis means this axis is not constrained
        pub min_size: (i32, i32),
        /// Maximum size requested for this surface
        ///
        /// A value of 0 on an axis means this axis is not constrained
        pub max_size: (i32, i32),
        /// Holds the pending state as set by the server.
        pub server_pending: Option<ToplevelState>,
        /// Holds the last server_pending state that has been acknowledged
        /// by the client. This state should be cloned to the current
        /// during a commit.
        pub last_acked: Option<ToplevelState>,
        /// Holds the current state of the toplevel after a successful
        /// commit.
        pub current: ToplevelState
    },
    |attributes, configure| {
        attributes.last_acked = Some(configure.state);
    }
);

xdg_role!(
    PopupConfigure,
    /// Role specific attributes for xdg_popup
    ///
    /// A popup surface is a short-lived, temporary surface. It can be used to
    /// implement for example menus, popovers, tooltips and other similar user
    /// interface concepts.
    ///
    /// A popup can be made to take an explicit grab. See xdg_popup.grab for
    /// details.
    ///
    /// When the popup is dismissed, a popup_done event will be sent out, and at
    /// the same time the surface will be unmapped. See the xdg_popup.popup_done
    /// event for details.
    ///
    /// Explicitly destroying the xdg_popup object will also dismiss the popup and
    /// unmap the surface. Clients that want to dismiss the popup when another
    /// surface of their own is clicked should dismiss the popup using the destroy
    /// request.
    ///
    /// A newly created xdg_popup will be stacked on top of all previously created
    /// xdg_popup surfaces associated with the same xdg_toplevel.
    ///
    /// The parent of an xdg_popup must be mapped (see the xdg_surface
    /// description) before the xdg_popup itself.
    ///
    /// The client must call wl_surface.commit on the corresponding wl_surface
    /// for the xdg_popup state to take effect.
    #[derive(Debug)]
    XdgPopupSurfaceRoleAttributes {
        /// Holds the parent for the xdg_popup.
        ///
        /// The parent is allowed to remain unset as long
        /// as no commit has been requested for the underlying
        /// wl_surface. The parent can either be directly
        /// specified during xdg_surface.get_popup or using
        /// another protocol extension, for example xdg_layer_shell.
        ///
        /// It is a protocol error to call commit on a wl_surface with
        /// the xdg_popup role when no parent is set.
        pub parent: Option<wl_surface::WlSurface>,
        /// The positioner state can be used by the compositor
        /// to calculate the best placement for the popup.
        ///
        /// For example the compositor should prevent that a popup
        /// is placed outside the visible rectangle of a output.
        pub positioner: PositionerState,
        /// Holds the last server_pending state that has been acknowledged
        /// by the client. This state should be cloned to the current
        /// during a commit.
        pub last_acked: Option<PopupState>,
        /// Holds the current state of the popup after a successful
        /// commit.
        pub current: PopupState,
        /// Holds the pending state as set by the server.
        pub server_pending: Option<PopupState>,
        popup_handle: Option<PopupKind>
    },
    |attributes,configure| {
        attributes.last_acked = Some(configure.state);
    }
);

/// Represents the state of the popup
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupState {
    /// Holds the geometry of the popup as defined by the positioner.
    ///
    /// `Rectangle::width` and `Rectangle::height` holds the size of the
    /// of the popup in surface-local coordinates and corresponds to the
    /// window geometry
    ///
    /// `Rectangle::x` and `Rectangle::y` holds the position of the popup
    /// The position is relative to the window geometry as defined by
    /// xdg_surface.set_window_geometry of the parent surface.
    pub geometry: Rectangle,
}

impl Default for PopupState {
    fn default() -> Self {
        Self {
            geometry: Default::default(),
        }
    }
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

impl Default for PositionerState {
    fn default() -> Self {
        PositionerState {
            anchor_edges: xdg_positioner::Anchor::None,
            anchor_rect: Default::default(),
            constraint_adjustment: xdg_positioner::ConstraintAdjustment::empty(),
            gravity: xdg_positioner::Gravity::None,
            offset: (0, 0),
            rect_size: (0, 0),
        }
    }
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

    pub(crate) fn anchor_has_edge(&self, edge: xdg_positioner::Anchor) -> bool {
        match edge {
            xdg_positioner::Anchor::Top => {
                self.anchor_edges == xdg_positioner::Anchor::Top
                    || self.anchor_edges == xdg_positioner::Anchor::TopLeft
                    || self.anchor_edges == xdg_positioner::Anchor::TopRight
            }
            xdg_positioner::Anchor::Bottom => {
                self.anchor_edges == xdg_positioner::Anchor::Bottom
                    || self.anchor_edges == xdg_positioner::Anchor::BottomLeft
                    || self.anchor_edges == xdg_positioner::Anchor::BottomRight
            }
            xdg_positioner::Anchor::Left => {
                self.anchor_edges == xdg_positioner::Anchor::Left
                    || self.anchor_edges == xdg_positioner::Anchor::TopLeft
                    || self.anchor_edges == xdg_positioner::Anchor::BottomLeft
            }
            xdg_positioner::Anchor::Right => {
                self.anchor_edges == xdg_positioner::Anchor::Right
                    || self.anchor_edges == xdg_positioner::Anchor::TopRight
                    || self.anchor_edges == xdg_positioner::Anchor::BottomRight
            }
            _ => unreachable!(),
        }
    }

    pub(crate) fn gravity_has_edge(&self, edge: xdg_positioner::Gravity) -> bool {
        match edge {
            xdg_positioner::Gravity::Top => {
                self.gravity == xdg_positioner::Gravity::Top
                    || self.gravity == xdg_positioner::Gravity::TopLeft
                    || self.gravity == xdg_positioner::Gravity::TopRight
            }
            xdg_positioner::Gravity::Bottom => {
                self.gravity == xdg_positioner::Gravity::Bottom
                    || self.gravity == xdg_positioner::Gravity::BottomLeft
                    || self.gravity == xdg_positioner::Gravity::BottomRight
            }
            xdg_positioner::Gravity::Left => {
                self.gravity == xdg_positioner::Gravity::Left
                    || self.gravity == xdg_positioner::Gravity::TopLeft
                    || self.gravity == xdg_positioner::Gravity::BottomLeft
            }
            xdg_positioner::Gravity::Right => {
                self.gravity == xdg_positioner::Gravity::Right
                    || self.gravity == xdg_positioner::Gravity::TopRight
                    || self.gravity == xdg_positioner::Gravity::BottomRight
            }
            _ => unreachable!(),
        }
    }

    /// Get the geometry for a popup as defined by this positioner.
    ///
    /// `Rectangle::width` and `Rectangle::height` corresponds to the
    /// size set by `xdg_positioner.set_size`.
    ///
    /// `Rectangle::x` and `Rectangle::y` define the position of the
    /// popup relative to it's parent surface `window_geometry`.
    /// The position is calculated according to the rules defined
    /// in the `xdg_shell` protocol.
    /// The `constraint_adjustment` will not be considered by this
    /// implementation and the position and size should be re-calculated
    /// in the compositor if the compositor implements `constraint_adjustment`
    pub(crate) fn get_geometry(&self) -> Rectangle {
        // From the `xdg_shell` prococol specification:
        //
        // set_offset:
        //
        //  Specify the surface position offset relative to the position of the
        //  anchor on the anchor rectangle and the anchor on the surface. For
        //  example if the anchor of the anchor rectangle is at (x, y), the surface
        //  has the gravity bottom|right, and the offset is (ox, oy), the calculated
        //  surface position will be (x + ox, y + oy)
        let mut geometry = Rectangle {
            x: self.offset.0,
            y: self.offset.1,
            width: self.rect_size.0,
            height: self.rect_size.1,
        };

        // Defines the anchor point for the anchor rectangle. The specified anchor
        // is used derive an anchor point that the child surface will be
        // positioned relative to. If a corner anchor is set (e.g. 'top_left' or
        // 'bottom_right'), the anchor point will be at the specified corner;
        // otherwise, the derived anchor point will be centered on the specified
        // edge, or in the center of the anchor rectangle if no edge is specified.
        if self.anchor_has_edge(xdg_positioner::Anchor::Top) {
            geometry.y += self.anchor_rect.y;
        } else if self.anchor_has_edge(xdg_positioner::Anchor::Bottom) {
            geometry.y += self.anchor_rect.y + self.anchor_rect.height;
        } else {
            geometry.y += self.anchor_rect.y + self.anchor_rect.height / 2;
        }

        if self.anchor_has_edge(xdg_positioner::Anchor::Left) {
            geometry.x += self.anchor_rect.x;
        } else if self.anchor_has_edge(xdg_positioner::Anchor::Right) {
            geometry.x += self.anchor_rect.x + self.anchor_rect.width;
        } else {
            geometry.x += self.anchor_rect.x + self.anchor_rect.width / 2;
        }

        // Defines in what direction a surface should be positioned, relative to
        // the anchor point of the parent surface. If a corner gravity is
        // specified (e.g. 'bottom_right' or 'top_left'), then the child surface
        // will be placed towards the specified gravity; otherwise, the child
        // surface will be centered over the anchor point on any axis that had no
        // gravity specified.
        if self.gravity_has_edge(xdg_positioner::Gravity::Top) {
            geometry.y -= geometry.height;
        } else if !self.gravity_has_edge(xdg_positioner::Gravity::Bottom) {
            geometry.y -= geometry.height / 2;
        }

        if self.gravity_has_edge(xdg_positioner::Gravity::Left) {
            geometry.x -= geometry.width;
        } else if !self.gravity_has_edge(xdg_positioner::Gravity::Right) {
            geometry.x -= geometry.width / 2;
        }

        geometry
    }
}

/// State of a regular toplevel surface
#[derive(Debug, PartialEq)]
pub struct ToplevelState {
    /// The suggested size of the surface
    pub size: Option<(i32, i32)>,

    /// The states for this surface
    pub states: ToplevelStateSet,

    /// The output for a fullscreen display
    pub fullscreen_output: Option<wl_output::WlOutput>,
}

impl Default for ToplevelState {
    fn default() -> Self {
        ToplevelState {
            fullscreen_output: None,
            states: Default::default(),
            size: None,
        }
    }
}

impl Clone for ToplevelState {
    fn clone(&self) -> ToplevelState {
        ToplevelState {
            fullscreen_output: self.fullscreen_output.clone(),
            states: self.states.clone(),
            size: self.size,
        }
    }
}

/// Container holding the states for a `XdgToplevel`
///
/// This container will prevent the `XdgToplevel` from
/// having the same `xdg_toplevel::State` multiple times
/// and simplifies setting and un-setting a particularly
/// `xdg_toplevel::State`
#[derive(Debug, Clone, PartialEq)]
pub struct ToplevelStateSet {
    states: Vec<xdg_toplevel::State>,
}

impl ToplevelStateSet {
    /// Returns `true` if the states contains a state.
    pub fn contains(&self, state: xdg_toplevel::State) -> bool {
        self.states.iter().any(|s| *s == state)
    }

    /// Adds a state to the states.
    ///
    /// If the states did not have this state present, `true` is returned.
    ///
    /// If the states did have this state present, `false` is returned.
    pub fn set(&mut self, state: xdg_toplevel::State) -> bool {
        if self.contains(state) {
            false
        } else {
            self.states.push(state);
            true
        }
    }

    /// Removes a state from the states. Returns whether the state was
    /// present in the states.
    pub fn unset(&mut self, state: xdg_toplevel::State) -> bool {
        if !self.contains(state) {
            false
        } else {
            self.states.retain(|s| *s != state);
            true
        }
    }
}

impl Default for ToplevelStateSet {
    fn default() -> Self {
        Self { states: Vec::new() }
    }
}

impl IntoIterator for ToplevelStateSet {
    type Item = xdg_toplevel::State;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.states.into_iter()
    }
}

impl From<ToplevelStateSet> for Vec<xdg_toplevel::State> {
    fn from(states: ToplevelStateSet) -> Self {
        states.states
    }
}

/// Represents the client pending state
#[derive(Debug, Clone, Copy)]
pub struct SurfaceCachedState {
    /// Holds the double-buffered geometry that may be specified
    /// by xdg_surface.set_window_geometry.
    pub geometry: Option<Rectangle>,
    /// Minimum size requested for this surface
    ///
    /// A value of 0 on an axis means this axis is not constrained
    ///
    /// This is only relevant for xdg_toplevel, and will always be
    /// `(0, 0)` for xdg_popup.
    pub min_size: (i32, i32),
    /// Maximum size requested for this surface
    ///
    /// A value of 0 on an axis means this axis is not constrained
    ///
    /// This is only relevant for xdg_toplevel, and will always be
    /// `(0, 0)` for xdg_popup.
    pub max_size: (i32, i32),
}

impl Default for SurfaceCachedState {
    fn default() -> Self {
        Self {
            geometry: None,
            min_size: (0, 0),
            max_size: (0, 0),
        }
    }
}

impl Cacheable for SurfaceCachedState {
    fn commit(&mut self) -> Self {
        *self
    }
    fn merge_into(self, into: &mut Self) {
        *into = self;
    }
}

pub(crate) struct ShellData {
    log: ::slog::Logger,
    user_impl: Rc<RefCell<dyn FnMut(XdgRequest, DispatchData<'_>)>>,
    shell_state: Arc<Mutex<ShellState>>,
}

impl Clone for ShellData {
    fn clone(&self) -> Self {
        ShellData {
            log: self.log.clone(),
            user_impl: self.user_impl.clone(),
            shell_state: self.shell_state.clone(),
        }
    }
}

/// Create a new `xdg_shell` globals
pub fn xdg_shell_init<L, Impl>(
    display: &mut Display,
    implementation: Impl,
    logger: L,
) -> (
    Arc<Mutex<ShellState>>,
    Global<xdg_wm_base::XdgWmBase>,
    Global<zxdg_shell_v6::ZxdgShellV6>,
)
where
    L: Into<Option<::slog::Logger>>,
    Impl: FnMut(XdgRequest, DispatchData<'_>) + 'static,
{
    let log = crate::slog_or_fallback(logger);
    let shell_state = Arc::new(Mutex::new(ShellState {
        known_toplevels: Vec::new(),
        known_popups: Vec::new(),
    }));

    let shell_data = ShellData {
        log: log.new(slog::o!("smithay_module" => "xdg_shell_handler")),
        user_impl: Rc::new(RefCell::new(implementation)),
        shell_state: shell_state.clone(),
    };

    let shell_data_z = shell_data.clone();

    let xdg_shell_global = display.create_global(
        1,
        Filter::new(move |(shell, _version), _, dispatch_data| {
            self::xdg_handlers::implement_wm_base(shell, &shell_data, dispatch_data);
        }),
    );

    let zxdgv6_shell_global = display.create_global(
        1,
        Filter::new(move |(shell, _version), _, dispatch_data| {
            self::zxdgv6_handlers::implement_shell(shell, &shell_data_z, dispatch_data);
        }),
    );

    (shell_state, xdg_shell_global, zxdgv6_shell_global)
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
#[derive(Debug)]
pub struct ShellState {
    known_toplevels: Vec<ToplevelSurface>,
    known_popups: Vec<PopupSurface>,
}

impl ShellState {
    /// Access all the shell surfaces known by this handler
    pub fn toplevel_surfaces(&self) -> &[ToplevelSurface] {
        &self.known_toplevels[..]
    }

    /// Access all the popup surfaces known by this handler
    pub fn popup_surfaces(&self) -> &[PopupSurface] {
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
    pending_ping: Option<Serial>,
    data: UserDataMap,
}

fn make_shell_client_data() -> ShellClientData {
    ShellClientData {
        pending_ping: None,
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
pub struct ShellClient {
    kind: ShellClientKind,
}

impl std::cmp::PartialEq for ShellClient {
    fn eq(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (&ShellClientKind::Xdg(ref s1), &ShellClientKind::Xdg(ref s2)) => s1 == s2,
            (&ShellClientKind::ZxdgV6(ref s1), &ShellClientKind::ZxdgV6(ref s2)) => s1 == s2,
            _ => false,
        }
    }
}

impl ShellClient {
    /// Is the shell client represented by this handle still connected?
    pub fn alive(&self) -> bool {
        match self.kind {
            ShellClientKind::Xdg(ref s) => s.as_ref().is_alive(),
            ShellClientKind::ZxdgV6(ref s) => s.as_ref().is_alive(),
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
    pub fn send_ping(&self, serial: Serial) -> Result<(), PingError> {
        if !self.alive() {
            return Err(PingError::DeadSurface);
        }
        match self.kind {
            ShellClientKind::Xdg(ref shell) => {
                let user_data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellUserData>()
                    .unwrap();
                let mut guard = user_data.client_data.lock().unwrap();
                if let Some(pending_ping) = guard.pending_ping {
                    return Err(PingError::PingAlreadyPending(pending_ping));
                }
                guard.pending_ping = Some(serial);
                shell.ping(serial.into());
            }
            ShellClientKind::ZxdgV6(ref shell) => {
                let user_data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellUserData>()
                    .unwrap();
                let mut guard = user_data.client_data.lock().unwrap();
                if let Some(pending_ping) = guard.pending_ping {
                    return Err(PingError::PingAlreadyPending(pending_ping));
                }
                guard.pending_ping = Some(serial);
                shell.ping(serial.into());
            }
        }
        Ok(())
    }

    /// Access the user data associated with this shell client
    pub fn with_data<F, T>(&self, f: F) -> Result<T, crate::utils::DeadResource>
    where
        F: FnOnce(&mut UserDataMap) -> T,
    {
        if !self.alive() {
            return Err(crate::utils::DeadResource);
        }
        match self.kind {
            ShellClientKind::Xdg(ref shell) => {
                let data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellUserData>()
                    .unwrap();
                let mut guard = data.client_data.lock().unwrap();
                Ok(f(&mut guard.data))
            }
            ShellClientKind::ZxdgV6(ref shell) => {
                let data = shell
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellUserData>()
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
#[derive(Debug, Clone)]
pub struct ToplevelSurface {
    wl_surface: wl_surface::WlSurface,
    shell_surface: ToplevelKind,
}

impl std::cmp::PartialEq for ToplevelSurface {
    fn eq(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface == other.wl_surface
    }
}

impl ToplevelSurface {
    /// Is the toplevel surface referred by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_alive = match self.shell_surface {
            ToplevelKind::Xdg(ref s) => s.as_ref().is_alive(),
            ToplevelKind::ZxdgV6(ref s) => s.as_ref().is_alive(),
        };
        shell_alive && self.wl_surface.as_ref().is_alive()
    }

    /// Retrieve the shell client owning this toplevel surface
    ///
    /// Returns `None` if the surface does actually no longer exist.
    pub fn client(&self) -> Option<ShellClient> {
        if !self.alive() {
            return None;
        }

        let shell = match self.shell_surface {
            ToplevelKind::Xdg(ref s) => {
                let data = s
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellSurfaceUserData>()
                    .unwrap();
                ShellClientKind::Xdg(data.wm_base.clone())
            }
            ToplevelKind::ZxdgV6(ref s) => {
                let data = s
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellSurfaceUserData>()
                    .unwrap();
                ShellClientKind::ZxdgV6(data.shell.clone())
            }
        };

        Some(ShellClient { kind: shell })
    }

    /// Gets the current pending state for a configure
    ///
    /// Returns `Some` if either no initial configure has been sent or
    /// the `server_pending` is `Some` and different from the last pending
    /// configure or `last_acked` if there is no pending
    ///
    /// Returns `None` if either no `server_pending` or the pending
    /// has already been sent to the client or the pending is equal
    /// to the `last_acked`
    fn get_pending_state(&self, attributes: &mut XdgToplevelSurfaceRoleAttributes) -> Option<ToplevelState> {
        if !attributes.initial_configure_sent {
            return Some(attributes.server_pending.take().unwrap_or_default());
        }

        let server_pending = match attributes.server_pending.take() {
            Some(state) => state,
            None => {
                return None;
            }
        };

        let last_state = attributes
            .pending_configures
            .last()
            .map(|c| &c.state)
            .or_else(|| attributes.last_acked.as_ref());

        if let Some(state) = last_state {
            if state == &server_pending {
                return None;
            }
        }

        Some(server_pending)
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    ///
    /// You can manipulate the state that will be sent to the client with the [`with_pending_state`](#method.with_pending_state)
    /// method.
    pub fn send_configure(&self) {
        if let Some(surface) = self.get_surface() {
            let configure = compositor::with_states(surface, |states| {
                let mut attributes = states
                    .data_map
                    .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();
                if let Some(pending) = self.get_pending_state(&mut *attributes) {
                    let configure = ToplevelConfigure {
                        serial: SERIAL_COUNTER.next_serial(),
                        state: pending,
                    };

                    attributes.pending_configures.push(configure.clone());
                    attributes.initial_configure_sent = true;

                    Some(configure)
                } else {
                    None
                }
            })
            .unwrap_or(None);
            if let Some(configure) = configure {
                match self.shell_surface {
                    ToplevelKind::Xdg(ref s) => self::xdg_handlers::send_toplevel_configure(s, configure),
                    ToplevelKind::ZxdgV6(ref s) => {
                        self::zxdgv6_handlers::send_toplevel_configure(s, configure)
                    }
                }
            }
        }
    }

    /// Handles the role specific commit logic
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn commit_hook(surface: &wl_surface::WlSurface) {
        compositor::with_states(surface, |states| {
            let mut guard = states
                .data_map
                .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if let Some(state) = guard.last_acked.clone() {
                guard.current = state;
            }
        })
        .unwrap();
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
        let configured = compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .configured
        })
        .unwrap();
        if !configured {
            match self.shell_surface {
                ToplevelKind::Xdg(ref s) => {
                    let data = s
                        .as_ref()
                        .user_data()
                        .get::<self::xdg_handlers::ShellSurfaceUserData>()
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
                        .get::<self::zxdgv6_handlers::ShellSurfaceUserData>()
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

    /// Allows the pending state of this toplevel to
    /// be manipulated.
    ///
    /// This should be used to inform the client about size and state changes,
    /// for example after a resize request from the client.
    ///
    /// The state will be sent to the client when calling [`send_configure`](#method.send_configure).
    pub fn with_pending_state<F, T>(&self, f: F) -> Result<T, DeadResource>
    where
        F: FnOnce(&mut ToplevelState) -> T,
    {
        if !self.alive() {
            return Err(DeadResource);
        }

        Ok(compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.server_pending.is_none() {
                attributes.server_pending = Some(attributes.current.clone());
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
        .unwrap())
    }

    /// Gets a copy of the current state of this toplevel
    ///
    /// Returns `None` if the underlying surface has been
    /// destroyed
    pub fn current_state(&self) -> Option<ToplevelState> {
        if !self.alive() {
            return None;
        }

        Some(
            compositor::with_states(&self.wl_surface, |states| {
                let attributes = states
                    .data_map
                    .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();

                attributes.current.clone()
            })
            .unwrap(),
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PopupKind {
    Xdg(xdg_popup::XdgPopup),
    ZxdgV6(zxdg_popup_v6::ZxdgPopupV6),
}

/// A handle to a popup surface
///
/// This is an unified abstraction over the popup surfaces
/// of both `wl_shell` and `xdg_shell`.
#[derive(Debug, Clone)]
pub struct PopupSurface {
    wl_surface: wl_surface::WlSurface,
    shell_surface: PopupKind,
}

impl std::cmp::PartialEq for PopupSurface {
    fn eq(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface == other.wl_surface
    }
}

impl PopupSurface {
    /// Is the popup surface referred by this handle still alive?
    pub fn alive(&self) -> bool {
        let shell_alive = match self.shell_surface {
            PopupKind::Xdg(ref p) => p.as_ref().is_alive(),
            PopupKind::ZxdgV6(ref p) => p.as_ref().is_alive(),
        };
        shell_alive && self.wl_surface.as_ref().is_alive()
    }

    /// Gets a reference of the parent WlSurface of
    /// this popup.
    pub fn get_parent_surface(&self) -> Option<wl_surface::WlSurface> {
        if !self.alive() {
            None
        } else {
            compositor::with_states(&self.wl_surface, |states| {
                states
                    .data_map
                    .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .parent
                    .clone()
            })
            .unwrap()
        }
    }

    /// Retrieve the shell client owning this popup surface
    ///
    /// Returns `None` if the surface does actually no longer exist.
    pub fn client(&self) -> Option<ShellClient> {
        if !self.alive() {
            return None;
        }

        let shell = match self.shell_surface {
            PopupKind::Xdg(ref p) => {
                let data = p
                    .as_ref()
                    .user_data()
                    .get::<self::xdg_handlers::ShellSurfaceUserData>()
                    .unwrap();
                ShellClientKind::Xdg(data.wm_base.clone())
            }
            PopupKind::ZxdgV6(ref p) => {
                let data = p
                    .as_ref()
                    .user_data()
                    .get::<self::zxdgv6_handlers::ShellSurfaceUserData>()
                    .unwrap();
                ShellClientKind::ZxdgV6(data.shell.clone())
            }
        };

        Some(ShellClient { kind: shell })
    }

    /// Send a configure event to this popup surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    ///
    /// You can manipulate the state that will be sent to the client with the [`with_pending_state`](#method.with_pending_state)
    /// method.
    pub fn send_configure(&self) {
        if let Some(surface) = self.get_surface() {
            let next_configure = compositor::with_states(surface, |states| {
                let mut attributes = states
                    .data_map
                    .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();
                if !attributes.initial_configure_sent || attributes.server_pending.is_some() {
                    let pending = attributes.server_pending.take().unwrap_or(attributes.current);

                    let configure = PopupConfigure {
                        state: pending,
                        serial: SERIAL_COUNTER.next_serial(),
                    };

                    attributes.pending_configures.push(configure);
                    attributes.initial_configure_sent = true;

                    Some(configure)
                } else {
                    None
                }
            })
            .unwrap_or(None);
            if let Some(configure) = next_configure {
                match self.shell_surface {
                    PopupKind::Xdg(ref p) => {
                        self::xdg_handlers::send_popup_configure(p, configure);
                    }
                    PopupKind::ZxdgV6(ref p) => {
                        self::zxdgv6_handlers::send_popup_configure(p, configure);
                    }
                }
            }
        }
    }

    /// Handles the role specific commit logic
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn commit_hook(surface: &wl_surface::WlSurface) {
        let send_error_to = compositor::with_states(surface, |states| {
            let attributes = states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.parent.is_none() {
                attributes.popup_handle.clone()
            } else {
                None
            }
        })
        .unwrap_or(None);
        if let Some(handle) = send_error_to {
            match handle {
                PopupKind::Xdg(ref s) => {
                    let data = s
                        .as_ref()
                        .user_data()
                        .get::<self::xdg_handlers::ShellSurfaceUserData>()
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
                        .get::<self::zxdgv6_handlers::ShellSurfaceUserData>()
                        .unwrap();
                    data.xdg_surface.as_ref().post_error(
                        zxdg_surface_v6::Error::NotConstructed as u32,
                        "Surface has not been configured yet.".into(),
                    );
                }
            }
            return;
        }

        compositor::with_states(surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.initial_configure_sent {
                if let Some(state) = attributes.last_acked {
                    if state != attributes.current {
                        attributes.current = state;
                    }
                }
            }
            !attributes.initial_configure_sent
        })
        .unwrap();
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
        let configured = compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .configured
        })
        .unwrap();
        if !configured {
            match self.shell_surface {
                PopupKind::Xdg(ref s) => {
                    let data = s
                        .as_ref()
                        .user_data()
                        .get::<self::xdg_handlers::ShellSurfaceUserData>()
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
                        .get::<self::zxdgv6_handlers::ShellSurfaceUserData>()
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
    /// Returns `None` if the popup surface actually no longer exists.
    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        if self.alive() {
            Some(&self.wl_surface)
        } else {
            None
        }
    }

    /// Allows the pending state of this popup to
    /// be manipulated.
    ///
    /// This should be used to inform the client about size and position changes,
    /// for example after a move of the parent toplevel.
    ///
    /// The state will be sent to the client when calling [`send_configure`](#method.send_configure).
    pub fn with_pending_state<F, T>(&self, f: F) -> Result<T, DeadResource>
    where
        F: FnOnce(&mut PopupState) -> T,
    {
        if !self.alive() {
            return Err(DeadResource);
        }

        Ok(compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.server_pending.is_none() {
                attributes.server_pending = Some(attributes.current);
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
        .unwrap())
    }
}

/// A configure message for toplevel surfaces
#[derive(Debug, Clone)]
pub struct ToplevelConfigure {
    /// The state associated with this configure
    pub state: ToplevelState,

    /// A serial number to track ACK from the client
    ///
    /// This should be an ever increasing number, as the ACK-ing
    /// from a client for a serial will validate all pending lower
    /// serials.
    pub serial: Serial,
}

/// A configure message for popup surface
#[derive(Debug, Clone, Copy)]
pub struct PopupConfigure {
    /// The state associated with this configure,
    pub state: PopupState,
    /// A serial number to track ACK from the client
    ///
    /// This should be an ever increasing number, as the ACK-ing
    /// from a client for a serial will validate all pending lower
    /// serials.
    pub serial: Serial,
}

/// Defines the possible configure variants
/// for a XdgSurface that will be issued in
/// the user_impl for notifying about a ack_configure
#[derive(Debug)]
pub enum Configure {
    /// A xdg_surface with a role of xdg_toplevel
    /// has processed an ack_configure request
    Toplevel(ToplevelConfigure),
    /// A xdg_surface with a role of xdg_popup
    /// has processed an ack_configure request
    Popup(PopupConfigure),
}

impl From<ToplevelConfigure> for Configure {
    fn from(configure: ToplevelConfigure) -> Self {
        Configure::Toplevel(configure)
    }
}

impl From<PopupConfigure> for Configure {
    fn from(configure: PopupConfigure) -> Self {
        Configure::Popup(configure)
    }
}

/// Events generated by xdg shell surfaces
///
/// These are events that the provided implementation cannot process
/// for you directly.
///
/// Depending on what you want to do, you might ignore some of them
#[derive(Debug)]
pub enum XdgRequest {
    /// A new shell client was instantiated
    NewClient {
        /// the client
        client: ShellClient,
    },
    /// The pong for a pending ping of this shell client was received
    ///
    /// The `ShellHandler` already checked for you that the serial matches the one
    /// from the pending ping.
    ClientPong {
        /// the client
        client: ShellClient,
    },
    /// A new toplevel surface was created
    ///
    /// You likely need to send a [`ToplevelConfigure`] to the surface, to hint the
    /// client as to how its window should be sized.
    NewToplevel {
        /// the surface
        surface: ToplevelSurface,
    },
    /// A new popup surface was created
    ///
    /// You likely need to send a [`PopupConfigure`] to the surface, to hint the
    /// client as to how its popup should be sized.
    NewPopup {
        /// the surface
        surface: PopupSurface,
    },
    /// The client requested the start of an interactive move for this surface
    Move {
        /// the surface
        surface: ToplevelSurface,
        /// the seat associated to this move
        seat: wl_seat::WlSeat,
        /// the grab serial
        serial: Serial,
    },
    /// The client requested the start of an interactive resize for this surface
    Resize {
        /// The surface
        surface: ToplevelSurface,
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
        surface: PopupSurface,
        /// The seat to grab
        seat: wl_seat::WlSeat,
        /// The grab serial
        serial: Serial,
    },
    /// A toplevel surface requested to be maximized
    Maximize {
        /// The surface
        surface: ToplevelSurface,
    },
    /// A toplevel surface requested to stop being maximized
    UnMaximize {
        /// The surface
        surface: ToplevelSurface,
    },
    /// A toplevel surface requested to be set fullscreen
    Fullscreen {
        /// The surface
        surface: ToplevelSurface,
        /// The output (if any) on which the fullscreen is requested
        output: Option<wl_output::WlOutput>,
    },
    /// A toplevel surface request to stop being fullscreen
    UnFullscreen {
        /// The surface
        surface: ToplevelSurface,
    },
    /// A toplevel surface requested to be minimized
    Minimize {
        /// The surface
        surface: ToplevelSurface,
    },
    /// The client requests the window menu to be displayed on this surface at this location
    ///
    /// This menu belongs to the compositor. It is typically expected to contain options for
    /// control of the window (maximize/minimize/close/move/etc...).
    ShowWindowMenu {
        /// The surface
        surface: ToplevelSurface,
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
        configure: Configure,
    },
}
