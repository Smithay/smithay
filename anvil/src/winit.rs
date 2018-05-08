use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use smithay::wayland::shm::init_shm_global;
use smithay::wayland::seat::Seat;
use smithay::wayland::compositor::{SubsurfaceRole, TraversalAction};
use smithay::wayland::compositor::roles::Role;
use smithay::wayland::output::{Mode, Output, PhysicalProperties};
use smithay::backend::input::InputBackend;
use smithay::backend::winit;
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::graphics::egl::wayland::{EGLWaylandExtensions, Format};
use smithay::wayland_server::{Display, EventLoop};
use smithay::wayland_server::protocol::wl_output;

use glium::Surface;

use slog::Logger;

use glium_drawer::GliumDrawer;
use shell::{init_shell, Buffer};
use input_handler::AnvilInputHandler;

pub fn run_winit(display: &mut Display, event_loop: &mut EventLoop, log: Logger) -> Result<(), ()> {
    let (renderer, mut input) = winit::init(log.clone()).map_err(|_| ())?;

    let egl_display = Rc::new(RefCell::new(
        if let Ok(egl_display) = renderer.bind_wl_display(&display) {
            info!(log, "EGL hardware-acceleration enabled");
            Some(egl_display)
        } else {
            None
        },
    ));

    let (w, h) = renderer.get_framebuffer_dimensions();
    let drawer = GliumDrawer::from(renderer);

    let name = display.add_socket_auto().unwrap().into_string().unwrap();
    info!(log, "Listening on wayland socket"; "name" => name.clone());
    ::std::env::set_var("WAYLAND_DISPLAY", name);

    let running = Arc::new(AtomicBool::new(true));

    /*
     * Initialize the globals
     */

    init_shm_global(display, event_loop.token(), vec![], log.clone());

    let (compositor_token, _, _, window_map) =
        init_shell(display, event_loop.token(), log.clone(), egl_display);

    let (mut seat, _) = Seat::new(display, event_loop.token(), "winit".into(), log.clone());

    let pointer = seat.add_pointer();
    let keyboard = seat.add_keyboard("", "fr", "oss", None, 1000, 500)
        .expect("Failed to initialize the keyboard");

    let (output, _) = Output::new(
        display,
        event_loop.token(),
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

    output.change_current_state(
        Some(Mode {
            width: w as i32,
            height: h as i32,
            refresh: 60_000,
        }),
        None,
        None,
    );
    output.set_preferred(Mode {
        width: w as i32,
        height: h as i32,
        refresh: 60_000,
    });

    input.set_handler(AnvilInputHandler::new(
        log.clone(),
        pointer,
        keyboard,
        window_map.clone(),
        (0, 0),
        running.clone(),
        Rc::new(RefCell::new((0.0, 0.0))),
    ));

    info!(log, "Initialization completed, starting the main loop.");

    loop {
        input.dispatch_new_events().unwrap();

        let mut frame = drawer.draw();
        frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = drawer.borrow().get_framebuffer_dimensions();
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
                                    // there is actually something to draw !
                                    if attributes.user_data.texture.is_none() {
                                        let mut remove = false;
                                        match attributes.user_data.buffer {
                                            Some(Buffer::Egl { ref images }) => {
                                                match images.format {
                                                    Format::RGB | Format::RGBA => {
                                                        attributes.user_data.texture =
                                                            drawer.texture_from_egl(&images);
                                                    }
                                                    _ => {
                                                        // we don't handle the more complex formats here.
                                                        attributes.user_data.texture = None;
                                                        remove = true;
                                                    }
                                                };
                                            }
                                            Some(Buffer::Shm { ref data, ref size }) => {
                                                attributes.user_data.texture =
                                                    Some(drawer.texture_from_mem(data, *size));
                                            }
                                            _ => {}
                                        }
                                        if remove {
                                            attributes.user_data.buffer = None;
                                        }
                                    }

                                    if let Some(ref texture) = attributes.user_data.texture {
                                        if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                                            x += subdata.location.0;
                                            y += subdata.location.1;
                                        }
                                        drawer.render_texture(
                                            &mut frame,
                                            texture,
                                            match *attributes.user_data.buffer.as_ref().unwrap() {
                                                Buffer::Egl { ref images } => images.y_inverted,
                                                Buffer::Shm { .. } => false,
                                            },
                                            match *attributes.user_data.buffer.as_ref().unwrap() {
                                                Buffer::Egl { ref images } => (images.width, images.height),
                                                Buffer::Shm { ref size, .. } => *size,
                                            },
                                            (x, y),
                                            screen_dimensions,
                                            ::glium::Blend {
                                                color: ::glium::BlendingFunction::Addition {
                                                    source: ::glium::LinearBlendingFactor::One,
                                                    destination:
                                                        ::glium::LinearBlendingFactor::OneMinusSourceAlpha,
                                                },
                                                alpha: ::glium::BlendingFunction::Addition {
                                                    source: ::glium::LinearBlendingFactor::One,
                                                    destination:
                                                        ::glium::LinearBlendingFactor::OneMinusSourceAlpha,
                                                },
                                                ..Default::default()
                                            },
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
