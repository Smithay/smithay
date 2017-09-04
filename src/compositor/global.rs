use super::{CompositorHandler, Handler as UserHandler, Role, RoleType, SubsurfaceRole};

use wayland_server::{Client, EventLoopHandle, GlobalHandler};
use wayland_server::protocol::{wl_compositor, wl_subcompositor};

impl<U, R, H> GlobalHandler<wl_compositor::WlCompositor> for CompositorHandler<U, R, H>
where
    U: Default
        + Send
        + 'static,
    R: Default
        + Send
        + 'static,
    H: UserHandler<U, R>
        + Send
        + 'static,
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_compositor::WlCompositor) {
        debug!(self.log, "New compositor global binded.");
        evlh.register::<_, CompositorHandler<U, R, H>>(&global, self.my_id);
    }
}

impl<U, R, H> GlobalHandler<wl_subcompositor::WlSubcompositor> for CompositorHandler<U, R, H>
where
    U: Send + 'static,
    R: RoleType + Role<SubsurfaceRole> + Send + 'static,
    H: Send + 'static,
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_subcompositor::WlSubcompositor) {
        debug!(self.log, "New subcompositor global binded.");
        evlh.register::<_, CompositorHandler<U, R, H>>(&global, self.my_id);
    }
}
