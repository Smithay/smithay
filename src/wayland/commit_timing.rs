use crate::{
    utils::{Clock, Monotonic, Time},
    wayland::compositor::{Blocker, BlockerState}
};
use std::time::Duration;
use wayland_protocols::wp::commit_timing::v1::server::{wp_commit_timer_v1, wp_commit_timing_manager_v1};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource, WEnum, Weak,
};

// TODO add in pre_commit_hook
struct CommitTimingBlocker {
    // TODO presentation protocol allows different clock ids
    time: Time<Monotonic>
}

impl Blocker for CommitTimingBlocker {
    fn state(&self) -> BlockerState {
        let now = Clock::<Monotonic>::new().now();
        if now >= self.time {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}

pub struct CommitTimingState {
}

impl CommitTimingState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<
            wp_commit_timing_manager_v1::WpCommitTimingManagerV1,
            CommitTimingManagerGlobalData,
        >,
        D: Dispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, ()>,
        D: Dispatch<wp_commit_timer_v1::WpCommitTimerV1, CommitTimerData>,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = CommitTimingManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, wp_commit_timing_manager_v1::WpCommitTimingManagerV1, _>(1, data);

        Self {}
    }
}

#[allow(missing_debug_implementations)]
#[doc(hidden)]
pub struct CommitTimingManagerGlobalData {
    /// Filter whether the clients can view global.
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl<D> GlobalDispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, CommitTimingManagerGlobalData, D>
    for CommitTimingState
where
    D: GlobalDispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, CommitTimingManagerGlobalData>,
    D: Dispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, ()>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<wp_commit_timing_manager_v1::WpCommitTimingManagerV1>,
        _global_data: &CommitTimingManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }

    fn can_view(client: Client, global_data: &CommitTimingManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

#[doc(hidden)]
pub struct CommitTimerData {
    surface: Weak<wl_surface::WlSurface>,
}

impl<D> Dispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, (), D> for CommitTimingState
where
    D: Dispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, ()>,
    D: Dispatch<wp_commit_timer_v1::WpCommitTimerV1, CommitTimerData>,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _proxy: &wp_commit_timing_manager_v1::WpCommitTimingManagerV1,
        request: wp_commit_timing_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_commit_timing_manager_v1::Request::GetTimer { id, surface } => {
                // TODO CommitTimerExists
                data_init.init(
                    id,
                    CommitTimerData {
                        surface: surface.downgrade(),
                    },
                );
            }
            wp_commit_timing_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<wp_commit_timer_v1::WpCommitTimerV1, CommitTimerData, D> for CommitTimingState
where
    D: Dispatch<wp_commit_timer_v1::WpCommitTimerV1, CommitTimerData>,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _proxy: &wp_commit_timer_v1::WpCommitTimerV1,
        request: wp_commit_timer_v1::Request,
        _data: &CommitTimerData,
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_commit_timer_v1::Request::SetTimestamp {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let secs = (u64::from(tv_sec_hi) << 32) + u64::from(tv_sec_lo);
                let time = Time::<Monotonic>::from(Duration::new(secs, tv_nsec));
            }
            wp_commit_timer_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

#[allow(missing_docs)]
#[macro_export]
macro_rules! delegate_commit_timing {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::commit_timing::v1::server::wp_commit_timing_manager_v1::WpCommitTimingManagerV1: $crate::wayland::commit_timing::CommitTimingManagerGlobalData
        ] => $crate::wayland::commit_timing::CommitTimingState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::commit_timing::v1::server::wp_commit_timing_manager_v1::WpCommitTimingManagerV1: ()
        ] => $crate::wayland::commit_timing::CommitTimingState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::commit_timing::v1::server::wp_commit_timer_v1::WpCommitTimerV1: $crate::wayland::commit_timing::CommitTimerData
        ] => $crate::wayland::commit_timing::CommitTimingState);
    };
}
//use wayland_protocols::wp::commit_timing::v1::server::{wp_commit_timer_v1, wp_commit_timing_manager_v1};
