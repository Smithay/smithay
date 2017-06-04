use super::CompositorHandler;

use wayland_server::{Client, EventLoopHandle, GlobalHandler, Init};
use wayland_server::protocol::{wl_compositor, wl_subcompositor};

impl<U: Default> GlobalHandler<wl_compositor::WlCompositor> for CompositorHandler<U>
    where U: Send + Sync + 'static
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_compositor::WlCompositor) {
        evlh.register::<_, CompositorHandler<U>>(&global, self.my_id);
    }
}

impl<U> GlobalHandler<wl_subcompositor::WlSubcompositor> for CompositorHandler<U>
    where U: Send + Sync + 'static
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_subcompositor::WlSubcompositor) {
        evlh.register::<_, CompositorHandler<U>>(&global, self.my_id);
    }
}
