//! XDG Window decoration manager
//!
//! This interface allows a compositor to announce support for server-side decorations.
//!
//! A client can use this protocol to request being decorated by a supporting compositor.
//!
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellHandler};
//! # use smithay::utils::Serial;
//! # use smithay::wayland::shell::xdg::{XdgShellState, PopupSurface, PositionerState};
//! # use smithay::reexports::wayland_server::protocol::{wl_seat, wl_surface};
//! use smithay::wayland::shell::xdg::decoration::{XdgDecorationState, XdgDecorationHandler};
//! use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
//!
//! # struct State { decoration_state: XdgDecorationState, seat_state: SeatState<Self> }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! // Create a decoration state
//! let decoration_state = XdgDecorationState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // store that state inside your compositor state
//! // ...
//!
//! // implement the necessary traits
//! impl XdgShellHandler for State {
//!     # fn xdg_shell_state(&mut self) -> &mut XdgShellState { unimplemented!() }
//!     # fn new_toplevel(&mut self, surface: ToplevelSurface) { unimplemented!() }
//!     # fn new_popup(
//!     #     &mut self,
//!     #     surface: PopupSurface,
//!     #     positioner: PositionerState,
//!     # ) { unimplemented!() }
//!     # fn grab(
//!     #     &mut self,
//!     #     surface: PopupSurface,
//!     #     seat: wl_seat::WlSeat,
//!     #     serial: Serial,
//!     # ) { unimplemented!() }
//!     # fn reposition_request(
//!     #     &mut self,
//!     #     surface: PopupSurface,
//!     #     positioner: PositionerState,
//!     #     token: u32,
//!     # ) { unimplemented!() }
//!     // ...
//! }
//! impl XdgDecorationHandler for State {
//!     fn new_decoration(&mut self, toplevel: ToplevelSurface) {
//!         toplevel.with_pending_state(|state| {
//!             // Advertise server side decoration
//!             state.decoration_mode = Some(Mode::ServerSide);
//!         });
//!         toplevel.send_configure();
//!     }
//!     fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) { /* ... */ }
//!     fn unset_mode(&mut self, toplevel: ToplevelSurface) { /* ... */ }
//!     fn decoration_destroyed(&mut self, toplevel: ToplevelSurface) { /* ... */ }
//! }
//!
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! # use smithay::wayland::pointer_constraints::PointerConstraintsHandler;
//!
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a wayland_server::Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &wayland_server::protocol::wl_surface::WlSurface) {}
//! # }
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
//! # impl PointerConstraintsHandler for State {}
//!
//! smithay::delegate_dispatch2!(State);
//!
//! // You are ready to go!
// TODO: Describe how to change decoration mode.

use wayland_protocols::xdg::decoration::zv1::server::{
    zxdg_decoration_manager_v1,
    zxdg_toplevel_decoration_v1::{self, Mode},
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum, backend::GlobalId,
    protocol::wl_surface::WlSurface,
};

use super::{ToplevelSurface, XdgShellHandler};
use crate::wayland::{
    Dispatch2, GlobalData, GlobalDispatch2,
    compositor::{self, Cacheable, MultiCache},
    shell::xdg::{XdgShellSurfaceUserData, XdgToplevelSurfaceRoleAttributes},
};

/// Delegate type for handling xdg decoration events.
#[derive(Debug)]
pub struct XdgDecorationState {
    global: GlobalId,
}

/// Data associated with a XdgDecorationManager global.
#[allow(missing_debug_implementations)]
pub struct XdgDecorationManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl XdgDecorationState {
    /// Creates a new delegate type for handling xdg decoration events.
    ///
    /// A global id is also returned to allow destroying the global in the future.
    pub fn new<D>(display: &DisplayHandle) -> XdgDecorationState
    where
        D: GlobalDispatch<
                zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
                XdgDecorationManagerGlobalData,
            > + Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, GlobalData>
            + 'static,
    {
        Self::new_with_filter::<D, _>(display, |_| true)
    }

    /// Creates a new delegate type for handling xdg decoration events with a filter.
    ///
    /// Filters can be used to limit visibility of a global to certain clients.
    ///
    /// A global id is also returned to allow destroying the global in the future.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, filter: F) -> XdgDecorationState
    where
        D: GlobalDispatch<
                zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
                XdgDecorationManagerGlobalData,
            > + Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, GlobalData>
            + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = XdgDecorationManagerGlobalData {
            filter: Box::new(filter),
        };
        let global =
            display.create_global::<D, zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _>(2, data);

        XdgDecorationState { global }
    }

    /// Returns the xdg-decoration global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Handler trait for xdg decoration events.
pub trait XdgDecorationHandler {
    /// Notification the client supports server side decoration on the toplevel.
    fn new_decoration(&mut self, toplevel: ToplevelSurface);

    /// Notification the client prefers the provided decoration decoration mode on the toplevel.
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode);

    /// Notification the client does not prefer a particular decoration mode on the toplevel.
    fn unset_mode(&mut self, toplevel: ToplevelSurface);

    /// Notification the decoration object of the toplevel was destroyed, and the toplevel must switched back to CSD.
    ///
    /// This is called once the commit applying the destruction takes effect, right before [`CompositorHandler::commit`](crate::wayland::compositor::CompositorHandler::commit)
    /// By then the decoration mode of the toplevel has been reset to `None`.
    fn decoration_destroyed(&mut self, toplevel: ToplevelSurface);
}

/// Marks a surface that already has [`post_commit_hook`] installed
struct PostCommitHookInstalled;

fn post_commit_hook<D>(state: &mut D, _dh: &DisplayHandle, surface: &WlSurface)
where
    D: XdgShellHandler + XdgDecorationHandler + 'static,
{
    let is_destroy_committed = compositor::with_states(surface, |states| {
        DecorationDestroyStateMachine::is_destroy_committed(&states.cached_state)
    });

    if !is_destroy_committed {
        return;
    }

    let toplevel = state
        .xdg_shell_state()
        .toplevel_surfaces()
        .iter()
        .find(|toplevel| toplevel.wl_surface() == surface)
        .cloned();
    if let Some(toplevel) = toplevel {
        state.decoration_destroyed(toplevel);
    }
}

/// Called by [`super::ToplevelSurface::pre_commit_hook`] to make the ordering more obvious
pub(super) fn xdg_toplevel_pre_commit_hook(
    cached_state: &MultiCache,
    role: &mut XdgToplevelSurfaceRoleAttributes,
) {
    if !DecorationDestroyStateMachine::is_destroy_waiting_for_commit(cached_state) {
        return;
    }

    // Destroy is about to be committed so let's clean up the decoration state

    role.initial_decoration_configure_sent = false;

    if let Some(pending) = role.server_pending.as_mut() {
        pending.decoration_mode = None;
    }

    for configure in role.pending_configures.iter_mut() {
        configure.state.decoration_mode = None;
    }

    if let Some(configure) = role.last_acked.as_mut() {
        configure.state.decoration_mode = None;
    }
}

pub(super) fn send_decoration_configure(
    id: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    mode: Mode,
) {
    id.configure(mode)
}

impl<D> GlobalDispatch2<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, D>
    for XdgDecorationManagerGlobalData
where
    D: Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, GlobalData>
        + Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ToplevelSurface>
        + XdgShellHandler
        + XdgDecorationHandler
        + 'static,
{
    fn bind(
        &self,
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, D> for GlobalData
where
    D: Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ToplevelSurface>
        + XdgShellHandler
        + XdgDecorationHandler
        + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _: &Client,
        resource: &zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::zxdg_decoration_manager_v1::Request;

        match request {
            Request::GetToplevelDecoration { id, toplevel } => {
                let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();

                if !data.alive_tracker.alive() {
                    resource.post_error(
                        zxdg_toplevel_decoration_v1::Error::Orphaned,
                        "toplevel is already destroyed",
                    );
                    return;
                }

                let mut decoration_guard = data.decoration.lock().unwrap();

                if decoration_guard.is_some() {
                    resource.post_error(
                        zxdg_toplevel_decoration_v1::Error::AlreadyConstructed,
                        "toplevel decoration is already constructed",
                    );
                    return;
                }

                let toplevel = state.xdg_shell_state().get_toplevel(&toplevel).unwrap();
                let toplevel_decoration = data_init.init(id, toplevel.clone());

                *decoration_guard = Some(toplevel_decoration);
                drop(decoration_guard);

                let (recreated, install_hook) = compositor::with_states(toplevel.wl_surface(), |data| {
                    // Check for uncommitted destroy, if we got one it means we should inherit the
                    // state of the old decor object
                    let recreated =
                        DecorationDestroyStateMachine::is_destroy_waiting_for_commit(&data.cached_state);

                    DecorationDestroyStateMachine::set_idle(&data.cached_state);

                    let install_hook = data
                        .data_map
                        .insert_if_missing_threadsafe(|| PostCommitHookInstalled);
                    (recreated, install_hook)
                });

                if install_hook {
                    compositor::add_post_commit_hook(toplevel.wl_surface(), post_commit_hook::<D>);
                }

                if !recreated {
                    state.new_decoration(toplevel);
                }
            }

            Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}

// zxdg_toplevel_decoration_v1

impl<D> Dispatch2<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, D> for ToplevelSurface
where
    D: XdgDecorationHandler,
{
    fn request(
        &self,
        state: &mut D,
        _: &Client,
        resource: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        _dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        use self::zxdg_toplevel_decoration_v1::{Error, Request};

        match request {
            Request::SetMode { mode } => {
                if let WEnum::Value(mode) = mode {
                    state.request_mode(self.clone(), mode);
                } else {
                    resource.post_error(Error::InvalidMode, "invalid decoration mode");
                }
            }

            Request::UnsetMode => {
                state.unset_mode(self.clone());
            }

            Request::Destroy => {
                if let Some(data) = self.xdg_toplevel().data::<XdgShellSurfaceUserData>() {
                    data.decoration.lock().unwrap().take();
                }

                compositor::with_states(self.wl_surface(), |data| {
                    DecorationDestroyStateMachine::set_destroy_waiting_for_commit(&data.cached_state);
                });
            }

            _ => unreachable!(),
        }
    }
}

#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
enum DecorationDestroyStateMachine {
    #[default]
    Idle,
    DestroyWaitingForCommit,
    DestroyCommitted,
}

impl DecorationDestroyStateMachine {
    fn set_idle(cached_state: &MultiCache) {
        *cached_state.get::<DecorationDestroyStateMachine>().pending() = Self::Idle;
    }

    fn set_destroy_waiting_for_commit(cached_state: &MultiCache) {
        *cached_state.get::<DecorationDestroyStateMachine>().pending() = Self::DestroyWaitingForCommit;
    }

    fn is_destroy_waiting_for_commit(cached_state: &MultiCache) -> bool {
        *cached_state.get::<DecorationDestroyStateMachine>().pending() == Self::DestroyWaitingForCommit
    }

    fn is_destroy_committed(cached_state: &MultiCache) -> bool {
        *cached_state.get::<DecorationDestroyStateMachine>().current() == Self::DestroyCommitted
    }
}

impl Cacheable for DecorationDestroyStateMachine {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self = match self {
            Self::Idle => Self::Idle,
            Self::DestroyWaitingForCommit => Self::DestroyCommitted,
            Self::DestroyCommitted => Self::Idle,
        };
        *self
    }
    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}
