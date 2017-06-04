use super::CompositorToken;
use super::handlers::CompositorHandler;

use wayland_server::{Client, EventLoopHandle, GlobalHandler, Init};
use wayland_server::protocol::{wl_compositor, wl_subcompositor};

pub struct CompositorGlobal<U> {
    handler_id: Option<usize>,
    log: ::slog::Logger,
    _data: ::std::marker::PhantomData<*mut U>,
}

impl<U> CompositorGlobal<U> {
    pub fn new<L>(logger: L) -> CompositorGlobal<U>
        where L: Into<Option<::slog::Logger>>
    {
        let log = ::slog_or_stdlog(logger);
        CompositorGlobal {
            handler_id: None,
            log: log.new(o!("smithay_module" => "wompositor_handler")),
            _data: ::std::marker::PhantomData,
        }
    }

    pub fn get_token(&self) -> CompositorToken<U> {
        super::make_token(self.handler_id
                              .expect("CompositorGlobal was not initialized."))
    }
}

impl<U> Init for CompositorGlobal<U>
    where U: Send + Sync + 'static
{
    fn init(&mut self, evlh: &mut EventLoopHandle, _index: usize) {
        let id = evlh.add_handler_with_init(CompositorHandler::<U>::new(self.log.clone()));
        self.handler_id = Some(id);
    }
}

impl<U: Default> GlobalHandler<wl_compositor::WlCompositor> for CompositorGlobal<U>
    where U: Send + Sync + 'static
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_compositor::WlCompositor) {
        let hid = self.handler_id
            .expect("CompositorGlobal was not initialized.");
        evlh.register::<_, CompositorHandler<U>>(&global, hid);
    }
}

impl<U> GlobalHandler<wl_subcompositor::WlSubcompositor> for CompositorGlobal<U>
    where U: Send + Sync + 'static
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_subcompositor::WlSubcompositor) {
        let hid = self.handler_id
            .expect("CompositorGlobal was not initialized.");
        evlh.register::<_, CompositorHandler<U>>(&global, hid);
    }
}
