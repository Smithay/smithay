//! idle-inhibit inhibitor.

use _idle_inhibit::zwp_idle_inhibitor_v1::{Request, ZwpIdleInhibitorV1};
use wayland_protocols::wp::idle_inhibit::zv1::server as _idle_inhibit;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, DisplayHandle};

use crate::wayland::Dispatch2;
use crate::wayland::idle_inhibit::IdleInhibitHandler;

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

impl<D> Dispatch2<ZwpIdleInhibitorV1, D> for IdleInhibitorState
where
    D: IdleInhibitHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _inhibitor: &ZwpIdleInhibitorV1,
        request: Request,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::Destroy => state.uninhibit(self.surface.clone()),
            _ => unreachable!(),
        }
    }
}
