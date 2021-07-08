use std::{
    cell::RefCell, collections::HashMap, os::unix::prelude::IntoRawFd, rc::Rc, sync::atomic::Ordering,
    time::Duration,
};

use smithay::{
    backend::{
        renderer::{Frame, Renderer, Transform},
        SwapBuffersError,
    },
    reexports::{
        calloop::{
            channel::{Channel, Event as ChannelEvent},
            EventLoop,
        },
        wayland_server::{protocol::wl_output, Client, Display},
    },
    wayland::{
        output::{Mode, PhysicalProperties},
        seat::CursorImageStatus,
    },
};

use anvil::{
    drawing::{draw_cursor, draw_dnd_icon, draw_windows},
    state::Backend,
    AnvilState,
};

use crate::WlcsEvent;

pub const OUTPUT_NAME: &str = "anvil";

struct TestState {
    clients: HashMap<i32, Client>,
}

impl Backend for TestState {
    fn seat_name(&self) -> String {
        "anvil_wlcs".into()
    }
}

pub fn run(channel: Channel<WlcsEvent>) {
    let mut event_loop =
        EventLoop::<AnvilState<TestState>>::try_new().expect("Failed to init the event loop.");

    let display = Rc::new(RefCell::new(Display::new()));

    let logger = slog::Logger::root(slog::Discard, slog::o!());

    let test_state = TestState {
        clients: HashMap::new(),
    };

    let mut state = AnvilState::init(
        display.clone(),
        event_loop.handle(),
        test_state,
        logger.clone(),
        false,
    );

    event_loop
        .handle()
        .insert_source(channel, move |event, &mut (), state| match event {
            ChannelEvent::Msg(evt) => handle_event(evt, state),
            ChannelEvent::Closed => handle_event(WlcsEvent::Exit, state),
        })
        .unwrap();

    let mut renderer = crate::renderer::DummyRenderer::new();

    let mode = Mode {
        size: (800, 600).into(),
        refresh: 60_000,
    };

    state.output_map.borrow_mut().add(
        OUTPUT_NAME,
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        mode,
    );

    while state.running.load(Ordering::SeqCst) {
        // pretend to draw something
        {
            let output_geometry = state
                .output_map
                .borrow()
                .find_by_name(OUTPUT_NAME)
                .unwrap()
                .geometry();

            renderer
                .render((800, 600).into(), Transform::Normal, |renderer, frame| {
                    frame.clear([0.8, 0.8, 0.9, 1.0])?;

                    // draw the windows
                    draw_windows(
                        renderer,
                        frame,
                        &*state.window_map.borrow(),
                        output_geometry,
                        1.0,
                        &logger,
                    )?;

                    // draw the dnd icon if any
                    {
                        let guard = state.dnd_icon.lock().unwrap();
                        if let Some(ref surface) = *guard {
                            if surface.as_ref().is_alive() {
                                draw_dnd_icon(
                                    renderer,
                                    frame,
                                    surface,
                                    state.pointer_location.to_i32_floor(),
                                    1.0,
                                    &logger,
                                )?;
                            }
                        }
                    }
                    // draw the cursor as relevant
                    {
                        let mut guard = state.cursor_status.lock().unwrap();
                        // reset the cursor if the surface is no longer alive
                        let mut reset = false;
                        if let CursorImageStatus::Image(ref surface) = *guard {
                            reset = !surface.as_ref().is_alive();
                        }
                        if reset {
                            *guard = CursorImageStatus::Default;
                        }

                        // draw as relevant
                        if let CursorImageStatus::Image(ref surface) = *guard {
                            draw_cursor(
                                renderer,
                                frame,
                                surface,
                                state.pointer_location.to_i32_floor(),
                                1.0,
                                &logger,
                            )?;
                        }
                    }

                    Ok(())
                })
                .map_err(Into::<SwapBuffersError>::into)
                .and_then(|x| x)
                .unwrap();
        }

        // Send frame events so that client start drawing their next frame
        state
            .window_map
            .borrow()
            .send_frames(state.start_time.elapsed().as_millis() as u32);
        display.borrow_mut().flush_clients(&mut state);

        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            display.borrow_mut().flush_clients(&mut state);
            state.window_map.borrow_mut().refresh();
            state.output_map.borrow_mut().refresh();
        }
    }
}

fn handle_event(event: WlcsEvent, state: &mut AnvilState<TestState>) {
    match event {
        WlcsEvent::Exit => state.running.store(false, Ordering::SeqCst),
        WlcsEvent::NewClient { stream, client_id } => {
            let display = state.display.clone();
            let client = unsafe { display.borrow_mut().create_client(stream.into_raw_fd(), state) };
            state.backend_data.clients.insert(client_id, client);
        }
        e => {
            // TODO: handle the actual events
            eprintln!("Unhandled event: {:?}", e);
        }
    }
}
