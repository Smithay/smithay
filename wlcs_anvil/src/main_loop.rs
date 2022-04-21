use std::{
    cell::RefCell, collections::HashMap, os::unix::prelude::IntoRawFd, rc::Rc, sync::atomic::Ordering,
    time::Duration,
};

use smithay::{
    reexports::{
        calloop::{
            channel::{Channel, Event as ChannelEvent},
            EventLoop,
        },
        wayland_server::{
            protocol::{wl_output, wl_pointer, wl_surface},
            Client, Display,
        },
    },
    wayland::{
        output::{Mode, Output, PhysicalProperties},
        seat::CursorImageStatus,
        SERIAL_COUNTER as SCOUNTER,
    },
};

use anvil::{
    drawing::{draw_cursor, draw_dnd_icon},
    render::render_output,
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

    fn reset_buffers(&mut self, _output: &Output) {}
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
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

    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "WLCS".into(),
        },
        logger.clone(),
    );
    let _global = output.create_global(&mut *display.borrow_mut());
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);
    state.space.borrow_mut().map_output(&output, 1.0, (0, 0));

    while state.running.load(Ordering::SeqCst) {
        // pretend to draw something
        {
            let mut elements = Vec::new();
            let dnd_guard = state.dnd_icon.lock().unwrap();
            let mut cursor_guard = state.cursor_status.lock().unwrap();

            // draw the dnd icon if any
            if let Some(ref surface) = *dnd_guard {
                if surface.as_ref().is_alive() {
                    elements.push(draw_dnd_icon(
                        surface.clone(),
                        state.pointer_location.to_i32_round(),
                        &logger,
                    ));
                }
            }

            // draw the cursor as relevant
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Image(ref surface) = *cursor_guard {
                reset = !surface.as_ref().is_alive();
            }
            if reset {
                *cursor_guard = CursorImageStatus::Default;
            }
            if let CursorImageStatus::Image(ref surface) = *cursor_guard {
                elements.push(draw_cursor(
                    surface.clone(),
                    state.pointer_location.to_i32_round(),
                    &logger,
                ));
            }

            let _ = render_output(
                &output,
                &mut *state.space.borrow_mut(),
                &mut renderer,
                0,
                &*elements,
                &logger,
            );
        }

        // Send frame events so that client start drawing their next frame
        state
            .space
            .borrow()
            .send_frames(state.start_time.elapsed().as_millis() as u32);

        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.borrow_mut().refresh();
            state.popups.borrow_mut().cleanup();
            display.borrow_mut().flush_clients(&mut state);
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
        WlcsEvent::PositionWindow {
            client_id,
            surface_id,
            location,
        } => {
            // find the surface
            let client = state.backend_data.clients.get(&client_id);
            let mut space = state.space.borrow_mut();
            let toplevel = space.windows().find(|w| {
                let surface = w.toplevel().wl_surface().unwrap();
                surface.as_ref().client().as_ref() == client && surface.as_ref().id() == surface_id
            });
            if let Some(toplevel) = toplevel.cloned() {
                // set its location
                space.map_window(&toplevel, location, false);
            }
        }
        // pointer inputs
        WlcsEvent::NewPointer { .. } => {}
        WlcsEvent::PointerMoveAbsolute { location, .. } => {
            state.pointer_location = location;
            let serial = SCOUNTER.next_serial();
            let under = state.surface_under();
            let time = state.start_time.elapsed().as_millis() as u32;
            state.pointer.motion(location, under, serial, time);
        }
        WlcsEvent::PointerMoveRelative { delta, .. } => {
            state.pointer_location += delta;
            let serial = SCOUNTER.next_serial();
            let under = state.surface_under();
            let time = state.start_time.elapsed().as_millis() as u32;
            state.pointer.motion(state.pointer_location, under, serial, time);
        }
        WlcsEvent::PointerButtonDown { button_id, .. } => {
            let serial = SCOUNTER.next_serial();
            if !state.pointer.is_grabbed() {
                let under = state.surface_under();
                if let Some((s, _)) = under.as_ref() {
                    let mut space = state.space.borrow_mut();
                    if let Some(window) = space.window_for_surface(s).cloned() {
                        space.raise_window(&window, true);
                    }
                }
                state
                    .keyboard
                    .set_focus(under.as_ref().map(|&(ref s, _)| s), serial);
            }
            let time = state.start_time.elapsed().as_millis() as u32;
            state
                .pointer
                .button(button_id as u32, wl_pointer::ButtonState::Pressed, serial, time);
        }
        WlcsEvent::PointerButtonUp { button_id, .. } => {
            let serial = SCOUNTER.next_serial();
            let time = state.start_time.elapsed().as_millis() as u32;
            state
                .pointer
                .button(button_id as u32, wl_pointer::ButtonState::Released, serial, time);
        }
        WlcsEvent::PointerRemoved { .. } => {}
        // touch inputs
        WlcsEvent::NewTouch { .. } => {}
        WlcsEvent::TouchDown { .. } => {}
        WlcsEvent::TouchMove { .. } => {}
        WlcsEvent::TouchUp { .. } => {}
        WlcsEvent::TouchRemoved { .. } => {}
    }
}
