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
//! let (shell_state, _) = xdg_shell_init(
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
use crate::utils::{user_data::UserDataMap, Logical, Point, Rectangle, Size};
use crate::wayland::compositor;
use crate::wayland::compositor::Cacheable;
use crate::wayland::shell::is_toplevel_equivalent;
use crate::wayland::{Serial, SERIAL_COUNTER};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use wayland_protocols::unstable::xdg_decoration::v1::server::zxdg_toplevel_decoration_v1;
use wayland_protocols::unstable::{xdg_decoration, xdg_shell};
use wayland_protocols::xdg_shell::server::xdg_surface;
use wayland_protocols::xdg_shell::server::xdg_wm_base::XdgWmBase;
use wayland_protocols::xdg_shell::server::{xdg_popup, xdg_positioner, xdg_toplevel, xdg_wm_base};
use wayland_server::backend::GlobalId;
use wayland_server::{
    protocol::{wl_output, wl_seat, wl_surface},
    DisplayHandle,
};
use wayland_server::{GlobalDispatch, Resource};

use super::PingError;

// pub mod decoration;

// handlers for the xdg_shell protocol
pub(super) mod xdg_handlers;
pub use xdg_handlers::{
    XdgPositionerUserData, XdgShellSurfaceUserData, XdgSurfaceUserData, XdgWmBaseUserData,
};

/// The role of an XDG toplevel surface.
///
/// If you are checking if the surface role is an xdg_toplevel, you should also check if the surface
/// is an [zxdg_toplevel] since the zxdg toplevel role is equivalent.
///
/// [zxdg_toplevel]: self::ZXDG_TOPLEVEL_ROLE
pub const XDG_TOPLEVEL_ROLE: &str = "xdg_toplevel";

/// The role of an XDG popup surface.
///
/// If you are checking if the surface role is an xdg_popup, you should also check if the surface
/// is a [zxdg_popup] since the zxdg popup role is equivalent.
///
/// [zxdg_popup]: self::ZXDG_POPUP_ROLE
pub const XDG_POPUP_ROLE: &str = "xdg_popup";

/// The role of an ZXDG toplevel surface.
///
/// If you are checking if the surface role is an zxdg_toplevel, you should also check if the surface
/// is an [xdg_toplevel] since the xdg toplevel role is equivalent.
///
/// [xdg_toplevel]: self::XDG_TOPLEVEL_ROLE
pub const ZXDG_TOPLEVEL_ROLE: &str = "zxdg_toplevel";

/// The role of an ZXDG popup surface.
///
/// If you are checking if the surface role is an zxdg_popup, you should also check if the surface
/// is a [xdg_popup] since the xdg popup role is equivalent.
///
/// [xdg_popup]: self::XDG_POPUP_ROLE
pub const ZXDG_POPUP_ROLE: &str = "zxdg_popup";

/// Constant for toplevel state version checking
const XDG_TOPLEVEL_STATE_TILED_SINCE: u32 = 2;

macro_rules! xdg_role {
    ($state:ty,
     $(#[$configure_meta:meta])* $configure_name:ident $({$($(#[$configure_field_meta:meta])* $configure_field_vis:vis$configure_field_name:ident:$configure_field_type:ty),*}),*,
     $(#[$attributes_meta:meta])* $attributes_name:ident {$($(#[$attributes_field_meta:meta])* $attributes_field_vis:vis$attributes_field_name:ident:$attributes_field_type:ty),*}) => {

        $(#[$configure_meta])*
        pub struct $configure_name {
                /// The state associated with this configure
                pub state: $state,
                /// A serial number to track ACK from the client
                ///
                /// This should be an ever increasing number, as the ACK-ing
                /// from a client for a serial will validate all pending lower
                /// serials.
                pub serial: Serial,

                $($(
                    $(#[$configure_field_meta])*
                    $configure_field_vis $configure_field_name: $configure_field_type,
                )*)*
        }

        $(#[$attributes_meta])*
        pub struct $attributes_name {
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
                pending_configures: Vec<$configure_name>,
                /// Holds the pending state as set by the server.
                pub server_pending: Option<$state>,
                /// Holds the last server_pending state that has been acknowledged
                /// by the client. This state should be cloned to the current
                /// during a commit.
                pub last_acked: Option<$state>,
                /// Holds the current state after a successful commit.
                pub current: $state,

                $(
                    $(#[$attributes_field_meta])*
                    $attributes_field_vis $attributes_field_name: $attributes_field_type,
                )*
        }

        impl $attributes_name {
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

                // Save the state as the last acked state
                self.last_acked = Some(configure.state.clone());

                // Set the xdg_surface to configured
                self.configured = true;

                // Save the last configure serial as a reference
                self.configure_serial = Some(Serial::from(serial));

                // Clean old configures
                self.pending_configures.retain(|c| c.serial > serial);

                Some(configure.into())
            }

            /// Gets the latest state that has been configured
            /// on the server and sent to the client.
            ///
            /// The state includes all changes that have been
            /// made on the server, including all not yet
            /// acked or committed changes, but excludes the
            /// current [`server_pending`](#structfield.server_pending) state.
            ///
            /// This can be used for example to check if the
            /// [`server_pending`](#structfield.server_pending) state
            /// is different from the last configured.
            pub fn current_server_state(&self) -> &$state {
                // We check if there is already a non-acked pending
                // configure and use its state or otherwise we could
                // loose some state that was previously configured
                // and sent, but not acked before calling with_pending_state
                // again. If there is no pending state we try to use the
                // last acked state which could contain state changes
                // already acked but not committed to the current state.
                // In case no last acked state is available, which is
                // the case on the first configure we fallback to the
                // current state.
                // In both cases the state already contains all previous
                // sent states. This way all pending state is accumulated
                // into the current state.
                self.pending_configures
                    .last()
                    .map(|c| &c.state)
                    .or_else(|| self.last_acked.as_ref())
                    .unwrap_or(&self.current)
            }

            /// Check if the state has pending changes that have
            /// not been sent to the client.
            ///
            /// This differs from just checking if the [`server_pending`](#structfield.server_pending)
            /// state is [`Some`] in that it also checks if a current pending
            /// state is different from the [`current_server_state`](#method.current_server_state).
            pub fn has_pending_changes(&self) -> bool {
                self.server_pending.as_ref().map(|s| s != self.current_server_state()).unwrap_or(false)
            }
        }

        impl Default for $attributes_name {
            fn default() -> Self {
                Self {
                    configured: false,
                    configure_serial: None,
                    pending_configures: Vec::new(),
                    initial_configure_sent: false,
                    server_pending: None,
                    last_acked: None,
                    current: Default::default(),

                    $(
                        $attributes_field_name: Default::default(),
                    )*
                }
            }
        }
    };
}

xdg_role!(
    ToplevelState,
    /// A configure message for toplevel surfaces
    #[derive(Debug, Clone)]
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
    /// All active operations (e.g., move, resize) are cancelled and all
    /// attributes (e.g. title, state, stacking, ...) are discarded for
    /// an xdg_toplevel surface when it is unmapped. The xdg_toplevel returns to
    /// the state it had right after xdg_surface.get_toplevel. The client
    /// can re-map the toplevel by performing a commit without any buffer
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
        pub min_size: Size<i32, Logical>,
        /// Maximum size requested for this surface
        ///
        /// A value of 0 on an axis means this axis is not constrained
        pub max_size: Size<i32, Logical>
    }
);

xdg_role!(
    PopupState,
    /// A configure message for popup surface
    #[derive(Debug, Clone, Copy)]
    PopupConfigure {
        /// The token the client provided in the `xdg_popup::reposition`
        /// request. The token itself is opaque, and has no other special meaning.
        /// The token is sent in the corresponding `xdg_popup::repositioned` event.
        pub reposition_token: Option<u32>
    },
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

        /// Defines if the surface has received at least one commit
        ///
        /// This can be used to check for protocol errors, like
        /// checking if a popup requested a grab after it has been
        /// mapped.
        pub committed: bool,

        popup_handle: Option<xdg_popup::XdgPopup>
    }
);

/// Represents the state of the popup
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PopupState {
    /// The positioner state can be used by the compositor
    /// to calculate the best placement for the popup.
    ///
    /// For example the compositor should prevent that a popup
    /// is placed outside the visible rectangle of a output.
    pub positioner: PositionerState,
    /// Holds the geometry of the popup as defined by the positioner.
    ///
    /// `Rectangle::width` and `Rectangle::height` holds the size of the
    /// of the popup in surface-local coordinates and corresponds to the
    /// window geometry
    ///
    /// `Rectangle::x` and `Rectangle::y` holds the position of the popup
    /// The position is relative to the window geometry as defined by
    /// xdg_surface.set_window_geometry of the parent surface.
    pub geometry: Rectangle<i32, Logical>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// The state of a positioner, as set by the client
pub struct PositionerState {
    /// Size of the rectangle that needs to be positioned
    pub rect_size: Size<i32, Logical>,
    /// Anchor rectangle in the parent surface coordinates
    /// relative to which the surface must be positioned
    pub anchor_rect: Rectangle<i32, Logical>,
    /// Edges defining the anchor point
    pub anchor_edges: xdg_positioner::Anchor,
    /// Gravity direction for positioning the child surface
    /// relative to its anchor point
    pub gravity: xdg_positioner::Gravity,
    /// Adjustments to do if previous criteria constrain the
    /// surface
    pub constraint_adjustment: xdg_positioner::ConstraintAdjustment,
    /// Offset placement relative to the anchor point
    pub offset: Point<i32, Logical>,
    /// When set reactive, the surface is reconstrained if the conditions
    /// used for constraining changed, e.g. the parent window moved.
    ///
    /// If the conditions changed and the popup was reconstrained,
    /// an xdg_popup.configure event is sent with updated geometry,
    /// followed by an xdg_surface.configure event.
    pub reactive: bool,
    /// The parent window geometry the compositor should use when
    /// positioning the popup. The compositor may use this information
    /// to determine the future state the popup should be constrained using.
    /// If this doesn't match the dimension of the parent the popup is
    /// eventually positioned against, the behavior is undefined.
    ///
    /// The arguments are given in the surface-local coordinate space.
    pub parent_size: Option<Size<i32, Logical>>,
    /// The serial of an xdg_surface.configure event this positioner will
    /// be used in response to. The compositor may use this information
    /// together with set_parent_size to determine what future state the
    /// popup should be constrained using.
    pub parent_configure: Option<Serial>,
}

impl Default for PositionerState {
    fn default() -> Self {
        PositionerState {
            anchor_edges: xdg_positioner::Anchor::None,
            anchor_rect: Default::default(),
            constraint_adjustment: xdg_positioner::ConstraintAdjustment::empty(),
            gravity: xdg_positioner::Gravity::None,
            offset: Default::default(),
            rect_size: Default::default(),
            reactive: false,
            parent_size: None,
            parent_configure: None,
        }
    }
}

impl PositionerState {
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
    pub fn get_geometry(&self) -> Rectangle<i32, Logical> {
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
            loc: self.offset,
            size: self.rect_size,
        };

        // Defines the anchor point for the anchor rectangle. The specified anchor
        // is used derive an anchor point that the child surface will be
        // positioned relative to. If a corner anchor is set (e.g. 'top_left' or
        // 'bottom_right'), the anchor point will be at the specified corner;
        // otherwise, the derived anchor point will be centered on the specified
        // edge, or in the center of the anchor rectangle if no edge is specified.
        if self.anchor_has_edge(xdg_positioner::Anchor::Top) {
            geometry.loc.y += self.anchor_rect.loc.y;
        } else if self.anchor_has_edge(xdg_positioner::Anchor::Bottom) {
            geometry.loc.y += self.anchor_rect.loc.y + self.anchor_rect.size.h;
        } else {
            geometry.loc.y += self.anchor_rect.loc.y + self.anchor_rect.size.h / 2;
        }

        if self.anchor_has_edge(xdg_positioner::Anchor::Left) {
            geometry.loc.x += self.anchor_rect.loc.x;
        } else if self.anchor_has_edge(xdg_positioner::Anchor::Right) {
            geometry.loc.x += self.anchor_rect.loc.x + self.anchor_rect.size.w;
        } else {
            geometry.loc.x += self.anchor_rect.loc.x + self.anchor_rect.size.w / 2;
        }

        // Defines in what direction a surface should be positioned, relative to
        // the anchor point of the parent surface. If a corner gravity is
        // specified (e.g. 'bottom_right' or 'top_left'), then the child surface
        // will be placed towards the specified gravity; otherwise, the child
        // surface will be centered over the anchor point on any axis that had no
        // gravity specified.
        if self.gravity_has_edge(xdg_positioner::Gravity::Top) {
            geometry.loc.y -= geometry.size.h;
        } else if !self.gravity_has_edge(xdg_positioner::Gravity::Bottom) {
            geometry.loc.y -= geometry.size.h / 2;
        }

        if self.gravity_has_edge(xdg_positioner::Gravity::Left) {
            geometry.loc.x -= geometry.size.w;
        } else if !self.gravity_has_edge(xdg_positioner::Gravity::Right) {
            geometry.loc.x -= geometry.size.w / 2;
        }

        geometry
    }
}

/// State of a regular toplevel surface
#[derive(Debug, Default, PartialEq)]
pub struct ToplevelState {
    /// The suggested size of the surface
    pub size: Option<Size<i32, Logical>>,

    /// The states for this surface
    pub states: ToplevelStateSet,

    /// The output for a fullscreen display
    pub fullscreen_output: Option<wl_output::WlOutput>,

    /// The xdg decoration mode of the surface
    pub decoration_mode: Option<zxdg_toplevel_decoration_v1::Mode>,
}

impl Clone for ToplevelState {
    fn clone(&self) -> ToplevelState {
        ToplevelState {
            fullscreen_output: self.fullscreen_output.clone(),
            states: self.states.clone(),
            size: self.size,
            decoration_mode: self.decoration_mode,
        }
    }
}

/// Container holding the states for a `XdgToplevel`
///
/// This container will prevent the `XdgToplevel` from
/// having the same `xdg_toplevel::State` multiple times
/// and simplifies setting and un-setting a particularly
/// `xdg_toplevel::State`
#[derive(Debug, Default, Clone, PartialEq)]
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

    /// Filter the states according to the provided version
    /// of the [`XdgToplevel`]
    pub(crate) fn into_filtered_states(self, version: u32) -> Vec<xdg_toplevel::State> {
        // If the client version supports the tiled states
        // we can directly return the states which will save
        // us from allocating another vector
        if version >= XDG_TOPLEVEL_STATE_TILED_SINCE {
            return self.states;
        }

        let is_tiled = |state: &xdg_toplevel::State| {
            matches!(
                state,
                xdg_toplevel::State::TiledTop
                    | xdg_toplevel::State::TiledBottom
                    | xdg_toplevel::State::TiledLeft
                    | xdg_toplevel::State::TiledRight
            )
        };

        let contains_tiled = self.states.iter().any(|state| is_tiled(state));

        // If the states do not contain a tiled state
        // we can directly return the states which will save
        // us from allocating another vector
        if !contains_tiled {
            return self.states;
        }

        // We need to filter out the unsupported states
        self.states.into_iter().filter(|state| !is_tiled(state)).collect()
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
#[derive(Debug, Default, Clone, Copy)]
pub struct SurfaceCachedState {
    /// Holds the double-buffered geometry that may be specified
    /// by xdg_surface.set_window_geometry.
    pub geometry: Option<Rectangle<i32, Logical>>,
    /// Minimum size requested for this surface
    ///
    /// A value of 0 on an axis means this axis is not constrained
    ///
    /// This is only relevant for xdg_toplevel, and will always be
    /// `(0, 0)` for xdg_popup.
    pub min_size: Size<i32, Logical>,
    /// Maximum size requested for this surface
    ///
    /// A value of 0 on an axis means this axis is not constrained
    ///
    /// This is only relevant for xdg_toplevel, and will always be
    /// `(0, 0)` for xdg_popup.
    pub max_size: Size<i32, Logical>,
}

impl<D> Cacheable<D> for SurfaceCachedState {
    fn commit(&mut self, cx: &mut DisplayHandle<'_, D>) -> Self {
        *self
    }
    fn merge_into(self, into: &mut Self, cx: &mut DisplayHandle<'_, D>) {
        *into = self;
    }
}

// pub(crate) struct ShellData {
//     log: ::slog::Logger,
//     // user_impl: Rc<RefCell<dyn FnMut(XdgRequest, DispatchData<'_>)>>,
//     shell_state: Arc<Mutex<XdgShellState>>,
// }

// impl Clone for ShellData {
//     fn clone(&self) -> Self {
//         ShellData {
//             log: self.log.clone(),
//             // user_impl: self.user_impl.clone(),
//             shell_state: self.shell_state.clone(),
//         }
//     }
// }

pub trait XdgShellHandler<D> {
    fn request(&mut self, cx: &mut DisplayHandle<'_, D>, request: XdgRequest);
}
pub struct XdgShellDispatch<'a, D, H: XdgShellHandler<D>>(pub &'a mut XdgShellState<D>, pub &'a mut H);

#[derive(Debug)]
pub(crate) struct InnerState {
    known_toplevels: Vec<ToplevelSurface>,
    known_popups: Vec<PopupSurface>,
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
#[derive(Debug)]
pub struct XdgShellState<D> {
    inner: Arc<Mutex<InnerState>>,

    log: slog::Logger,
    _ph: PhantomData<D>,
}

impl<D> XdgShellState<D> {
    /// Create a new `xdg_shell` global
    pub fn new<L>(display: &mut DisplayHandle<'_, D>, logger: L) -> (XdgShellState<D>, GlobalId)
    where
        L: Into<Option<::slog::Logger>>,
        D: GlobalDispatch<XdgWmBase, GlobalData = ()> + 'static,
    {
        let log = crate::slog_or_fallback(logger);
        let shell_state = XdgShellState {
            inner: Arc::new(Mutex::new(InnerState {
                known_toplevels: Vec::new(),
                known_popups: Vec::new(),
            })),

            log: log.new(slog::o!("smithay_module" => "xdg_shell_handler")),
            _ph: PhantomData::<D>,
        };

        let xdg_shell_global = display.create_global(
            3,
            (),
            // Filter::new(move |(shell, _version), _, dispatch_data| {
            //     self::xdg_handlers::implement_wm_base(shell, &shell_data, dispatch_data);
            // }),
        );

        (shell_state, xdg_shell_global)
    }

    /// Access all the shell surfaces known by this handler
    pub fn toplevel_surfaces<T, F: FnMut(&[ToplevelSurface]) -> T>(&self, mut cb: F) -> T {
        cb(&self.inner.lock().unwrap().known_toplevels)
    }

    // /// Returns a reference to the toplevel surface mapped to the provided wl_surface.
    // pub fn toplevel_surface(&self, surface: &wl_surface::WlSurface) -> Option<&ToplevelSurface> {
    //     self.known_toplevels
    //         .iter()
    //         .find(|toplevel| toplevel.wl_surface == *surface)
    // }

    // /// Access all the popup surfaces known by this handler
    // pub fn popup_surfaces(&self) -> &[PopupSurface] {
    //     &self.known_popups[..]
    // }
}

#[derive(Default, Debug)]
pub(crate) struct ShellClientData {
    pending_ping: Option<Serial>,
    data: UserDataMap,
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
    kind: xdg_wm_base::XdgWmBase,
}

impl std::cmp::PartialEq for ShellClient {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl ShellClient {
    fn new(resource: &xdg_wm_base::XdgWmBase) -> Self {
        Self {
            kind: resource.clone(),
        }
    }

    /// Is the shell client represented by this handle still connected?
    pub fn alive<D>(&self, cx: &mut DisplayHandle<'_, D>) -> bool {
        cx.object_info(self.kind.id()).is_ok()
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
    pub fn send_ping<D>(&self, cx: &mut DisplayHandle<'_, D>, serial: Serial) -> Result<(), PingError> {
        if !self.alive(cx) {
            return Err(PingError::DeadSurface);
        }
        let user_data = self.kind.data::<self::xdg_handlers::XdgWmBaseUserData>().unwrap();
        let mut guard = user_data.client_data.lock().unwrap();
        if let Some(pending_ping) = guard.pending_ping {
            return Err(PingError::PingAlreadyPending(pending_ping));
        }
        guard.pending_ping = Some(serial);
        self.kind.ping(cx, serial.into());

        Ok(())
    }

    /// Access the user data associated with this shell client
    pub fn with_data<D, F, T>(
        &self,
        cx: &mut DisplayHandle<'_, D>,
        f: F,
    ) -> Result<T, crate::utils::DeadResource>
    where
        F: FnOnce(&mut UserDataMap) -> T,
    {
        if !self.alive(cx) {
            return Err(crate::utils::DeadResource);
        }
        let data = self.kind.data::<self::xdg_handlers::XdgWmBaseUserData>().unwrap();
        let mut guard = data.client_data.lock().unwrap();
        Ok(f(&mut guard.data))
    }
}

/// A handle to a toplevel surface
#[derive(Debug, Clone)]
pub struct ToplevelSurface {
    wl_surface: wl_surface::WlSurface,
    shell_surface: xdg_toplevel::XdgToplevel,
}

impl std::cmp::PartialEq for ToplevelSurface {
    fn eq(&self, other: &Self) -> bool {
        // self.alive() && other.alive() &&
        self.wl_surface == other.wl_surface
    }
}

impl ToplevelSurface {
    /// Is the toplevel surface referred by this handle still alive?
    pub fn alive<D>(&self, cx: &mut DisplayHandle<'_, D>) -> bool {
        let a = cx.object_info(self.shell_surface.id()).is_ok();
        let b = cx.object_info(self.wl_surface.id()).is_ok();
        a && b
    }

    /// Supported XDG shell protocol version.
    pub fn version(&self) -> u32 {
        self.shell_surface.version()
    }

    /// Retrieve the shell client owning this toplevel surface
    ///
    /// Returns `None` if the surface does actually no longer exist.
    pub fn client<D>(&self, cx: &mut DisplayHandle<'_, D>) -> Option<ShellClient> {
        if !self.alive(cx) {
            return None;
        }

        let shell = {
            let data = self
                .shell_surface
                .data::<self::xdg_handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.wm_base.clone()
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

        // Check if the state really changed, it is possible
        // that with_pending_state has been called without
        // modifying the state.
        if !attributes.has_pending_changes() {
            return None;
        }

        attributes.server_pending.take()
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    ///
    /// You can manipulate the state that will be sent to the client with the [`with_pending_state`](#method.with_pending_state)
    /// method.
    pub fn send_configure<D: 'static>(&self, cx: &mut DisplayHandle<'_, D>) {
        if let Some(surface) = self.get_surface(cx) {
            let configure = compositor::with_states::<D, _, _>(surface, |states| {
                let mut attributes = states
                    .data_map
                    .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();
                if let Some(pending) = self.get_pending_state(&mut *attributes) {
                    // Retrieve the last configured decoration mode and test
                    // if the mode has changed.
                    // We have to do this check before adding the pending state
                    // to the pending configures.
                    let decoration_mode_changed =
                        pending.decoration_mode != attributes.current_server_state().decoration_mode;

                    let configure = ToplevelConfigure {
                        serial: SERIAL_COUNTER.next_serial(),
                        state: pending,
                    };

                    attributes.pending_configures.push(configure.clone());
                    attributes.initial_configure_sent = true;

                    Some((configure, decoration_mode_changed))
                } else {
                    None
                }
            })
            .unwrap_or(None);
            if let Some((configure, decoration_mode_changed)) = configure {
                if decoration_mode_changed {
                    if let Some(data) = self.shell_surface.data::<XdgShellSurfaceUserData>() {
                        if let Some(decoration) = &*data.decoration.lock().unwrap() {
                            // TODO:
                            // self::decoration::send_decoration_configure(
                            //     decoration,
                            //     configure.state.decoration_mode.unwrap_or(
                            //         xdg_decoration::v1::server::zxdg_toplevel_decoration_v1::Mode::ClientSide,
                            //     ),
                            // );
                        }
                    }
                }

                self::xdg_handlers::send_toplevel_configure(cx, &self.shell_surface, configure)
            }
        }
    }

    /// Handles the role specific commit logic
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn commit_hook<D: 'static>(surface: &wl_surface::WlSurface) {
        compositor::with_states::<D, _, _>(surface, |states| {
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
    pub fn ensure_configured<D: 'static>(&self, cx: &mut DisplayHandle<'_, D>) -> bool {
        if !self.alive(cx) {
            return false;
        }
        let configured = compositor::with_states::<D, _, _>(&self.wl_surface, |states| {
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
            let data = self
                .shell_surface
                .data::<self::xdg_handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.xdg_surface.post_error(
                cx,
                xdg_surface::Error::NotConstructed,
                "Surface has not been configured yet.",
            );
        }
        configured
    }

    /// Send a "close" event to the client
    pub fn send_close<D>(&self, cx: &mut DisplayHandle<'_, D>) {
        self.shell_surface.close(cx)
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    ///
    /// Returns `None` if the toplevel surface actually no longer exists.
    pub fn get_surface<D>(&self, cx: &mut DisplayHandle<'_, D>) -> Option<&wl_surface::WlSurface> {
        if self.alive(cx) {
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
    pub fn with_pending_state<D, F, T>(&self, cx: &mut DisplayHandle<'_, D>, f: F) -> Result<T, DeadResource>
    where
        F: FnOnce(&mut ToplevelState) -> T,
        D: 'static,
    {
        if !self.alive(cx) {
            return Err(DeadResource);
        }

        Ok(compositor::with_states::<D, _, _>(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.server_pending.is_none() {
                attributes.server_pending = Some(attributes.current_server_state().clone());
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
    pub fn current_state<D: 'static>(&self, cx: &mut DisplayHandle<'_, D>) -> Option<ToplevelState> {
        if !self.alive(cx) {
            return None;
        }

        Some(
            compositor::with_states::<D, _, _>(&self.wl_surface, |states| {
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

    /// Returns the parent of this toplevel surface.
    pub fn parent<D: 'static>(&self) -> Option<wl_surface::WlSurface> {
        xdg_handlers::get_parent::<D>(&self.shell_surface)
    }

    /// Sets the parent of this toplevel surface and returns whether the parent was successfully set.
    ///
    /// The parent must be another toplevel equivalent surface.
    ///
    /// If the parent is `None`, the parent-child relationship is removed.
    pub fn set_parent<D: 'static>(
        &self,
        cx: &mut DisplayHandle<'_, D>,
        parent: Option<&wl_surface::WlSurface>,
    ) -> bool {
        if let Some(parent) = parent {
            if !is_toplevel_equivalent(cx, parent) {
                return false;
            }
        }

        // Unset the parent
        xdg_handlers::set_parent::<D>(&self.shell_surface, None);

        true
    }
}

/// Represents the possible errors that
/// can be returned from [`PopupSurface::send_configure`]
#[derive(Debug, thiserror::Error)]
pub enum PopupConfigureError {
    /// The popup has already been configured and the
    /// protocol version forbids the popup to
    /// be re-configured
    #[error("The popup has already been configured")]
    AlreadyConfigured,
    /// The popup is not allowed to be re-configured,
    /// the positioner is not reactive
    #[error("The popup positioner is not reactive")]
    NotReactive,
}

/// A handle to a popup surface
///
/// This is an unified abstraction over the popup surfaces
/// of both `wl_shell` and `xdg_shell`.
#[derive(Debug, Clone)]
pub struct PopupSurface {
    wl_surface: wl_surface::WlSurface,
    shell_surface: xdg_popup::XdgPopup,
}

impl std::cmp::PartialEq for PopupSurface {
    fn eq(&self, other: &Self) -> bool {
        // self.alive() && other.alive() &&
        self.wl_surface == other.wl_surface
    }
}

impl PopupSurface {
    /// Is the popup surface referred by this handle still alive?
    pub fn alive<D>(&self, cx: &mut DisplayHandle<'_, D>) -> bool {
        let a = cx.object_info(self.shell_surface.id()).is_ok();
        let b = cx.object_info(self.wl_surface.id()).is_ok();

        a && b
    }

    /// Gets a reference of the parent WlSurface of
    /// this popup.
    pub fn get_parent_surface<D: 'static>(
        &self,
        cx: &mut DisplayHandle<'_, D>,
    ) -> Option<wl_surface::WlSurface> {
        if !self.alive(cx) {
            None
        } else {
            compositor::with_states::<D, _, _>(&self.wl_surface, |states| {
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
    pub fn client<D>(&self, cx: &mut DisplayHandle<'_, D>) -> Option<ShellClient> {
        if !self.alive(cx) {
            return None;
        }

        let shell = {
            let data = self
                .shell_surface
                .data::<self::xdg_handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.wm_base.clone()
        };

        Some(ShellClient { kind: shell })
    }

    /// Get the version of the popup resource
    fn version(&self) -> u32 {
        self.shell_surface.version()
    }

    /// Internal configure function to re-use the configure
    /// logic for both [`XdgRequest::send_configure`] and [`XdgRequest::send_repositioned`]
    fn send_configure_internal<D: 'static>(
        &self,
        cx: &mut DisplayHandle<'_, D>,
        reposition_token: Option<u32>,
    ) {
        if let Some(surface) = self.get_surface(cx) {
            let next_configure = compositor::with_states::<D, _, _>(surface, |states| {
                let mut attributes = states
                    .data_map
                    .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();

                if !attributes.initial_configure_sent
                    || attributes.has_pending_changes()
                    || reposition_token.is_some()
                {
                    let pending = attributes
                        .server_pending
                        .take()
                        .unwrap_or_else(|| *attributes.current_server_state());

                    let configure = PopupConfigure {
                        state: pending,
                        serial: SERIAL_COUNTER.next_serial(),
                        reposition_token,
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
                self::xdg_handlers::send_popup_configure(cx, &self.shell_surface, configure);
            }
        }
    }

    /// Send a configure event to this popup surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    ///
    /// You can manipulate the state that will be sent to the client with the [`with_pending_state`](#method.with_pending_state)
    /// method.
    ///
    /// Returns [`Err(PopupConfigureError)`] if the initial configure has already been sent and
    /// the client protocol version disallows a re-configure or the current [`PositionerState`]
    /// is not reactive
    pub fn send_configure<D: 'static>(
        &self,
        cx: &mut DisplayHandle<'_, D>,
    ) -> Result<(), PopupConfigureError> {
        if let Some(surface) = self.get_surface(cx) {
            // Check if we are allowed to send a configure
            compositor::with_states::<D, _, _>(surface, |states| {
                let attributes = states
                    .data_map
                    .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();

                if attributes.initial_configure_sent && self.version() < xdg_popup::EVT_REPOSITIONED_SINCE {
                    // Return error, initial configure already sent and client
                    // does not support re-configure
                    return Err(PopupConfigureError::AlreadyConfigured);
                }

                let is_reactive = attributes.current.positioner.reactive;

                if attributes.initial_configure_sent && !is_reactive {
                    // Return error, the positioner does not allow re-configure
                    return Err(PopupConfigureError::NotReactive);
                }

                Ok(())
            })
            .unwrap_or(Ok(()))?;

            self.send_configure_internal(cx, None);
        }

        Ok(())
    }

    /// Send a configure event, including the `repositioned` event to the client
    /// in response to a `reposition` request.
    ///
    /// For further information see [`send_configure`](#method.send_configure)
    pub fn send_repositioned<D: 'static>(&self, cx: &mut DisplayHandle<'_, D>, token: u32) {
        self.send_configure_internal(cx, Some(token))
    }

    /// Handles the role specific commit logic
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn commit_hook<D: 'static>(surface: &wl_surface::WlSurface) {
        let send_error_to = compositor::with_states::<D, _, _>(surface, |states| {
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
            let data = handle
                .data::<self::xdg_handlers::XdgShellSurfaceUserData>()
                .unwrap();
            // TODO:
            // data.xdg_surface.post_error(
            //     cx,
            //     xdg_surface::Error::NotConstructed,
            //     "Surface has not been configured yet.",
            // );
            return;
        }

        compositor::with_states::<D, _, _>(surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            attributes.committed = true;
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
    pub fn ensure_configured<D: 'static>(&self, cx: &mut DisplayHandle<'_, D>) -> bool {
        if !self.alive(cx) {
            return false;
        }
        let configured = compositor::with_states::<D, _, _>(&self.wl_surface, |states| {
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
            let data = self
                .shell_surface
                .data::<self::xdg_handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.xdg_surface.post_error(
                cx,
                xdg_surface::Error::NotConstructed,
                "Surface has not been configured yet.",
            );
        }
        configured
    }

    /// Send a `popup_done` event to the popup surface
    ///
    /// It means that the use has dismissed the popup surface, or that
    /// the pointer has left the area of popup grab if there was a grab.
    pub fn send_popup_done<D>(&self, cx: &mut DisplayHandle<'_, D>) {
        if !self.alive(cx) {
            return;
        }

        self.shell_surface.popup_done(cx);
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    ///
    /// Returns `None` if the popup surface actually no longer exists.
    pub fn get_surface<D>(&self, cx: &mut DisplayHandle<'_, D>) -> Option<&wl_surface::WlSurface> {
        if self.alive(cx) {
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
    pub fn with_pending_state<D, F, T>(&self, cx: &mut DisplayHandle<'_, D>, f: F) -> Result<T, DeadResource>
    where
        F: FnOnce(&mut PopupState) -> T,
        D: 'static,
    {
        if !self.alive(cx) {
            return Err(DeadResource);
        }

        Ok(compositor::with_states::<D, _, _>(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.server_pending.is_none() {
                attributes.server_pending = Some(*attributes.current_server_state());
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
        .unwrap())
    }
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
        /// The state of the positioner at the time
        /// the popup was requested.
        ///
        /// The positioner state can be used by the compositor
        /// to calculate the best placement for the popup.
        ///
        /// For example the compositor should prevent that a popup
        /// is placed outside the visible rectangle of a output.
        positioner: PositionerState,
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
        /// location of the menu request relative to the surface geometry
        location: Point<i32, Logical>,
    },
    /// A surface has acknowledged a configure serial.
    AckConfigure {
        /// The surface.
        surface: wl_surface::WlSurface,
        /// The configure serial.
        configure: Configure,
    },
    /// A client requested a reposition, providing a new
    /// positioner, of a popup.
    RePosition {
        /// The popup for which a reposition has been requested
        surface: PopupSurface,
        /// The state of the positioner at the time
        /// the reposition request was made.
        ///
        /// The positioner state can be used by the compositor
        /// to calculate the best placement for the popup.
        ///
        /// For example the compositor should prevent that a popup
        /// is placed outside the visible rectangle of a output.
        positioner: PositionerState,
        /// The passed token will be sent in the corresponding xdg_popup.repositioned event.
        /// The new popup position will not take effect until the corresponding configure event
        /// is acknowledged by the client. See xdg_popup.repositioned for details.
        /// The token itself is opaque, and has no other special meaning.
        token: u32,
    },
}
