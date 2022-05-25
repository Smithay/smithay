use std::time::Duration;

use smithay::{
    backend::{
        renderer::gles2::Gles2Renderer,
        winit::{self, WinitError, WinitEvent, WinitEventLoop, WinitGraphicsBackend},
    },
    desktop::space::SurfaceTree,
    reexports::{
        calloop::{timer::Timer, EventLoop},
        wayland_server::protocol::wl_output,
    },
    utils::Rectangle,
    wayland::output::{Mode, Output, PhysicalProperties},
};

use slog::Logger;

use crate::CalloopData;

pub fn init_winit(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
    log: Logger,
) -> Result<(), Box<dyn std::error::Error>> {
    let display = &mut data.display;
    let state = &mut data.state;

    let (mut backend, mut winit) = winit::init(log.clone())?;

    let mode = Mode {
        size: backend.window_size().physical_size,
        refresh: 60_000,
    };

    let (output, _global) = Output::new(
        display,
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );
    output.change_current_state(
        Some(mode),
        Some(wl_output::Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.space.map_output(&output, 1.0, (0, 0));

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    let mut full_redraw = 0u8;

    let timer = Timer::<()>::new().unwrap();

    timer.handle().add_timeout(Duration::ZERO, ());
    event_loop.handle().insert_source(timer, move |_, timer, data| {
        winit_dispatch(&mut backend, &mut winit, data, &output, &mut full_redraw).unwrap();
        timer.add_timeout(Duration::from_millis(16), ());
    })?;

    Ok(())
}

pub fn winit_dispatch(
    backend: &mut WinitGraphicsBackend,
    winit: &mut WinitEventLoop,
    data: &mut CalloopData,
    output: &Output,
    full_redraw: &mut u8,
) -> Result<(), Box<dyn std::error::Error>> {
    let display = &mut data.display;
    let state = &mut data.state;

    let res = winit.dispatch_new_events(|event| match event {
        WinitEvent::Resized { size, .. } => {
            output.change_current_state(
                Some(Mode {
                    size,
                    refresh: 60_000,
                }),
                None,
                None,
                None,
            );
        }
        WinitEvent::Input(event) => state.process_input_event(display, event),
        _ => (),
    });

    if let Err(WinitError::WindowClosed) = res {
        // Stop the loop
        state.loop_signal.stop();

        return Ok(());
    } else {
        res?;
    }

    *full_redraw = full_redraw.saturating_sub(1);

    let size = backend.window_size().physical_size;
    let damage = Rectangle::from_loc_and_size((0, 0), size);

    backend.bind().ok().and_then(|_| {
        state
            .space
            .render_output::<Gles2Renderer, SurfaceTree>(
                &display.handle(),
                backend.renderer(),
                output,
                0,
                [0.1, 0.1, 0.1, 1.0],
                &[],
            )
            .unwrap()
    });

    backend.submit(Some(&[damage.to_logical(1)]), 1.0).unwrap();

    state
        .space
        .send_frames(state.start_time.elapsed().as_millis() as u32);

    state.space.refresh(&display.handle());
    display.flush_clients()?;

    Ok(())
}
