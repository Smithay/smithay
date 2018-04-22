use super::WindowMap;
use glium::texture::Texture2d;
use rand;
use smithay::backend::graphics::egl::wayland::{BufferAccessError, Format};
use smithay::backend::graphics::egl::wayland::{EGLDisplay, EGLImages};
use smithay::wayland::compositor::{compositor_init, CompositorToken, SurfaceAttributes, SurfaceEvent};
use smithay::wayland::shell::xdg::{xdg_shell_init, PopupConfigure, ShellEvent, ShellState, ShellSurfaceRole,
                                   ToplevelConfigure};
use smithay::wayland::shm::with_buffer_contents as shm_buffer_contents;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use wayland_server::{Display, LoopToken, Resource};
use wayland_server::protocol::{wl_buffer, wl_callback, wl_surface};

define_roles!(Roles => [ ShellSurface, ShellSurfaceRole ] );

#[derive(Default)]
pub struct SurfaceData {
    pub buffer: Option<Buffer>,
    pub texture: Option<Texture2d>,
}

pub enum Buffer {
    Egl { images: EGLImages },
    Shm { data: Vec<u8>, size: (u32, u32) },
}

fn surface_commit(
    surface: &Resource<wl_surface::WlSurface>,
    token: CompositorToken<SurfaceData, Roles>,
    display: &RefCell<Option<EGLDisplay>>,
) {
    // we retrieve the contents of the associated buffer and copy it
    token.with_surface_data(surface, |attributes| {
        match attributes.buffer.take() {
            Some(Some((buffer, (_x, _y)))) => {
                // we ignore hotspot coordinates in this simple example
                match if let Some(display) = display.borrow().as_ref() {
                    display.egl_buffer_contents(buffer)
                } else {
                    Err(BufferAccessError::NotManaged(buffer))
                } {
                    Ok(images) => {
                        match images.format {
                            Format::RGB => {}
                            Format::RGBA => {}
                            _ => {
                                // we don't handle the more complex formats here.
                                attributes.user_data.buffer = None;
                                attributes.user_data.texture = None;
                                return;
                            }
                        };
                        attributes.user_data.texture = None;
                        attributes.user_data.buffer = Some(Buffer::Egl { images });
                    }
                    Err(BufferAccessError::NotManaged(buffer)) => {
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
                            attributes.user_data.texture = None;
                            attributes.user_data.buffer = Some(Buffer::Shm { data: new_vec, size: (data.width as u32, data.height as u32) });
                        }).expect("Got EGL buffer with no set EGLDisplay. You need to unbind your EGLContexts before dropping them!");
                        buffer.send(wl_buffer::Event::Release);
                    }
                    Err(err) => panic!("EGL error: {}", err),
                }
            }
            Some(None) => {
                // erase the contents
                attributes.user_data.buffer = None;
                attributes.user_data.texture = None;
            }
            None => {}
        }
    });
}

fn get_size(attrs: &SurfaceAttributes<SurfaceData>) -> Option<(i32, i32)> {
    attrs
        .user_data
        .buffer
        .as_ref()
        .map(|ref buffer| match **buffer {
            Buffer::Shm { ref size, .. } => *size,
            Buffer::Egl { ref images } => (images.width, images.height),
        })
        .map(|(x, y)| (x as i32, y as i32))
}

pub type MyWindowMap =
    WindowMap<SurfaceData, Roles, (), fn(&SurfaceAttributes<SurfaceData>) -> Option<(i32, i32)>>;

pub fn init_shell(
    display: &mut Display,
    looptoken: LoopToken,
    log: ::slog::Logger,
    egl_display: Rc<RefCell<Option<EGLDisplay>>>,
) -> (
    CompositorToken<SurfaceData, Roles>,
    Arc<Mutex<ShellState<SurfaceData, Roles, ()>>>,
    Rc<RefCell<MyWindowMap>>,
) {
    // Create the compositor
    let c_egl_display = egl_display.clone();
    let (compositor_token, _, _) = compositor_init(
        display,
        looptoken.clone(),
        move |request, (surface, ctoken)| match request {
            SurfaceEvent::Commit => surface_commit(&surface, ctoken, &*c_egl_display),
            SurfaceEvent::Frame { callback } => {
                // TODO: uncomment when https://github.com/rust-lang/rust/issues/50153 is fixed
                // callback.implement(|e, _| match e {}, None::<fn(_,_)>).send(wl_callback::Event::Done { callback_data: 0})
            }
        },
        log.clone(),
    );

    // Init a window map, to track the location of our windows
    let window_map = Rc::new(RefCell::new(WindowMap::<_, _, (), _>::new(
        compositor_token,
        get_size as _,
    )));

    // init the xdg_shell
    let shell_window_map = window_map.clone();
    let (shell_state, _, _) = xdg_shell_init(
        display,
        looptoken,
        compositor_token.clone(),
        move |shell_event, ()| match shell_event {
            ShellEvent::NewToplevel { surface } => {
                // place the window at a random location in the [0;300]x[0;300] square
                use rand::distributions::{IndependentSample, Range};
                let range = Range::new(0, 300);
                let mut rng = rand::thread_rng();
                let x = range.ind_sample(&mut rng);
                let y = range.ind_sample(&mut rng);
                surface.send_configure(ToplevelConfigure {
                    size: None,
                    states: vec![],
                    serial: 42,
                });
                shell_window_map.borrow_mut().insert(surface, (x, y));
            }
            ShellEvent::NewPopup { surface } => surface.send_configure(PopupConfigure {
                size: (10, 10),
                position: (10, 10),
                serial: 42,
            }),
            _ => (),
        },
        log.clone(),
    );

    (compositor_token, shell_state, window_map)
}
