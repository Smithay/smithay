//! Utilities for handling shell surfaces with the `xdg_shell` protocol
//!
//! This module provides automatic handling of shell surfaces objects, by being registered
//! as a global handler for `xdg_shell`.
//!
//! ## Why use this implementation
//!
//! This implementation can track for you the various shell surfaces defined by the
//! clients by handling the `xdg_shell` protocol.
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
//! To initialize this handler, create [`XdgShellState`], store it in your `State` struct and
//! implement the [`XdgShellHandler`], as shown in this example:
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::delegate_xdg_shell;
//! use smithay::reexports::wayland_server::protocol::{wl_seat, wl_surface};
//! use smithay::wayland::shell::xdg::{XdgShellState, XdgShellHandler, ToplevelSurface, PopupSurface, PositionerState};
//! use smithay::utils::Serial;
//!
//! # struct State { xdg_shell_state: XdgShellState, seat_state: SeatState<Self> }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! let xdg_shell_state = XdgShellState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // insert the xdg_shell_state into your state
//! // ...
//!
//! // implement the necessary traits
//! impl XdgShellHandler for State {
//!     fn xdg_shell_state(&mut self) -> &mut XdgShellState {
//!         &mut self.xdg_shell_state
//!     }
//!
//!     // handle the shell requests here.
//!     // more optional methods can be used to further customized
//!     fn new_toplevel(&mut self, surface: ToplevelSurface) {
//!         // ...
//!     }
//!     fn new_popup(
//!         &mut self,
//!         surface: PopupSurface,
//!         positioner: PositionerState,
//!     ) {
//!         // ...
//!     }
//!     fn grab(
//!         &mut self,
//!         surface: PopupSurface,
//!         seat: wl_seat::WlSeat,
//!         serial: Serial,
//!     ) {
//!         // ...
//!     }
//!     fn reposition_request(
//!         &mut self,
//!         surface: PopupSurface,
//!         positioner: PositionerState,
//!         token: u32,
//!     ) {
//!         // ...
//!     }
//! }
//!
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//!
//! type Target = wl_surface::WlSurface;
//! impl SeatHandler for State {
//!     type KeyboardFocus = Target;
//!     type PointerFocus = Target;
//!     type TouchFocus = Target;
//!
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Target>) {
//!         // handle focus changes, if you need to ...
//!     }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
//!         // handle new images for the cursor ...
//!     }
//! }
//! delegate_xdg_shell!(State);
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
//!   [`XdgShellState`].
//! - [`PopupSurface`]:
//!   This is a handle representing a popup/tooltip surface. Similarly,
//!   you can get a list of all currently alive popup surface from the
//!   [`XdgShellState`].
//!
//! You'll obtain these objects though two means: either via the callback methods of
//! the [`XdgShellHandler`], or via methods on the [`XdgShellState`].

use crate::utils::alive_tracker::IsAlive;
use crate::utils::{user_data::UserDataMap, Logical, Point, Rectangle, Size};
use crate::utils::{Serial, SERIAL_COUNTER};
use crate::wayland::compositor;
use crate::wayland::compositor::Cacheable;
use std::cmp::min;
use std::{collections::HashSet, fmt::Debug, sync::Mutex};

use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use wayland_protocols::xdg::shell::server::xdg_positioner::{Anchor, ConstraintAdjustment, Gravity};
use wayland_protocols::xdg::shell::server::xdg_surface;
use wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase;
use wayland_protocols::xdg::shell::server::{xdg_popup, xdg_positioner, xdg_toplevel, xdg_wm_base};
use wayland_server::backend::GlobalId;
use wayland_server::{
    protocol::{wl_output, wl_seat, wl_surface},
    DisplayHandle, GlobalDispatch, Resource,
};

use super::PingError;

pub mod decoration;
pub mod dialog;

// handlers for the xdg_shell protocol
pub(super) mod handlers;
pub use handlers::{XdgPositionerUserData, XdgShellSurfaceUserData, XdgSurfaceUserData, XdgWmBaseUserData};

/// The role of an XDG toplevel surface.
pub const XDG_TOPLEVEL_ROLE: &str = "xdg_toplevel";

/// The role of an XDG popup surface.
pub const XDG_POPUP_ROLE: &str = "xdg_popup";

/// Constant for toplevel state version checking
const XDG_TOPLEVEL_STATE_TILED_SINCE: u32 = 2;
const XDG_TOPLEVEL_STATE_SUSPENDED_SINCE: u32 = 6;

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
                /// Holds the last acked configure serial at the time of the last successful
                /// commit. This serial corresponds to the current state.
                pub current_serial: Option<Serial>,
                /// Does the surface have a buffer (updated on every commit)
                has_buffer: bool,

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

            /// Returns a list of configures sent to, but not yet acknowledged by the client.
            ///
            /// The list is ordered by age, so the last configure in the list is the last one sent
            /// to the client.
            pub fn pending_configures(&self) -> &[$configure_name] {
                &self.pending_configures
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
                    current_serial: None,
                    has_buffer: false,

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
        /// Hints that the dialog has "modal" behavior.
        /// Modal dialogs typically require to be fully addressed by the user (i.e. closed)
        /// before resuming interaction with the parent toplevel, and may require a distinct presentation.
        ///
        /// This value has no effect on toplevels that are not attached to a parent toplevel.
        pub modal: bool,
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
        /// An `zxdg_toplevel_decoration_v1::configure` event has been sent
        /// to the client.
        pub initial_decoration_configure_sent: bool
    }
);

/// Data associated with XDG toplevel surface  
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     states.data_map.get::<XdgToplevelSurfaceData>();
/// });
/// ```
pub type XdgToplevelSurfaceData = Mutex<XdgToplevelSurfaceRoleAttributes>;

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

/// Data associated with XDG popup surface  
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::shell::xdg::XdgPopupSurfaceData;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     states.data_map.get::<XdgPopupSurfaceData>();
/// });
/// ```
pub type XdgPopupSurfaceData = Mutex<XdgPopupSurfaceRoleAttributes>;

/// Represents the state of the popup
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    /// Get the anchor point for a popup as defined by this positioner.
    ///
    /// Defined by `xdg_positioner.set_anchor_rect` and
    /// `xdg_positioner.set_anchor`.
    pub fn get_anchor_point(&self) -> Point<i32, Logical> {
        let mut point = self.anchor_rect.loc;

        point.y += if self.anchor_has_edge(xdg_positioner::Anchor::Top) {
            0
        } else if self.anchor_has_edge(xdg_positioner::Anchor::Bottom) {
            self.anchor_rect.size.h
        } else {
            self.anchor_rect.size.h / 2
        };

        point.x += if self.anchor_has_edge(xdg_positioner::Anchor::Left) {
            0
        } else if self.anchor_has_edge(xdg_positioner::Anchor::Right) {
            self.anchor_rect.size.w
        } else {
            self.anchor_rect.size.w / 2
        };

        point
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
    /// popup relative to its parent surface's `window_geometry`.
    /// The position is calculated according to the rules defined
    /// in the `xdg_shell` protocol.
    /// The `constraint_adjustment` will not be considered by this
    /// implementation and the position and size should be re-calculated
    /// in the compositor if the compositor implements `constraint_adjustment`
    ///
    /// [`PositionerState::get_unconstrained_geometry`] does take `constraint_adjustment` into account.
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
        let mut geometry = Rectangle::new(self.offset, self.rect_size);

        // Defines the anchor point for the anchor rectangle. The specified anchor
        // is used derive an anchor point that the child surface will be
        // positioned relative to. If a corner anchor is set (e.g. 'top_left' or
        // 'bottom_right'), the anchor point will be at the specified corner;
        // otherwise, the derived anchor point will be centered on the specified
        // edge, or in the center of the anchor rectangle if no edge is specified.
        geometry.loc += self.get_anchor_point();

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

    /// Get the geometry for a popup as defined by this positioner, after trying to fit the popup into the
    /// target rectangle.
    ///
    /// `Rectangle::width` and `Rectangle::height` corresponds to the size set by `xdg_positioner.set_size`.
    ///
    /// `Rectangle::x` and `Rectangle::y` define the position of the popup relative to its parent surface's
    /// `window_geometry`. The position is calculated according to the rules defined in the `xdg_shell`
    /// protocol.
    ///
    /// This method does consider `constrain_adjustment` by trying to fit the popup into the provided target
    /// rectangle. The target rectangle is in the same coordinate system as the rectangle returned by this
    /// method. So, it is relative to the parent surface's geometry.
    pub fn get_unconstrained_geometry(mut self, target: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        // The protocol defines the following order for adjustments: flip, slide, resize. If the flip fails
        // to remove the constraints, it is reverted.
        //
        // The adjustments are applied individually between axes. We can do that reasonably safely, given
        // that both our target and our popup are simple rectangles. The code is grouped per adjustment for
        // easier copy-paste checking, and because flips replace the geometry entirely, while further
        // adjustments change individual fields.
        let mut geo = self.get_geometry();
        let (mut off_left, mut off_right, mut off_top, mut off_bottom) = compute_offsets(target, geo);

        // Try to flip horizontally.
        if (off_left > 0 || off_right > 0) && self.constraint_adjustment.contains(ConstraintAdjustment::FlipX)
        {
            let mut new = self;
            new.anchor_edges = invert_anchor_x(new.anchor_edges);
            new.gravity = invert_gravity_x(new.gravity);
            let new_geo = new.get_geometry();
            let (new_off_left, new_off_right, _, _) = compute_offsets(target, new_geo);

            // Apply flip only if it removed the constraint.
            if new_off_left <= 0 && new_off_right <= 0 {
                self = new;
                geo = new_geo;
                off_left = 0;
                off_right = 0;
                // off_top and off_bottom are unchanged since we're using rectangles.
            }
        }

        // Try to flip vertically.
        if (off_top > 0 || off_bottom > 0) && self.constraint_adjustment.contains(ConstraintAdjustment::FlipY)
        {
            let mut new = self;
            new.anchor_edges = invert_anchor_y(new.anchor_edges);
            new.gravity = invert_gravity_y(new.gravity);
            let new_geo = new.get_geometry();
            let (_, _, new_off_top, new_off_bottom) = compute_offsets(target, new_geo);

            // Apply flip only if it removed the constraint.
            if new_off_top <= 0 && new_off_bottom <= 0 {
                self = new;
                geo = new_geo;
                off_top = 0;
                off_bottom = 0;
                // off_left and off_right are unchanged since we're using rectangles.
            }
        }

        // Try to slide horizontally.
        if (off_left > 0 || off_right > 0)
            && self.constraint_adjustment.contains(ConstraintAdjustment::SlideX)
        {
            // Prefer to show the top-left corner of the popup so that we can easily do a resize
            // adjustment next.
            if off_left > 0 {
                geo.loc.x += off_left;
            } else if off_right > 0 {
                geo.loc.x -= min(off_right, -off_left);
            }

            (off_left, off_right, _, _) = compute_offsets(target, geo);
            // off_top and off_bottom are the same since we're using rectangles.
        }

        // Try to slide vertically.
        if (off_top > 0 || off_bottom > 0)
            && self.constraint_adjustment.contains(ConstraintAdjustment::SlideY)
        {
            // Prefer to show the top-left corner of the popup so that we can easily do a resize
            // adjustment next.
            if off_top > 0 {
                geo.loc.y += off_top;
            } else if off_bottom > 0 {
                geo.loc.y -= min(off_bottom, -off_top);
            }

            (_, _, off_top, off_bottom) = compute_offsets(target, geo);
            // off_left and off_right are the same since we're using rectangles.
        }

        // Try to resize horizontally.
        if self.constraint_adjustment.contains(ConstraintAdjustment::ResizeX) {
            // Unconstrain both edges by clamping the left and right sides of the popup rectangle.
            // Skip if the offset is larger than the width, in which case the entirety of the popup
            // is outside the target rectangle and a clamp would result in a zero-sized geometry.

            if off_left > 0 && off_left < geo.size.w {
                geo.loc.x += off_left;
                geo.size.w -= off_left;
            }
            if off_right > 0 && off_right < geo.size.w {
                geo.size.w -= off_right;
            }
        }

        // Try to resize vertically.
        if self.constraint_adjustment.contains(ConstraintAdjustment::ResizeY) {
            // Unconstrain both edges by clamping the top and bottom sides of the popup rectangle.
            // Skip if the offset is larger than the height, in which case the entirety of the popup
            // is outside the target rectangle and a clamp would result in a zero-sized geometry.

            if off_top > 0 && off_top < geo.size.h {
                geo.loc.y += off_top;
                geo.size.h -= off_top;
            }
            if off_bottom > 0 && off_bottom < geo.size.h {
                geo.size.h -= off_bottom;
            }
        }

        geo
    }
}

fn compute_offsets(target: Rectangle<i32, Logical>, popup: Rectangle<i32, Logical>) -> (i32, i32, i32, i32) {
    let off_left = target.loc.x - popup.loc.x;
    let off_right = (popup.loc.x + popup.size.w) - (target.loc.x + target.size.w);
    let off_top = target.loc.y - popup.loc.y;
    let off_bottom = (popup.loc.y + popup.size.h) - (target.loc.y + target.size.h);
    (off_left, off_right, off_top, off_bottom)
}

fn invert_anchor_x(anchor: Anchor) -> Anchor {
    match anchor {
        Anchor::Left => Anchor::Right,
        Anchor::Right => Anchor::Left,
        Anchor::TopLeft => Anchor::TopRight,
        Anchor::TopRight => Anchor::TopLeft,
        Anchor::BottomLeft => Anchor::BottomRight,
        Anchor::BottomRight => Anchor::BottomLeft,
        x => x,
    }
}

fn invert_anchor_y(anchor: Anchor) -> Anchor {
    match anchor {
        Anchor::Top => Anchor::Bottom,
        Anchor::Bottom => Anchor::Top,
        Anchor::TopLeft => Anchor::BottomLeft,
        Anchor::TopRight => Anchor::BottomRight,
        Anchor::BottomLeft => Anchor::TopLeft,
        Anchor::BottomRight => Anchor::TopRight,
        x => x,
    }
}

fn invert_gravity_x(gravity: Gravity) -> Gravity {
    match gravity {
        Gravity::Left => Gravity::Right,
        Gravity::Right => Gravity::Left,
        Gravity::TopLeft => Gravity::TopRight,
        Gravity::TopRight => Gravity::TopLeft,
        Gravity::BottomLeft => Gravity::BottomRight,
        Gravity::BottomRight => Gravity::BottomLeft,
        x => x,
    }
}

fn invert_gravity_y(gravity: Gravity) -> Gravity {
    match gravity {
        Gravity::Top => Gravity::Bottom,
        Gravity::Bottom => Gravity::Top,
        Gravity::TopLeft => Gravity::BottomLeft,
        Gravity::TopRight => Gravity::BottomRight,
        Gravity::BottomLeft => Gravity::TopLeft,
        Gravity::BottomRight => Gravity::TopRight,
        x => x,
    }
}

/// State of a regular toplevel surface
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ToplevelState {
    /// The suggested size of the surface
    pub size: Option<Size<i32, Logical>>,

    /// The bounds for this toplevel
    ///
    /// The bounds can for example correspond to the size of a monitor excluding any panels or
    /// other shell components, so that a surface isn't created in a way that it cannot fit.
    pub bounds: Option<Size<i32, Logical>>,

    /// The states for this surface
    pub states: ToplevelStateSet,

    /// The output for a fullscreen display
    pub fullscreen_output: Option<wl_output::WlOutput>,

    /// The xdg decoration mode of the surface
    pub decoration_mode: Option<zxdg_toplevel_decoration_v1::Mode>,

    /// The wm capabilities for this toplevel
    pub capabilities: WmCapabilitySet,
}

impl Clone for ToplevelState {
    fn clone(&self) -> ToplevelState {
        ToplevelState {
            fullscreen_output: self.fullscreen_output.clone(),
            states: self.states.clone(),
            size: self.size,
            bounds: self.bounds,
            decoration_mode: self.decoration_mode,
            capabilities: self.capabilities.clone(),
        }
    }
}

/// Container holding the states for a `XdgToplevel`
///
/// This container will prevent the `XdgToplevel` from
/// having the same `xdg_toplevel::State` multiple times
/// and simplifies setting and un-setting a particularly
/// `xdg_toplevel::State`
#[derive(Debug, Default, Clone, PartialEq, Eq)]
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
        let tiled_supported = version >= XDG_TOPLEVEL_STATE_TILED_SINCE;
        let suspended_supported = version >= XDG_TOPLEVEL_STATE_SUSPENDED_SINCE;

        // If the client version supports the suspended states
        // we can directly return the states which will save
        // us from allocating another vector
        if suspended_supported {
            return self.states;
        }

        let is_suspended = |state: &xdg_toplevel::State| *state == xdg_toplevel::State::Suspended;
        let contains_suspended = self.states.contains(&xdg_toplevel::State::Suspended);

        // If tiled is supported and there is no suspend state there is nothing to filter out
        if tiled_supported && !contains_suspended {
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
        let contains_tiled = self.states.iter().any(is_tiled);

        // If it does not contain unsupported values
        if !contains_suspended && !contains_tiled {
            return self.states;
        }

        self.states
            .into_iter()
            .filter(|state| {
                if tiled_supported {
                    !is_suspended(state)
                } else {
                    !is_suspended(state) && !is_tiled(state)
                }
            })
            .collect()
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
    #[inline]
    fn from(states: ToplevelStateSet) -> Self {
        states.states
    }
}

/// Container holding the [`xdg_toplevel::WmCapabilities`] for a toplevel
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct WmCapabilitySet {
    capabilities: HashSet<xdg_toplevel::WmCapabilities>,
}

impl WmCapabilitySet {
    /// Returns `true` if the set contains a capability.
    pub fn contains(&self, capability: xdg_toplevel::WmCapabilities) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Adds a capability to the set.
    ///
    /// If the set did not have this capability present, `true` is returned.
    ///
    /// If the set did have this capability present, `false` is returned.
    pub fn set(&mut self, capability: xdg_toplevel::WmCapabilities) -> bool {
        self.capabilities.insert(capability)
    }

    /// Removes a capability from the set. Returns whether the capability was
    /// present in the set.
    pub fn unset(&mut self, capability: xdg_toplevel::WmCapabilities) -> bool {
        self.capabilities.remove(&capability)
    }

    /// Replace all capabilities in this set
    pub fn replace(&mut self, capabilities: impl IntoIterator<Item = xdg_toplevel::WmCapabilities>) {
        self.capabilities.clear();
        self.capabilities.extend(capabilities);
    }

    /// Returns the raw [`xdg_toplevel::WmCapabilities`] stored in this set
    pub fn capabilities(&self) -> impl Iterator<Item = &xdg_toplevel::WmCapabilities> {
        self.capabilities.iter()
    }
}

impl<T> From<T> for WmCapabilitySet
where
    T: IntoIterator<Item = xdg_toplevel::WmCapabilities>,
{
    #[inline]
    fn from(capabilities: T) -> Self {
        let capabilities = capabilities.into_iter().collect();
        Self { capabilities }
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

impl Cacheable for SurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }
    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// Xdg Shell handler type
#[allow(unused_variables)]
pub trait XdgShellHandler {
    /// [XdgShellState] getter
    fn xdg_shell_state(&mut self) -> &mut XdgShellState;

    /// A new shell client was instantiated
    fn new_client(&mut self, client: ShellClient) {}

    /// The pong for a pending ping of this shell client was received
    ///
    /// The `ShellHandler` already checked for you that the serial matches the one
    /// from the pending ping.
    fn client_pong(&mut self, client: ShellClient) {}

    /// A new toplevel surface was created
    ///
    /// You likely need to send a [`ToplevelConfigure`] to the surface, to hint the
    /// client as to how its window should be sized.
    fn new_toplevel(&mut self, surface: ToplevelSurface);

    /// A new popup surface was created
    ///
    /// You likely need to send a [`PopupConfigure`] to the surface, to hint the
    /// client as to how its popup should be sized.
    ///
    /// ## Arguments
    ///
    /// - `positioner` - The state of the positioner at the timethe popup was requested.
    ///   The positioner state can be used by the compositor
    ///   to calculate the best placement for the popup.
    ///   For example the compositor should prevent that a popup
    ///   is placed outside the visible rectangle of a output.
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState);

    /// The client requested the start of an interactive move for this surface
    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {}

    /// The client requested the start of an interactive resize for this surface
    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
    }

    /// This popup requests a grab of the pointer
    ///
    /// This means it requests to be sent a `popup_done` event when the pointer leaves
    /// the grab area.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial);

    /// A toplevel surface requested to be maximized
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        surface.send_configure();
    }

    /// A toplevel surface requested to stop being maximized
    fn unmaximize_request(&mut self, surface: ToplevelSurface) {}

    /// A toplevel surface requested to be set fullscreen
    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<wl_output::WlOutput>) {
        surface.send_configure();
    }

    /// A toplevel surface request to stop being fullscreen
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {}

    /// A toplevel surface requested to be minimized
    fn minimize_request(&mut self, surface: ToplevelSurface) {}

    /// The client requests the window menu to be displayed on this surface at this location
    ///
    /// This menu belongs to the compositor. It is typically expected to contain options for
    /// control of the window (maximize/minimize/close/move/etc...).
    fn show_window_menu(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        location: Point<i32, Logical>,
    ) {
    }

    /// A surface has acknowledged a configure serial.
    fn ack_configure(&mut self, surface: wl_surface::WlSurface, configure: Configure) {}

    /// A client requested a reposition, providing a new positioner for a popup.
    ///
    /// To confirm the new popup position, `PopupSurface::send_repositioned` must be
    /// called on the provided surface with the token.
    ///
    /// ## Arguments
    ///
    /// - `positioner` - The state of the positioner at the time the reposition request was made.
    ///   The positioner state can be used by the compositor
    ///   to calculate the best placement for the popup.
    ///   For example the compositor should prevent that a popup
    ///   is placed outside the visible rectangle of a output.
    /// - `token` - The passed token will be sent in the corresponding xdg_popup.repositioned event.
    ///   The new popup position will not take effect until the corresponding configure event
    ///   is acknowledged by the client. See xdg_popup.repositioned for details.
    ///   The token itself is opaque, and has no other special meaning.
    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32);

    /// A shell client was destroyed.
    fn client_destroyed(&mut self, client: ShellClient) {}

    /// A toplevel surface was destroyed.
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {}

    /// A popup surface was destroyed.
    fn popup_destroyed(&mut self, surface: PopupSurface) {}

    /// The toplevel surface set a different app id.
    fn app_id_changed(&mut self, surface: ToplevelSurface) {}

    /// The toplevel surface set a different title.
    fn title_changed(&mut self, surface: ToplevelSurface) {}

    /// The parent of a toplevel surface has changed.
    fn parent_changed(&mut self, surface: ToplevelSurface) {}
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
#[derive(Debug)]
pub struct XdgShellState {
    known_toplevels: Vec<ToplevelSurface>,
    known_popups: Vec<PopupSurface>,
    default_capabilities: WmCapabilitySet,
    global: GlobalId,
}

impl XdgShellState {
    /// Create a new `xdg_shell` global with all [`WmCapabilities`](xdg_toplevel::WmCapabilities)
    pub fn new<D>(display: &DisplayHandle) -> XdgShellState
    where
        D: GlobalDispatch<XdgWmBase, ()> + 'static,
    {
        Self::new_with_capabilities::<D>(
            display,
            [
                xdg_toplevel::WmCapabilities::Fullscreen,
                xdg_toplevel::WmCapabilities::Maximize,
                xdg_toplevel::WmCapabilities::Minimize,
                xdg_toplevel::WmCapabilities::WindowMenu,
            ],
        )
    }

    /// Create a new `xdg_shell` global with a specific set of [`WmCapabilities`](xdg_toplevel::WmCapabilities)
    pub fn new_with_capabilities<D>(
        display: &DisplayHandle,
        capabilities: impl Into<WmCapabilitySet>,
    ) -> XdgShellState
    where
        D: GlobalDispatch<XdgWmBase, ()> + 'static,
    {
        let global = display.create_global::<D, XdgWmBase, _>(6, ());

        XdgShellState {
            known_toplevels: Vec::new(),
            known_popups: Vec::new(),
            default_capabilities: capabilities.into(),
            global,
        }
    }

    /// Replace the capabilities of this global
    ///
    /// *Note*: This does not update the capabilities on existing toplevels, only new
    /// toplevels are affected. To update existing toplevels iterate over them,
    /// update their capabilities and send a configure.
    pub fn replace_capabilities(&mut self, capabilities: impl Into<WmCapabilitySet>) {
        self.default_capabilities = capabilities.into();
    }

    /// Access all the shell surfaces known by this handler
    pub fn toplevel_surfaces(&self) -> &[ToplevelSurface] {
        &self.known_toplevels
    }

    /// Returns a [`ToplevelSurface`] from an underlying toplevel surface.
    pub fn get_toplevel(&self, toplevel: &xdg_toplevel::XdgToplevel) -> Option<ToplevelSurface> {
        self.known_toplevels
            .iter()
            .find(|surface| surface.xdg_toplevel() == toplevel)
            .cloned()
    }

    /// Access all the popup surfaces known by this handler
    pub fn popup_surfaces(&self) -> &[PopupSurface] {
        &self.known_popups
    }

    /// Returns a [`PopupSurface`] from an underlying popup surface.
    pub fn get_popup(&self, popup: &xdg_popup::XdgPopup) -> Option<PopupSurface> {
        self.known_popups
            .iter()
            .find(|surface| surface.xdg_popup() == popup)
            .cloned()
    }

    /// Returns the xdg shell global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
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
    #[inline]
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
    #[inline]
    pub fn alive(&self) -> bool {
        self.kind.alive()
    }

    /// Send a ping request to this shell client
    ///
    /// You'll receive the reply as a [`XdgShellHandler::client_pong`] request.
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
        let user_data = self.kind.data::<self::handlers::XdgWmBaseUserData>().unwrap();
        let mut guard = user_data.client_data.lock().unwrap();
        if let Some(pending_ping) = guard.pending_ping {
            return Err(PingError::PingAlreadyPending(pending_ping));
        }
        guard.pending_ping = Some(serial);
        self.kind.ping(serial.into());

        Ok(())
    }

    /// Kill the shell client for being unresponsive.
    ///
    /// Generally this will be used if the client does not respond to a ping in a reasonable amount of time.
    pub fn unresponsive(&self) -> Result<(), crate::utils::DeadResource> {
        if !self.alive() {
            return Err(crate::utils::DeadResource);
        }
        self.kind.post_error(
            xdg_wm_base::Error::Unresponsive,
            "client did not respond to ping on time",
        );

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
        let data = self.kind.data::<self::handlers::XdgWmBaseUserData>().unwrap();
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
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // self.alive() && other.alive() &&
        self.wl_surface == other.wl_surface
    }
}

impl ToplevelSurface {
    /// Is the toplevel surface referred by this handle still alive?
    #[inline]
    pub fn alive(&self) -> bool {
        self.wl_surface.alive() && self.shell_surface.alive()
    }

    /// Supported XDG shell protocol version.
    pub fn version(&self) -> u32 {
        self.shell_surface.version()
    }

    /// Retrieve the shell client owning this toplevel surface
    pub fn client(&self) -> ShellClient {
        let shell = {
            let data = self
                .shell_surface
                .data::<self::handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.wm_base.clone()
        };

        ShellClient { kind: shell }
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
            return Some(
                attributes
                    .server_pending
                    .take()
                    .unwrap_or_else(|| attributes.current_server_state().clone()),
            );
        }

        // Check if the state really changed, it is possible
        // that with_pending_state has been called without
        // modifying the state.
        if !attributes.has_pending_changes() {
            return None;
        }

        attributes.server_pending.take()
    }

    /// Send a pending configure event to this toplevel surface to suggest it a new configuration
    ///
    /// If changes have occurred a configure event will be send to the clients and the serial will be returned
    /// (for tracking the configure in [`XdgShellHandler::ack_configure`] if desired).
    /// If no changes occurred no event will be send and `None` will be returned.
    ///
    /// See [`send_configure`](ToplevelSurface::send_configure) and [`has_pending_changes`](ToplevelSurface::has_pending_changes)
    /// for more information.
    pub fn send_pending_configure(&self) -> Option<Serial> {
        if self.has_pending_changes() {
            Some(self.send_configure())
        } else {
            None
        }
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    ///
    /// You can manipulate the state that will be sent to the client with the [`with_pending_state`](#method.with_pending_state)
    /// method.
    ///
    /// Note: This will always send a configure event, if you intend to only send a configure event on changes take a look at
    /// [`send_pending_configure`](ToplevelSurface::send_pending_configure)
    pub fn send_configure(&self) -> Serial {
        let shell_surface_data = self.shell_surface.data::<XdgShellSurfaceUserData>();
        let decoration =
            shell_surface_data.and_then(|data| data.decoration.lock().unwrap().as_ref().cloned());
        let (configure, decoration_mode_changed, bounds_changed, capabilities_changed) =
            compositor::with_states(&self.wl_surface, |states| {
                let mut attributes = states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap();

                let pending = self
                    .get_pending_state(&mut attributes)
                    .unwrap_or_else(|| attributes.current_server_state().clone());
                // retrieve the current state before adding it to the
                // pending state so that we can compare what has changed
                let current = attributes.current_server_state();

                // test if we should send the decoration mode, either because it changed
                // or we never sent it
                let decoration_mode_changed = !attributes.initial_decoration_configure_sent
                    || (pending.decoration_mode != current.decoration_mode);

                // test if we should send a bounds configure event, either because the
                // bounds changed or we never sent one
                let bounds_changed = !attributes.initial_configure_sent || (pending.bounds != current.bounds);

                // test if we should send a capabilities event, either because the
                // capabilities changed or we never sent one
                let capabilities_changed =
                    !attributes.initial_configure_sent || (pending.capabilities != current.capabilities);

                let configure = ToplevelConfigure {
                    serial: SERIAL_COUNTER.next_serial(),
                    state: pending,
                };

                attributes.pending_configures.push(configure.clone());
                attributes.initial_configure_sent = true;
                if decoration.is_some() {
                    attributes.initial_decoration_configure_sent = true;
                }

                (
                    configure,
                    decoration_mode_changed,
                    bounds_changed,
                    capabilities_changed,
                )
            });

        if decoration_mode_changed {
            if let Some(decoration) = &decoration {
                self::decoration::send_decoration_configure(
                    decoration,
                    configure
                        .state
                        .decoration_mode
                        .unwrap_or(zxdg_toplevel_decoration_v1::Mode::ClientSide),
                );
            }
        }

        let serial = configure.serial;
        self::handlers::send_toplevel_configure(
            &self.shell_surface,
            configure,
            bounds_changed,
            capabilities_changed,
        );

        serial
    }

    /// Did the surface sent the initial
    /// configure event to the client.
    ///
    /// Calls [`compositor::with_states`] internally.
    pub fn is_initial_configure_sent(&self) -> bool {
        compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        })
    }

    /// A newly-unmapped toplevel surface has to perform the initial commit-configure sequence as if it was
    /// a new toplevel.
    ///
    /// This method is used to mark a surface for reinitialization.
    ///
    /// NOTE: If you are using smithay's rendering abstractions you don't have to call this manually
    ///
    /// Calls [`compositor::with_states`] internally.
    pub fn reset_initial_configure_sent(&self) {
        compositor::with_states(&self.wl_surface, |states| {
            let mut data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            data.initial_configure_sent = false;
            data.initial_decoration_configure_sent = false;
        });
    }

    /// Handles the role specific commit logic
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn commit_hook<D: 'static>(
        _state: &mut D,
        _dh: &DisplayHandle,
        surface: &wl_surface::WlSurface,
    ) {
        let has_buffer = crate::backend::renderer::utils::with_renderer_surface_state(surface, |state| {
            state.buffer().is_some()
        });

        compositor::with_states(surface, |states| {
            let mut guard = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();

            // This can be None if rendering utils are not used by the user
            if let Some(has_buffer) = has_buffer {
                // The surface was mapped in the past, and now got unmapped
                if guard.has_buffer && !has_buffer {
                    // After xdg surface unmaps it has to perform the initial commit-configure sequence again
                    guard.initial_configure_sent = false;
                    guard.initial_decoration_configure_sent = false;
                }

                guard.has_buffer = has_buffer;
            }

            if let Some(state) = guard.last_acked.clone() {
                guard.current = state;
                guard.current_serial = guard.configure_serial;
            }
        });
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
        let configured = compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .configured
        });
        if !configured {
            let data = self
                .shell_surface
                .data::<self::handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.xdg_surface.post_error(
                xdg_surface::Error::NotConstructed,
                "Surface has not been configured yet.",
            );
        }
        configured
    }

    /// Send a "close" event to the client
    pub fn send_close(&self) {
        self.shell_surface.close()
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    #[inline]
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        &self.wl_surface
    }

    /// Access the underlying `xdg_toplevel` of this toplevel surface
    pub fn xdg_toplevel(&self) -> &xdg_toplevel::XdgToplevel {
        &self.shell_surface
    }

    /// Allows the pending state of this toplevel to
    /// be manipulated.
    ///
    /// This should be used to inform the client about size and state changes,
    /// for example after a resize request from the client.
    ///
    /// The state will be sent to the client when calling [`send_configure`](#method.send_configure).
    pub fn with_pending_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut ToplevelState) -> T,
    {
        compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.server_pending.is_none() {
                attributes.server_pending = Some(attributes.current_server_state().clone());
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
    }

    /// Tests this [`ToplevelSurface`] for pending changes
    ///
    /// Returns `true` if [`with_pending_state`](ToplevelSurface::with_pending_state) was used to manipulate the state
    /// and resulted in a different state or if the initial configure is still pending.
    pub fn has_pending_changes(&self) -> bool {
        compositor::with_states(&self.wl_surface, |states| {
            let attributes = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();

            !attributes.initial_configure_sent || attributes.has_pending_changes()
        })
    }

    /// Gets a copy of the current state of this toplevel
    pub fn current_state(&self) -> ToplevelState {
        compositor::with_states(&self.wl_surface, |states| {
            let attributes = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();

            attributes.current.clone()
        })
    }

    /// Returns the parent of this toplevel surface.
    pub fn parent(&self) -> Option<wl_surface::WlSurface> {
        handlers::get_parent(&self.shell_surface)
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
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // self.alive() && other.alive() &&
        self.wl_surface == other.wl_surface
    }
}

impl PopupSurface {
    /// Is the toplevel surface referred by this handle still alive?
    #[inline]
    pub fn alive(&self) -> bool {
        self.wl_surface.alive() && self.shell_surface.alive()
    }

    /// Gets a reference of the parent WlSurface of
    /// this popup.
    pub fn get_parent_surface(&self) -> Option<wl_surface::WlSurface> {
        compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .parent
                .clone()
        })
    }

    /// Retrieve the shell client owning this popup surface
    pub fn client(&self) -> ShellClient {
        let shell = {
            let data = self
                .shell_surface
                .data::<self::handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.wm_base.clone()
        };

        ShellClient { kind: shell }
    }

    /// Get the version of the popup resource
    fn version(&self) -> u32 {
        self.shell_surface.version()
    }

    /// Internal configure function to re-use the configure
    /// logic for both [`XdgRequest::send_configure`] and [`XdgRequest::send_repositioned`]
    fn send_configure_internal(&self, reposition_token: Option<u32>) -> Serial {
        let configure = compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();

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

            configure
        });
        let serial = configure.serial;
        self::handlers::send_popup_configure(&self.shell_surface, configure);
        serial
    }

    /// Send a pending configure event to this popup surface to suggest it a new configuration
    ///
    /// If changes have occurred a configure event will be send to the clients and the serial will be returned
    /// (for tracking the configure in [`XdgShellHandler::ack_configure`] if desired).
    /// If no changes occurred no event will be send and `Ok(None)` will be returned.
    ///
    /// See [`send_configure`](PopupSurface::send_configure) and [`has_pending_changes`](PopupSurface::has_pending_changes)
    /// for more information.
    pub fn send_pending_configure(&self) -> Result<Option<Serial>, PopupConfigureError> {
        if self.has_pending_changes() {
            self.send_configure().map(Some)
        } else {
            Ok(None)
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
    /// is not reactive.
    ///
    /// Note: This will always send a configure event, if you intend to only send a configure event on changes take a look at
    /// [`send_pending_configure`](PopupSurface::send_pending_configure)
    pub fn send_configure(&self) -> Result<Serial, PopupConfigureError> {
        // Check if we are allowed to send a configure
        compositor::with_states(&self.wl_surface, |states| {
            let attributes = states
                .data_map
                .get::<XdgPopupSurfaceData>()
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
        })?;

        let serial = self.send_configure_internal(None);

        Ok(serial)
    }

    /// Did the surface sent the initial
    /// configure event to the client.
    ///
    /// Calls [`compositor::with_states`] internally.
    pub fn is_initial_configure_sent(&self) -> bool {
        compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        })
    }

    /// A newly-unmapped popup surface has to perform the initial commit-configure sequence as if it was
    /// a new popup.
    ///
    /// This method is used to mark a surface for reinitialization.
    ///
    /// NOTE: If you are using smithay's rendering abstractions you don't have to call this manually
    ///
    /// Calls [`compositor::with_states`] internally.
    pub fn reset_initial_configure_sent(&self) {
        compositor::with_states(&self.wl_surface, |states| {
            let mut data = states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            data.initial_configure_sent = false;
        });
    }

    /// Send a configure event, including the `repositioned` event to the client
    /// in response to a `reposition` request.
    ///
    /// For further information see [`send_configure`](#method.send_configure)
    pub fn send_repositioned(&self, token: u32) -> Serial {
        self.send_configure_internal(Some(token))
    }

    /// Handles the role specific commit error checking
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn pre_commit_hook<D: 'static>(
        _state: &mut D,
        _dh: &DisplayHandle,
        surface: &wl_surface::WlSurface,
    ) {
        let send_error_to = compositor::with_states(surface, |states| {
            let attributes = states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.parent.is_none() {
                attributes.popup_handle.clone()
            } else {
                None
            }
        });
        if let Some(handle) = send_error_to {
            let data = handle.data::<self::handlers::XdgShellSurfaceUserData>().unwrap();
            data.xdg_surface.post_error(
                xdg_surface::Error::NotConstructed,
                "Surface has not been configured yet.",
            );
        }
    }

    /// Handles the role specific commit state application
    ///
    /// This should be called when the underlying WlSurface
    /// applies a wl_surface.commit state.
    pub(crate) fn post_commit_hook<D: 'static>(
        _state: &mut D,
        _dh: &DisplayHandle,
        surface: &wl_surface::WlSurface,
    ) {
        let has_buffer = crate::backend::renderer::utils::with_renderer_surface_state(surface, |state| {
            state.buffer().is_some()
        });

        compositor::with_states(surface, |states| {
            let mut attributes = states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            attributes.committed = true;

            // This can be None if rendering utils are not used by the user
            if let Some(has_buffer) = has_buffer {
                // The surface was mapped in the past, and now got unmapped
                if attributes.has_buffer && !has_buffer {
                    // After xdg surface unmaps it has to perform the initial commit-configure sequence again
                    attributes.initial_configure_sent = false;
                }

                attributes.has_buffer = has_buffer;
            }

            if attributes.initial_configure_sent {
                if let Some(state) = attributes.last_acked {
                    attributes.current = state;
                    attributes.current_serial = attributes.configure_serial;
                }
            }
        });
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
        let configured = compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .configured
        });
        if !configured {
            let data = self
                .shell_surface
                .data::<self::handlers::XdgShellSurfaceUserData>()
                .unwrap();
            data.xdg_surface.post_error(
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
    pub fn send_popup_done(&self) {
        self.shell_surface.popup_done();
    }

    /// Access the underlying `wl_surface` of this popup surface
    #[inline]
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        &self.wl_surface
    }

    /// Access the underlying `xdg_popup` of this popup surface
    pub fn xdg_popup(&self) -> &xdg_popup::XdgPopup {
        &self.shell_surface
    }

    /// Allows the pending state of this popup to
    /// be manipulated.
    ///
    /// This should be used to inform the client about size and position changes,
    /// for example after a move of the parent toplevel.
    ///
    /// The state will be sent to the client when calling [`send_configure`](#method.send_configure).
    pub fn with_pending_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut PopupState) -> T,
    {
        compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.server_pending.is_none() {
                attributes.server_pending = Some(*attributes.current_server_state());
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
    }

    /// Tests this [`PopupSurface`] for pending changes
    ///
    /// Returns `true` if [`with_pending_state`](PopupSurface::with_pending_state) was used to manipulate the state
    /// and resulted in a different state or if the initial configure is still pending.
    pub fn has_pending_changes(&self) -> bool {
        compositor::with_states(&self.wl_surface, |states| {
            let attributes = states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();

            !attributes.initial_configure_sent || attributes.has_pending_changes()
        })
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
    #[inline]
    fn from(configure: ToplevelConfigure) -> Self {
        Configure::Toplevel(configure)
    }
}

impl From<PopupConfigure> for Configure {
    #[inline]
    fn from(configure: PopupConfigure) -> Self {
        Configure::Popup(configure)
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_xdg_shell {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase: ()
        ] => $crate::wayland::shell::xdg::XdgShellState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase: $crate::wayland::shell::xdg::XdgWmBaseUserData
        ] => $crate::wayland::shell::xdg::XdgShellState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::XdgPositioner: $crate::wayland::shell::xdg::XdgPositionerUserData
        ] => $crate::wayland::shell::xdg::XdgShellState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::shell::server::xdg_popup::XdgPopup: $crate::wayland::shell::xdg::XdgShellSurfaceUserData
        ] => $crate::wayland::shell::xdg::XdgShellState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::shell::server::xdg_surface::XdgSurface: $crate::wayland::shell::xdg::XdgSurfaceUserData
        ] => $crate::wayland::shell::xdg::XdgShellState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel: $crate::wayland::shell::xdg::XdgShellSurfaceUserData
        ] => $crate::wayland::shell::xdg::XdgShellState);
    };
}
