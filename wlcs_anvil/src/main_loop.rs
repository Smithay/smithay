use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use smithay::{
    backend::{
        input::ButtonState,
        renderer::{damage::DamageTrackedRenderer, element::AsRenderElements},
    },
    input::pointer::{ButtonEvent, CursorImageAttributes, CursorImageStatus, MotionEvent},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            channel::{Channel, Event as ChannelEvent},
            EventLoop,
        },
        wayland_server::{protocol::wl_surface, Client, Display, Resource},
    },
    utils::{IsAlive, Point, Scale, SERIAL_COUNTER as SCOUNTER},
    wayland::{compositor, input_method::InputMethodSeat},
};

use anvil::{drawing::PointerElement, render::*, state::Backend, AnvilState, CalloopData, ClientState};

use crate::{renderer::DummyRenderer, WlcsEvent};

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
        EventLoop::<CalloopData<TestState>>::try_new().expect("Failed to init the event loop.");

    let mut display = Display::new().expect("Failed to init display");
    let dh = display.handle();

    let logger = slog::Logger::root(slog::Discard, slog::o!());

    let test_state = TestState {
        clients: HashMap::new(),
    };

    let mut state = AnvilState::init(
        &mut display,
        event_loop.handle(),
        test_state,
        logger.clone(),
        false,
    );

    event_loop
        .handle()
        .insert_source(channel, move |event, &mut (), data| match event {
            ChannelEvent::Msg(evt) => handle_event(evt, &mut data.state, &mut data.display),
            ChannelEvent::Closed => handle_event(WlcsEvent::Exit, &mut data.state, &mut data.display),
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
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "WLCS".into(),
        },
        logger.clone(),
    );
    let _global = output.create_global::<AnvilState<TestState>>(&dh);
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracked_renderer = DamageTrackedRenderer::from_output(&output);
    let mut pointer_element = PointerElement::default();

    while state.running.load(Ordering::SeqCst) {
        // pretend to draw something
        {
            let scale = Scale::from(output.current_scale().fractional_scale());
            let mut cursor_guard = state.cursor_status.lock().unwrap();
            let mut elements: Vec<CustomRenderElements<_>> = Vec::new();

            // draw input method square if any
            let input_method = state.seat.input_method().unwrap();
            let rectangle = input_method.coordinates();
            let position = Point::from((
                rectangle.loc.x + rectangle.size.w,
                rectangle.loc.y + rectangle.size.h,
            ));
            input_method.with_surface(|surface| {
                elements.extend(AsRenderElements::<DummyRenderer>::render_elements(
                    &smithay::desktop::space::SurfaceTree::from_surface(surface, position),
                    position.to_physical_precise_round(scale),
                    scale,
                ));
            });

            // draw the cursor as relevant
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
                reset = !surface.alive();
            }
            if reset {
                *cursor_guard = CursorImageStatus::Default;
            }

            let cursor_hotspot = if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
                compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                })
            } else {
                (0, 0).into()
            };
            let cursor_pos = state.pointer_location - cursor_hotspot.to_f64();
            let cursor_pos_scaled = cursor_pos.to_physical(scale).to_i32_round();

            pointer_element.set_status(cursor_guard.clone());
            elements.extend(pointer_element.render_elements(cursor_pos_scaled, scale));

            // draw the dnd icon if any
            if let Some(surface) = state.dnd_icon.as_ref() {
                if surface.alive() {
                    elements.extend(AsRenderElements::<DummyRenderer>::render_elements(
                        &smithay::desktop::space::SurfaceTree::from_surface(
                            surface,
                            cursor_pos.to_i32_round(),
                        ),
                        cursor_pos_scaled,
                        scale,
                    ));
                }
            }

            let _ = render_output(
                &output,
                &state.space,
                &elements,
                &mut renderer,
                &mut damage_tracked_renderer,
                0,
                &logger,
            );
        }

        // Send frame events so that client start drawing their next frame
        state
            .space
            .elements()
            .for_each(|window| window.send_frame(state.start_time.elapsed().as_millis() as u32));

        let mut calloop_data = CalloopData { state, display };
        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut calloop_data);
        CalloopData { state, display } = calloop_data;

        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.refresh();
            state.popups.cleanup();
            display.flush_clients().unwrap();
        }
    }
}

fn handle_event(
    event: WlcsEvent,
    state: &mut AnvilState<TestState>,
    display: &mut Display<AnvilState<TestState>>,
) {
    match event {
        WlcsEvent::Exit => state.running.store(false, Ordering::SeqCst),
        WlcsEvent::NewClient { stream, client_id } => {
            let client = display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
                .expect("Failed to insert client");
            state.backend_data.clients.insert(client_id, client);
        }
        WlcsEvent::PositionWindow {
            client_id,
            surface_id,
            location,
        } => {
            // find the surface
            let client = state.backend_data.clients.get(&client_id);
            let toplevel = state.space.elements().find(|w| {
                let surface = w.toplevel().wl_surface();
                display.handle().get_client(surface.id()).ok().as_ref() == client
                    && surface.id().protocol_id() == surface_id
            });
            if let Some(toplevel) = toplevel.cloned() {
                // set its location
                state.space.map_element(toplevel, location, false);
            }
        }
        // pointer inputs
        WlcsEvent::NewPointer { .. } => {}
        WlcsEvent::PointerMoveAbsolute { location, .. } => {
            state.pointer_location = location;
            let serial = SCOUNTER.next_serial();
            let under = state.surface_under();
            let time = state.start_time.elapsed().as_millis() as u32;
            state.seat.get_pointer().unwrap().motion(
                state,
                under,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            );
        }
        WlcsEvent::PointerMoveRelative { delta, .. } => {
            state.pointer_location += delta;
            let serial = SCOUNTER.next_serial();
            let under = state.surface_under();
            let time = state.start_time.elapsed().as_millis() as u32;
            state.seat.get_pointer().unwrap().motion(
                state,
                under,
                &MotionEvent {
                    location: state.pointer_location,
                    serial,
                    time,
                },
            );
        }
        WlcsEvent::PointerButtonDown { button_id, .. } => {
            let serial = SCOUNTER.next_serial();
            let pointer = state.seat.get_pointer().unwrap();
            if !pointer.is_grabbed() {
                let under = state
                    .space
                    .element_under(pointer.current_location())
                    .map(|(w, _)| w.clone());
                if let Some(window) = under.as_ref() {
                    state.space.raise_element(window, true);
                }
                state
                    .seat
                    .get_keyboard()
                    .unwrap()
                    .set_focus(state, under.map(Into::into), serial);
            }
            let time = state.start_time.elapsed().as_millis() as u32;
            pointer.button(
                state,
                &ButtonEvent {
                    button: button_id as u32,
                    state: ButtonState::Pressed,
                    serial,
                    time,
                },
            );
        }
        WlcsEvent::PointerButtonUp { button_id, .. } => {
            let serial = SCOUNTER.next_serial();
            let time = state.start_time.elapsed().as_millis() as u32;
            state.seat.get_pointer().unwrap().button(
                state,
                &ButtonEvent {
                    button: button_id as u32,
                    state: ButtonState::Released,
                    serial,
                    time,
                },
            );
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
