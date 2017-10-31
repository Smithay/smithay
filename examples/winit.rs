#[macro_use]
extern crate glium;
extern crate rand;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
#[macro_use(define_roles)]
extern crate smithay;
extern crate wayland_server;

mod helpers;

use glium::Surface;
use helpers::{init_shell, GliumDrawer, MyWindowMap};
use slog::{Drain, Logger};
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::input::{self, Event, InputBackend, InputHandler, KeyboardKeyEvent, PointerAxisEvent,
                              PointerButtonEvent, PointerMotionAbsoluteEvent};
use smithay::backend::winit;
use smithay::wayland::compositor::{SubsurfaceRole, TraversalAction};
use smithay::wayland::compositor::roles::Role;
use smithay::wayland::output::{Mode, Output, PhysicalProperties};
use smithay::wayland::seat::{KeyboardHandle, PointerHandle, Seat};
use smithay::wayland::shm::init_shm_global;
use std::cell::RefCell;
use std::rc::Rc;
use wayland_server::protocol::{wl_output, wl_pointer};

struct WinitInputHandler {
    log: Logger,
    pointer: PointerHandle,
    keyboard: KeyboardHandle,
    window_map: Rc<RefCell<MyWindowMap>>,
    pointer_location: (f64, f64),
    serial: u32,
}

impl WinitInputHandler {
    fn next_serial(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }
}

impl InputHandler<winit::WinitInputBackend> for WinitInputHandler {
    fn on_seat_created(&mut self, _: &input::Seat) {
        /* never happens with winit */
    }
    fn on_seat_destroyed(&mut self, _: &input::Seat) {
        /* never happens with winit */
    }
    fn on_seat_changed(&mut self, _: &input::Seat) {
        /* never happens with winit */
    }
    fn on_keyboard_key(&mut self, _: &input::Seat, evt: winit::WinitKeyboardInputEvent) {
        let keycode = evt.key_code();
        let state = evt.state();
        debug!(self.log, "key"; "keycode" => keycode, "state" => format!("{:?}", state));
        let serial = self.next_serial();
        self.keyboard.input(keycode, state, serial, |_, _| true);
    }
    fn on_pointer_move(&mut self, _: &input::Seat, _: input::UnusedEvent) {
        /* never happens with winit */
    }
    fn on_pointer_move_absolute(&mut self, _: &input::Seat, evt: winit::WinitMouseMovedEvent) {
        // on winit, mouse events are already in pixel coordinates
        let (x, y) = evt.position();
        self.pointer_location = (x, y);
        let serial = self.next_serial();
        let under = self.window_map.borrow().get_surface_under((x, y));
        self.pointer.motion(
            under.as_ref().map(|&(ref s, (x, y))| (s, x, y)),
            serial,
            evt.time(),
        );
    }
    fn on_pointer_button(&mut self, _: &input::Seat, evt: winit::WinitMouseInputEvent) {
        let serial = self.next_serial();
        let button = match evt.button() {
            input::MouseButton::Left => 0x110,
            input::MouseButton::Right => 0x111,
            input::MouseButton::Middle => 0x112,
            input::MouseButton::Other(b) => b as u32,
        };
        let state = match evt.state() {
            input::MouseButtonState::Pressed => {
                // change the keyboard focus
                let under = self.window_map
                    .borrow_mut()
                    .get_surface_and_bring_to_top(self.pointer_location);
                self.keyboard
                    .set_focus(under.as_ref().map(|&(ref s, _)| s), serial);
                wl_pointer::ButtonState::Pressed
            }
            input::MouseButtonState::Released => wl_pointer::ButtonState::Released,
        };
        self.pointer.button(button, state, serial, evt.time());
    }
    fn on_pointer_axis(&mut self, _: &input::Seat, evt: winit::WinitMouseWheelEvent) {
        let axis = match evt.axis() {
            input::Axis::Vertical => wayland_server::protocol::wl_pointer::Axis::VerticalScroll,
            input::Axis::Horizontal => wayland_server::protocol::wl_pointer::Axis::HorizontalScroll,
        };
        self.pointer.axis(axis, evt.amount(), evt.time());
    }
    fn on_touch_down(&mut self, _: &input::Seat, _: winit::WinitTouchStartedEvent) {
        /* not done in this example */
    }
    fn on_touch_motion(&mut self, _: &input::Seat, _: winit::WinitTouchMovedEvent) {
        /* not done in this example */
    }
    fn on_touch_up(&mut self, _: &input::Seat, _: winit::WinitTouchEndedEvent) {
        /* not done in this example */
    }
    fn on_touch_cancel(&mut self, _: &input::Seat, _: winit::WinitTouchCancelledEvent) {
        /* not done in this example */
    }
    fn on_touch_frame(&mut self, _: &input::Seat, _: input::UnusedEvent) {
        /* never happens with winit */
    }
    fn on_input_config_changed(&mut self, _: &mut ()) {
        /* never happens with winit */
    }
}

fn main() {
    // A logger facility, here we use the terminal for this example
    let log = Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    // Initialize a simple backend for testing
    let (renderer, mut input) = winit::init(log.clone()).unwrap();

    let (mut display, mut event_loop) = wayland_server::create_display();

    /*
     * Initialize the globals
     */

    init_shm_global(&mut event_loop, vec![], log.clone());

    let (compositor_token, _shell_state_token, window_map) = init_shell(&mut event_loop, log.clone());

    let (seat_token, _) = Seat::new(&mut event_loop, "winit".into(), log.clone());

    let pointer = event_loop.state().get_mut(&seat_token).add_pointer();
    let keyboard = event_loop
        .state()
        .get_mut(&seat_token)
        .add_keyboard("", "fr", "oss", None, 1000, 500)
        .expect("Failed to initialize the keyboard");

    let (output_token, _output_global) = Output::new(
        &mut event_loop,
        "Winit".into(),
        PhysicalProperties {
            width: 0,
            height: 0,
            subpixel: wl_output::Subpixel::Unknown,
            maker: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );

    let (w, h) = renderer.get_framebuffer_dimensions();
    event_loop
        .state()
        .get_mut(&output_token)
        .change_current_state(
            Some(Mode {
                width: w as i32,
                height: h as i32,
                refresh: 60_000,
            }),
            None,
            None,
        );
    event_loop
        .state()
        .get_mut(&output_token)
        .set_preferred(Mode {
            width: w as i32,
            height: h as i32,
            refresh: 60_000,
        });

    /*
     * Initialize glium
     */
    let drawer = GliumDrawer::from(renderer);

    input.set_handler(WinitInputHandler {
        log: log.clone(),
        pointer,
        keyboard,
        window_map: window_map.clone(),
        pointer_location: (0.0, 0.0),
        serial: 0,
    });

    /*
     * Add a listening socket:
     */
    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    println!("Listening on socket: {}", name);

    loop {
        input.dispatch_new_events().unwrap();

        let mut frame = drawer.draw();
        frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, None, None);
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = drawer.get_framebuffer_dimensions();
            window_map
                .borrow()
                .with_windows_from_bottom_to_top(|toplevel_surface, initial_place| {
                    if let Some(wl_surface) = toplevel_surface.get_surface() {
                        // this surface is a root of a subsurface tree that needs to be drawn
                        compositor_token
                            .with_surface_tree_upward(
                                wl_surface,
                                initial_place,
                                |_surface, attributes, role, &(mut x, mut y)| {
                                    if let Some((ref contents, (w, h))) = attributes.user_data.buffer {
                                        // there is actually something to draw !
                                        if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                                            x += subdata.x;
                                            y += subdata.y;
                                        }
                                        drawer.render(
                                            &mut frame,
                                            contents,
                                            (w, h),
                                            (x, y),
                                            screen_dimensions,
                                        );
                                        TraversalAction::DoChildren((x, y))
                                    } else {
                                        // we are not display, so our children are neither
                                        TraversalAction::SkipChildren
                                    }
                                },
                            )
                            .unwrap();
                    }
                });
        }
        frame.finish().unwrap();

        event_loop.dispatch(Some(16)).unwrap();
        display.flush_clients();

        window_map.borrow_mut().refresh();
    }
}
