use smithay_client_toolkit::{
    reexports::{
        calloop,
        calloop_wayland_source::WaylandSource,
        client::{
            self as wayland_client,
            globals::GlobalList,
            protocol::{wl_callback::WlCallback, wl_shm, wl_surface::WlSurface},
        },
    },
    shell::{xdg::window::Window, WaylandSurface},
    shm::slot::{Buffer, SlotPool},
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

pub fn fill_with_gradient_bytes(canvas: &mut [u8], shift: u32, width: u32, height: u32) {
    canvas.chunks_exact_mut(4).enumerate().for_each(|(index, chunk)| {
        let x = ((index + shift as usize) % width as usize) as u32;
        let y = (index / width as usize) as u32;

        let a = 0xFF;
        let r = u32::min(((width - x) * 0xFF) / width, ((height - y) * 0xFF) / height);
        let g = u32::min((x * 0xFF) / width, ((height - y) * 0xFF) / height);
        let b = u32::min(((width - x) * 0xFF) / width, (y * 0xFF) / height);
        let color = (a << 24) + (r << 16) + (g << 8) + b;

        let array: &mut [u8; 4] = chunk.try_into().unwrap();
        *array = color.to_le_bytes();
    });
}

pub fn draw<D>(
    qh: &QueueHandle<D>,
    window: &Window,
    pool: &mut SlotPool,
    buffer: &mut Option<Buffer>,
    width: u32,
    height: u32,
    shift: &mut u32,
) where
    D: 'static,
    D: Dispatch<WlCallback, WlSurface>,
{
    let stride = width as i32 * 4;

    let buffer = buffer.get_or_insert_with(|| {
        pool.create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)
            .unwrap()
            .0
    });

    let canvas = match pool.canvas(buffer) {
        Some(canvas) => canvas,
        None => {
            // This should be rare, but if the compositor has not released the previous
            // buffer, we need double-buffering.
            let (second_buffer, canvas) = pool
                .create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)
                .unwrap();
            *buffer = second_buffer;
            canvas
        }
    };

    // Draw to the window:
    fill_with_gradient_bytes(canvas, *shift, width, height);
    *shift = (*shift + 1) % width;

    window
        .wl_surface()
        .damage_buffer(0, 0, width as i32, height as i32);

    window.wl_surface().frame(qh, window.wl_surface().clone());

    buffer.attach_to(window.wl_surface()).expect("buffer attach");
    window.commit();
}
