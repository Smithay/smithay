//! idle-inhibit inhibitor.

use _idle_inhibit::zwp_idle_inhibitor_v1::{Request, ZwpIdleInhibitorV1};
use wayland_protocols::wp::idle_inhibit::zv1::server as _idle_inhibit;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle};

use crate::wayland::idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState};

/// State of zwp_idle_inhibitor_v1.
#[derive(Debug)]
pub struct IdleInhibitorState {
    surface: WlSurface,
}

impl IdleInhibitorState {
    /// Create `zwp_idle_inhibitor_v1` state.
    pub fn new(surface: WlSurface) -> Self {
        Self { surface }
    }
}

impl<D> Dispatch<ZwpIdleInhibitorV1, IdleInhibitorState, D> for IdleInhibitManagerState
where
    D: Dispatch<ZwpIdleInhibitorV1, IdleInhibitorState>,
    D: IdleInhibitHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _inhibitor: &ZwpIdleInhibitorV1,
        request: Request,
        data: &IdleInhibitorState,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::Destroy => state.uninhibit(data.surface.clone()),
            _ => unreachable!(),
        }
    }
}
