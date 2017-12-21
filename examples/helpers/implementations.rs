use super::{GliumDrawer, WindowMap};
use glium::texture::{Texture2d, UncompressedFloatFormat};
use rand;
use smithay::backend::graphics::egl::{EGLGraphicsBackend, EGLImage};
use smithay::wayland::compositor::{compositor_init, CompositorToken, SurfaceAttributes,
                                   SurfaceUserImplementation};
use smithay::wayland::shell::{shell_init, PopupConfigure, ShellState, ShellSurfaceRole,
                              ShellSurfaceUserImplementation, ToplevelConfigure};
use smithay::wayland::shm::with_buffer_contents as shm_buffer_contents;
use smithay::wayland::drm::{with_buffer_contents as drm_buffer_contents, Attributes, Format};
use std::cell::RefCell;
use std::rc::Rc;
use wayland_server::{EventLoop, StateToken};

define_roles!(Roles => [ ShellSurface, ShellSurfaceRole ] );

#[derive(Default)]
pub struct SurfaceData {
    pub texture: Option<Texture2d>,
    pub buffer: Option<Buffer>,
}

pub enum Buffer {
    Egl { images: Vec<EGLImage>, attributes: Attributes },
    Shm { data: Vec<u8>, size: (u32, u32) },
}

unsafe impl Send for Buffer {}

pub fn surface_implementation<G: EGLGraphicsBackend + 'static>() -> SurfaceUserImplementation<SurfaceData, Roles, GliumDrawer<G>> {
    SurfaceUserImplementation {
        commit: |_, drawer, surface, token| {
            // we retrieve the contents of the associated buffer and copy it
            token.with_surface_data(surface, |attributes| {
                match attributes.buffer() {
                    Some(ref buffer) => {
                        // we ignore hotspot coordinates in this simple example (`attributes.buffer_coordinates()`)
                        if drm_buffer_contents(&buffer, drawer.borrow(), |attributes, images| {
                            let format = match attributes.format {
                                Format::RGB => UncompressedFloatFormat::U8U8U8,
                                Format::RGBA => UncompressedFloatFormat::U8U8U8U8,
                                _ => {
                                    // we don't handle the more complex formats here.
                                    attributes.user_data.buffer = None;
                                    attributes.user_data.texture = None;
                                    return;
                                },
                            };
                            attributes.user_data.texture = Some(drawer.texture_from_egl(
                                images[0] /*Both simple formats only have one plane*/,
                                format,
                                (attributes.width, attributes.height)
                            ));
                            attributes.user_data.buffer = Some(Buffer::Egl { images, attributes });
                        }).is_err() {
                            shm_buffer_contents(&buffer, |slice, data| {
                                let offset = data.offset as usize;
                                let stride = data.stride as usize;
                                let width = data.width as usize;
                                let height = data.height as usize;
                                let mut new_vec = Vec::with_capacity(width * height * 4);
                                for i in 0..height {
                                    new_vec
                                        .extend(&slice[(offset + i * stride)..(offset + i * stride + width * 4)]);
                                }
                                attributes.user_data.texture = Some(drawer.texture_from_mem(&new_vec, data.width as u32, data.height as u32));
                                attributes.user_data.buffer =
                                    Some(Buffer::Shm { data: new_vec, position: (data.width as u32, data.height as u32) });
                            }).unwrap();
                        }
                    }
                    None => {
                        // erase the contents
                        attributes.user_data.buffer = None;
                    }
                }
            });
        },
        frame: |_, _, _, callback, _| {
            callback.done(0);
        },
    }
}

pub struct ShellIData<F> {
    pub token: CompositorToken<SurfaceData, Roles, ()>,
    pub window_map: Rc<RefCell<super::WindowMap<SurfaceData, Roles, (), (), F>>>,
}

pub fn shell_implementation<F>() -> ShellSurfaceUserImplementation<SurfaceData, Roles, (), ShellIData<F>, ()>
where
    F: Fn(&SurfaceAttributes<SurfaceData>) -> Option<(i32, i32)>,
{
    ShellSurfaceUserImplementation {
        new_client: |_, _, _| {},
        client_pong: |_, _, _| {},
        new_toplevel: |_, idata, toplevel| {
            // place the window at a random location in the [0;300]x[0;300] square
            use rand::distributions::{IndependentSample, Range};
            let range = Range::new(0, 300);
            let mut rng = rand::thread_rng();
            let x = range.ind_sample(&mut rng);
            let y = range.ind_sample(&mut rng);
            idata.window_map.borrow_mut().insert(toplevel, (x, y));
            ToplevelConfigure {
                size: None,
                states: vec![],
                serial: 42,
            }
        },
        new_popup: |_, _, _| {
            PopupConfigure {
                size: (10, 10),
                position: (10, 10),
                serial: 42,
            }
        },
        move_: |_, _, _, _, _| {},
        resize: |_, _, _, _, _, _| {},
        grab: |_, _, _, _, _| {},
        change_display_state: |_, _, _, _, _, _, _| {
            ToplevelConfigure {
                size: None,
                states: vec![],
                serial: 42,
            }
        },
        show_window_menu: |_, _, _, _, _, _, _| {},
    }
}

fn get_size(attrs: &SurfaceAttributes<SurfaceData>) -> Option<(i32, i32)> {
    attrs
        .user_data
        .buffer
        .as_ref()
        .map(|&(_, (w, h))| (w as i32, h as i32))
}

pub type MyWindowMap = WindowMap<
    SurfaceData,
    Roles,
    (),
    (),
    fn(&SurfaceAttributes<SurfaceData>) -> Option<(i32, i32)>,
>;

pub fn init_shell<ID: 'static>(
    evl: &mut EventLoop, log: ::slog::Logger, data: ID)
    -> (
        CompositorToken<SurfaceData, Roles, ID>,
        StateToken<ShellState<SurfaceData, Roles, ID, ()>>,
        Rc<RefCell<MyWindowMap>>,
    ) {
    let (compositor_token, _, _) = compositor_init(evl, surface_implementation(), data, log.clone());

    let window_map = Rc::new(RefCell::new(WindowMap::<_, _, _, (), _>::new(
        compositor_token,
        get_size as _,
    )));

    let (shell_state_token, _, _) = shell_init(
        evl,
        compositor_token,
        shell_implementation(),
        ShellIData {
            token: compositor_token,
            window_map: window_map.clone(),
        },
        log.clone(),
    );

    (compositor_token, shell_state_token, window_map)
}
