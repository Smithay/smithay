use smithay_client_toolkit::reexports::{
    calloop,
    calloop_wayland_source::WaylandSource,
    client::{self as wayland_client, globals::GlobalList},
};

use calloop::EventLoop;
use wayland_client::{
    globals::registry_queue_init, globals::GlobalListContents, protocol::wl_registry::WlRegistry, Connection,
    Dispatch, QueueHandle,
};

pub fn init_logging() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt()
            .compact()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().compact().init();
    }
}

pub fn init_connection<APP>() -> (EventLoop<'static, APP>, GlobalList, QueueHandle<APP>)
where
    APP: Dispatch<WlRegistry, GlobalListContents> + 'static,
{
    let conn = Connection::connect_to_env().unwrap();

    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();
    let event_loop: EventLoop<APP> = EventLoop::try_new().unwrap();
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle)
        .unwrap();

    (event_loop, globals, qh)
}
