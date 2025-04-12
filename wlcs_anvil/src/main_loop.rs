use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use smithay::{
    backend::{
        input::ButtonState,
        renderer::{
            damage::OutputDamageTracker,
            element::AsRenderElements,
            test::{DummyFramebuffer, DummyRenderer},
        },
    },
    input::pointer::{
        ButtonEvent, CursorImageAttributes, CursorImageStatus, MotionEvent, RelativeMotionEvent,
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            channel::{Channel, Event as ChannelEvent},
            EventLoop,
        },
        wayland_server::{protocol::wl_surface, Client, Display, Resource},
    },
    utils::{IsAlive, Scale, SERIAL_COUNTER as SCOUNTER},
    wayland::compositor,
};

use anvil::{drawing::PointerElement, render::*, state::Backend, AnvilState, ClientState};

use crate::WlcsEvent;

const OUTPUT_NAME: &str = "anvil";

struct TestState {
    clients: HashMap<i32, Client>,
}

impl Backend for TestState {
    fn seat_name(&self) -> String {
        "anvil_wlcs".into()
    }

    fn reset_buffers(&mut self, _output: &Output) {}
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
    fn update_led_state(&mut self, _led_state: smithay::input::keyboard::LedState) {}
}

pub fn run(channel: Channel<WlcsEvent>) {
    let mut event_loop =
        EventLoop::<AnvilState<TestState>>::try_new().expect("Failed to init the event loop.");

    let display = Display::new().expect("Failed to init display");
    let test_state = TestState {
        clients: HashMap::new(),
    };
    let mut state = AnvilState::init(display, event_loop.handle(), test_state, false);

    event_loop
        .handle()
        .insert_source(channel, move |event, &mut (), data| match event {
            ChannelEvent::Msg(evt) => handle_event(evt, data),
            ChannelEvent::Closed => handle_event(WlcsEvent::Exit, data),
        })
        .unwrap();

    let mut renderer = DummyRenderer::default();
    let mut framebuffer = DummyFramebuffer;

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
    );
    let _global = output.create_global::<AnvilState<TestState>>(&state.display_handle);
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let mut pointer_element = PointerElement::default();

    while state.running.load(Ordering::SeqCst) {
        // pretend to draw something
        {
            let scale = Scale::from(output.current_scale().fractional_scale());
            let mut elements: Vec<CustomRenderElements<_>> = Vec::new();

            // draw the cursor as relevant
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Surface(ref surface) = state.cursor_status {
                reset = !surface.alive();
            }
            if reset {
                state.cursor_status = CursorImageStatus::default_named();
            }

            let cursor_hotspot = if let CursorImageStatus::Surface(ref surface) = state.cursor_status {
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
            let cursor_pos = state.pointer.current_location();

            pointer_element.set_status(state.cursor_status.clone());
            elements.extend(
                pointer_element.render_elements(
                    &mut renderer,
                    (cursor_pos - cursor_hotspot.to_f64())
                        .to_physical(scale)
                        .to_i32_round(),
                    scale,
                    1.0,
                ),
            );

            // draw the dnd icon if any
            if let Some(icon) = state.dnd_icon.as_ref() {
                if icon.surface.alive() {
                    let dnd_icon_pos = (cursor_pos + icon.offset.to_f64())
                        .to_physical(scale)
                        .to_i32_round();
                    elements.extend(AsRenderElements::<DummyRenderer>::render_elements(
                        &smithay::desktop::space::SurfaceTree::from_surface(&icon.surface),
                        &mut renderer,
                        dnd_icon_pos,
                        scale,
                        1.0,
                    ));
                }
            }

            let _ = render_output(
                &output,
                &state.space,
                elements,
                &mut renderer,
                &mut framebuffer,
                &mut damage_tracker,
                0,
                false,
            );
        }

        // Send frame events so that client start drawing their next frame
        state.space.elements().for_each(|window| {
            window.send_frame(&output, state.clock.now(), Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            })
        });

        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.refresh();
            state.popups.cleanup();
            state.display_handle.flush_clients().unwrap();
        }
    }
}

fn handle_event(event: WlcsEvent, state: &mut AnvilState<TestState>) {
    match event {
        WlcsEvent::Exit => state.running.store(false, Ordering::SeqCst),
        WlcsEvent::NewClient { stream, client_id } => {
            let client = state
                .display_handle
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
                if let Some(surface) = w.wl_surface() {
                    state.display_handle.get_client(surface.id()).ok().as_ref() == client
                        && surface.id().protocol_id() == surface_id
                } else {
                    false
                }
            });
            if let Some(toplevel) = toplevel.cloned() {
                // set its location
                state.space.map_element(toplevel, location, false);
            }
        }
        // pointer inputs
        WlcsEvent::NewPointer { .. } => {}
        WlcsEvent::PointerMoveAbsolute { location, .. } => {
            let serial = SCOUNTER.next_serial();
            let under = state.surface_under(location);
            let time = Duration::from(state.clock.now()).as_millis() as u32;
            let ptr = state.pointer.clone();
            ptr.motion(
                state,
                under,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            );
            ptr.frame(state);
        }
        WlcsEvent::PointerMoveRelative { delta, .. } => {
            let pointer_location = state.pointer.current_location() + delta;
            let serial = SCOUNTER.next_serial();
            let under = state.surface_under(pointer_location);
            let time = Duration::from(state.clock.now()).as_millis() as u32;
            let utime = Duration::from(state.clock.now()).as_micros() as u64;
            let ptr = state.pointer.clone();
            ptr.motion(
                state,
                under.clone(),
                &MotionEvent {
                    location: pointer_location,
                    serial,
                    time,
                },
            );
            ptr.relative_motion(
                state,
                under,
                &RelativeMotionEvent {
                    delta,
                    delta_unaccel: delta,
                    utime,
                },
            );
            ptr.frame(state);
        }
        WlcsEvent::PointerButtonDown { button_id, .. } => {
            let serial = SCOUNTER.next_serial();
            let ptr = state.seat.get_pointer().unwrap();
            if !ptr.is_grabbed() {
                let under = state
                    .space
                    .element_under(ptr.current_location())
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
            let time = Duration::from(state.clock.now()).as_millis() as u32;
            ptr.button(
                state,
                &ButtonEvent {
                    button: button_id as u32,
                    state: ButtonState::Pressed,
                    serial,
                    time,
                },
            );
            ptr.frame(state);
        }
        WlcsEvent::PointerButtonUp { button_id, .. } => {
            let serial = SCOUNTER.next_serial();
            let time = Duration::from(state.clock.now()).as_millis() as u32;
            let ptr = state.seat.get_pointer().unwrap();
            ptr.button(
                state,
                &ButtonEvent {
                    button: button_id as u32,
                    state: ButtonState::Released,
                    serial,
                    time,
                },
            );
            ptr.frame(state);
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
