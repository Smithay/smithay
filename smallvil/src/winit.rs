use std::time::Duration;

use smithay::{
    backend::{
        renderer::{damage::DamageTrackedRenderer, gles2::Gles2Renderer},
        winit::{self, WinitError, WinitEvent, WinitEventLoop, WinitGraphicsBackend},
    },
    desktop::space::{SpaceRenderElements, SurfaceTree},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop,
        },
        wayland_server::protocol::wl_output,
    },
    utils::Rectangle,
    wayland::output::{Mode, Output, PhysicalProperties},
};

use slog::Logger;

use crate::{CalloopData, Smallvil};

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

    let output = Output::new::<_>(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );
    let _global = output.create_global::<Smallvil>(&display.handle());
    output.change_current_state(
        Some(mode),
        Some(wl_output::Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let mut damage_tracked_renderer = DamageTrackedRenderer::from_output(&output);

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    let mut full_redraw = 0u8;

    let timer = Timer::immediate();
    event_loop.handle().insert_source(timer, move |_, _, data| {
        winit_dispatch(
            &mut backend,
            &mut winit,
            data,
            &output,
            &mut damage_tracked_renderer,
            &mut full_redraw,
            &log,
        )
        .unwrap();
        TimeoutAction::ToDuration(Duration::from_millis(16))
    })?;

    Ok(())
}

pub fn winit_dispatch(
    backend: &mut WinitGraphicsBackend,
    winit: &mut WinitEventLoop,
    data: &mut CalloopData,
    output: &Output,
    damage_tracked_renderer: &mut DamageTrackedRenderer,
    full_redraw: &mut u8,
    log: &Logger,
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
        WinitEvent::Input(event) => state.process_input_event(event),
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
        smithay::desktop::space::render_output::<
            Gles2Renderer,
            SurfaceTree,
            SpaceRenderElements<Gles2Renderer>,
        >(
            output,
            backend.renderer(),
            0,
            &[(&state.space, &[])],
            &[],
            damage_tracked_renderer,
            [0.1, 0.1, 0.1, 1.0],
            log,
        )
        .unwrap()
    });

    backend.submit(Some(&[damage])).unwrap();

    state
        .space
        .send_frames(state.start_time.elapsed().as_millis() as u32);

    state.space.refresh(&display.handle());
    display.flush_clients()?;

    Ok(())
}
