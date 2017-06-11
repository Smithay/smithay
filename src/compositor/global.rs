use super::{CompositorHandler, Handler as UserHandler};

use wayland_server::{Client, EventLoopHandle, GlobalHandler};
use wayland_server::protocol::{wl_compositor, wl_subcompositor};

impl<U: Default, H: UserHandler> GlobalHandler<wl_compositor::WlCompositor> for CompositorHandler<U, H>
    where U: Send + 'static,
          H: Send + 'static
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_compositor::WlCompositor) {
        debug!(self.log, "New compositor global binded.");
        evlh.register::<_, CompositorHandler<U, H>>(&global, self.my_id);
    }
}

impl<U, H> GlobalHandler<wl_subcompositor::WlSubcompositor> for CompositorHandler<U, H>
    where U: Send + 'static,
          H: Send + 'static
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_subcompositor::WlSubcompositor) {
        debug!(self.log, "New subcompositor global binded.");
        evlh.register::<_, CompositorHandler<U, H>>(&global, self.my_id);
    }
}
