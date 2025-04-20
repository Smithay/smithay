//! Utilities for handling the `wp-commit-timing` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation create the [`CommitTimingManagerState`] and store it inside your `State` struct.
//!
//! ```
//! use smithay::delegate_commit_timing;
//! use smithay::wayland::compositor;
//! use smithay::wayland::commit_timing::CommitTimingManagerState;
//!
//! # struct State { commit_timing_manager_state: CommitTimingManagerState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the compositor state
//! let commit_timing_manager_state = CommitTimingManagerState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // insert the CommitTimingManagerState into your state
//! // ..
//!
//! delegate_commit_timing!(State);
//!
//! // You're now ready to go!
//! ```
//!
//! ### Use the commit timer state
//!
//! Whenever the client commits a surface content update containing a commit timer timestamp set
//! through [`wp_commit_timer_v1::Request::SetTimestamp`] the implementation will place a [`Blocker`](crate::wayland::compositor::Blocker)
//! on the surface and register it in the [`CommitTimerBarrierState`]. It is your responsibility to query for pending commit timers
//! and signal them to allow client to make forward progress.
//!
//! You can query the pending commit timers and signal them as shown in the following example:
//!
//! ```no_run
//! # use wayland_server::{backend::ObjectId, protocol::wl_surface, Resource};
//! use smithay::wayland::compositor;
//! use smithay::wayland::commit_timing::CommitTimerBarrierStateUserData;
//! # use smithay::wayland::commit_timing::Timestamp;
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let dh = display.handle();
//! # let surface = wl_surface::WlSurface::from_id(&dh, ObjectId::null()).unwrap();
//! # let frame_target: Timestamp = todo!();
//! compositor::with_states(&surface, |states| {
//!     if let Some(mut commit_timer_state) = states
//!         .data_map
//!         .get::<CommitTimerBarrierStateUserData>()
//!         .map(|commit_timer| commit_timer.lock().unwrap())
//!         {
//!             if commit_timer_state.signal_until(frame_target) {
//!                 // ..signal blocker cleared
//!             }
//!         }
//! });
//! ```
//!
//! ### Unmanaged mode
//!
//! If for some reason the integrated solution for commit timers does not suit your needs
//! you can create an unmanaged version with [`CommitTimingManagerState::unmanaged`].
//! In this case it is your responsibility to listen for surface commits and place blockers
//! accordingly to the spec.
//!
//! The commit timer timestamp can be retrieved like shown in the following example:
//!
//! ```no_run
//! # let surface: wayland_server::protocol::wl_surface::WlSurface = todo!();
//! use smithay::wayland::compositor;
//! use smithay::wayland::commit_timing::CommitTimerStateUserData;
//!
//! let timestamp = compositor::with_states(&surface, |states| {
//!     states
//!         .data_map
//!         .get::<CommitTimerStateUserData>()
//!         .and_then(|state| state.borrow_mut().timestamp.take())
//! });
//! ```
use std::{cell::RefCell, collections::BinaryHeap, sync::Mutex};

use rustix::fs::Timespec;
use wayland_protocols::wp::commit_timing::v1::server::{
    wp_commit_timer_v1::{self, WpCommitTimerV1},
    wp_commit_timing_manager_v1::{self, WpCommitTimingManagerV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, DataInit, Dispatch, DisplayHandle, GlobalDispatch,
    New, Resource, Weak,
};

use crate::{
    utils::Time,
    wayland::compositor::{add_blocker, add_pre_commit_hook},
};

use super::compositor::{with_states, Barrier};

/// State for the [`WpCommitTimingManagerV1`] global
#[derive(Debug)]
pub struct CommitTimingManagerState {
    global: GlobalId,
    is_managed: bool,
}

impl CommitTimingManagerState {
    /// Create a new [`WpCommitTimingManagerV1`] global
    //
    /// The id provided by [`CommitTimingManagerState::global`] may be used to
    /// remove or disable this global in the future.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WpCommitTimingManagerV1, bool>,
        D: Dispatch<WpCommitTimingManagerV1, bool>,
        D: 'static,
    {
        Self::new_internal::<D>(display, true)
    }

    /// Create a new unmanaged [`WpCommitTimingManagerV1`] global
    //
    /// The id provided by [`CommitTimingManagerState::global`] may be used to
    /// remove or disable this global in the future.
    pub fn unmanaged<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WpCommitTimingManagerV1, bool>,
        D: Dispatch<WpCommitTimingManagerV1, bool>,
        D: 'static,
    {
        Self::new_internal::<D>(display, false)
    }

    fn new_internal<D>(display: &DisplayHandle, is_managed: bool) -> Self
    where
        D: GlobalDispatch<WpCommitTimingManagerV1, bool>,
        D: Dispatch<WpCommitTimingManagerV1, bool>,
        D: 'static,
    {
        let global = display.create_global::<D, WpCommitTimingManagerV1, _>(1, is_managed);

        Self { global, is_managed }
    }

    /// Returns the id of the [`WpCommitTimingManagerV1`] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    /// Returns if this [`CommitTimingManagerState`] operates in managed mode.
    pub fn is_managed(&self) -> bool {
        self.is_managed
    }
}

impl<D> GlobalDispatch<WpCommitTimingManagerV1, bool, D> for CommitTimingManagerState
where
    D: GlobalDispatch<WpCommitTimingManagerV1, bool>,
    D: Dispatch<WpCommitTimingManagerV1, bool>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WpCommitTimingManagerV1>,
        global_data: &bool,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, *global_data);
    }
}

impl<D> Dispatch<WpCommitTimingManagerV1, bool, D> for CommitTimingManagerState
where
    D: Dispatch<WpCommitTimingManagerV1, bool>,
    D: Dispatch<WpCommitTimerV1, Weak<WlSurface>>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WpCommitTimingManagerV1,
        request: wp_commit_timing_manager_v1::Request,
        data: &bool,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let is_managed = *data;

        match request {
            wp_commit_timing_manager_v1::Request::GetTimer { id, surface } => {
                let (is_initial, has_active_commit_timer) = with_states(&surface, |states| {
                    let marker = states.data_map.get::<RefCell<CommitTimerMarker>>();
                    (
                        marker.is_none(),
                        marker.map(|m| m.borrow().0.is_some()).unwrap_or(false),
                    )
                });

                // The protocol mandates that only a single commit timer object is associated with a surface at all times
                if has_active_commit_timer {
                    surface.post_error(
                        wp_commit_timing_manager_v1::Error::CommitTimerExists,
                        "the surface has already a commit timer object associated",
                    );
                    return;
                }

                // Make sure we do not install the hook more then once in case the surface is being reused
                if is_managed && is_initial {
                    add_pre_commit_hook::<D, _>(&surface, |_, _, surface| {
                        let timestamp = with_states(surface, |states| {
                            states
                                .data_map
                                .get::<CommitTimerStateUserData>()
                                .and_then(|state| state.borrow_mut().timestamp.take())
                        });

                        if let Some(timestamp) = timestamp {
                            let barrier = with_states(surface, |states| {
                                let barrier_state = states
                                    .data_map
                                    .get_or_insert(CommitTimerBarrierStateUserData::default);
                                barrier_state.lock().unwrap().register(timestamp)
                            });

                            add_blocker(surface, barrier);
                        }
                    });
                }

                let commit_timer: WpCommitTimerV1 = data_init.init(id, surface.downgrade());

                with_states(&surface, |states| {
                    states
                        .data_map
                        .get_or_insert(|| RefCell::new(CommitTimerMarker(None)))
                        .borrow_mut()
                        .0 = Some(commit_timer);
                });
            }
            wp_commit_timing_manager_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }
}

// Internal marker to track if a commit timer object is currently associated with a surface.
// Used to realize the `AlreadyExists` protocol check.
struct CommitTimerMarker(Option<WpCommitTimerV1>);

impl<D> Dispatch<WpCommitTimerV1, Weak<WlSurface>, D> for CommitTimingManagerState
where
    D: Dispatch<WpCommitTimerV1, Weak<WlSurface>>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        resource: &WpCommitTimerV1,
        request: wp_commit_timer_v1::Request,
        data: &Weak<WlSurface>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_commit_timer_v1::Request::SetTimestamp {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let Ok(surface) = data.upgrade() else {
                    resource.post_error(
                        wp_commit_timer_v1::Error::SurfaceDestroyed as u32,
                        "the surface associated with this commit timer object has been destroyed".to_string(),
                    );
                    return;
                };

                let tv_sec = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;

                let timestamp = Timestamp(Timespec {
                    tv_sec: tv_sec as rustix::time::Secs,
                    tv_nsec: tv_nsec as rustix::time::Nsecs,
                });

                let already_has_timestamp = with_states(&surface, move |states| {
                    let mut commit_timer_state = states
                        .data_map
                        .get_or_insert(CommitTimerStateUserData::default)
                        .borrow_mut();

                    if commit_timer_state.timestamp.is_some() {
                        return true;
                    }

                    commit_timer_state.timestamp = Some(timestamp);
                    false
                });

                if already_has_timestamp {
                    resource.post_error(
                        wp_commit_timer_v1::Error::TimestampExists as u32,
                        "the surface already has a timestamp associated for this commit".to_string(),
                    );
                }
            }
            wp_commit_timer_v1::Request::Destroy => {
                if let Ok(surface) = data.upgrade() {
                    with_states(&surface, |states| {
                        states
                            .data_map
                            .get::<RefCell<CommitTimerMarker>>()
                            .unwrap()
                            .borrow_mut()
                            .0 = None;
                    });
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Timestamp set through [`wp_commit_timer_v1::Request::SetTimestamp`] for a surface
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct Timestamp(Timespec);

impl Ord for Timestamp {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.0.tv_sec.cmp(&other.0.tv_sec) {
            std::cmp::Ordering::Equal => self.0.tv_nsec.cmp(&other.0.tv_nsec),
            other => other,
        }
    }
}

impl PartialOrd for Timestamp {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Per surface [`WpCommitTimerV1`] state stored in the surface user data
pub type CommitTimerStateUserData = RefCell<CommitTimerState>;

/// Per surface state for [`WpCommitTimerV1`]
#[derive(Debug, Default)]
pub struct CommitTimerState {
    /// The timestamp set through [`wp_commit_timer_v1::Request::SetTimestamp`] for this surface
    pub timestamp: Option<Timestamp>,
}

/// Per surface barrier state stored in the surface user data
pub type CommitTimerBarrierStateUserData = Mutex<CommitTimerBarrierState>;

/// Per surface barrier state using the managed mode
#[derive(Debug, Default)]
pub struct CommitTimerBarrierState {
    barriers: BinaryHeap<CommitTimerBarrier>,
}

impl<Kind> From<Time<Kind>> for Timestamp {
    fn from(value: Time<Kind>) -> Self {
        Self(value.into())
    }
}

impl<Kind> From<Timestamp> for Time<Kind> {
    fn from(value: Timestamp) -> Self {
        Time::from(value.0)
    }
}

impl CommitTimerBarrierState {
    /// Signal all tracked barriers matching the specified deadline
    ///
    /// A barrier is considered to lie within the deadline if the
    /// timestamp of the barrier is past the specified deadline
    ///
    /// Returns `true` if a barrier has been signaled, false otherwise
    pub fn signal_until(&mut self, deadline: impl Into<Timestamp>) -> bool {
        let deadline = deadline.into();

        let num_barriers = self.barriers.len();
        loop {
            let Some(barrier) = self.barriers.peek() else {
                break;
            };

            if barrier.timestamp > deadline {
                break;
            }

            barrier.barrier.signal();
            let _ = self.barriers.pop();
        }

        num_barriers != self.barriers.len()
    }

    /// Register a new barrier with the provided timestamp
    pub fn register(&mut self, timestamp: Timestamp) -> Barrier {
        let barrier = Barrier::new(false);
        self.barriers.push(CommitTimerBarrier {
            timestamp,
            barrier: barrier.clone(),
        });
        barrier
    }

    /// Retrieve the next deadline when available
    pub fn next_deadline(&self) -> Option<Timestamp> {
        self.barriers.peek().map(|barrier| barrier.timestamp)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitTimerBarrier {
    timestamp: Timestamp,
    barrier: Barrier,
}

impl std::cmp::Ord for CommitTimerBarrier {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // earlier values have priority
        self.timestamp.cmp(&other.timestamp).reverse()
    }
}

impl std::cmp::PartialOrd for CommitTimerBarrier {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Macro used to delegate [`WpCommitTimingManagerV1`] events
#[macro_export]
macro_rules! delegate_commit_timing {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::commit_timing::v1::server::wp_commit_timing_manager_v1::WpCommitTimingManagerV1: bool
        ] => $crate::wayland::commit_timing::CommitTimingManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::commit_timing::v1::server::wp_commit_timing_manager_v1::WpCommitTimingManagerV1: bool
        ] => $crate::wayland::commit_timing::CommitTimingManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::commit_timing::v1::server::wp_commit_timer_v1::WpCommitTimerV1: $crate::reexports::wayland_server::Weak<$crate::reexports::wayland_server::protocol::wl_surface::WlSurface>
        ] => $crate::wayland::commit_timing::CommitTimingManagerState);
    };
}
