use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use rand;

use smithay::{
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_shell_surface, wl_surface},
        Display,
    },
    wayland::{
        compositor::{compositor_init, CompositorToken, SurfaceAttributes, SurfaceEvent},
        data_device::DnDIconRole,
        seat::CursorImageRole,
        shell::{
            legacy::{
                wl_shell_init, ShellRequest, ShellState as WlShellState, ShellSurfaceKind, ShellSurfaceRole,
            },
            xdg::{
                xdg_shell_init, PopupConfigure, ShellState as XdgShellState, ToplevelConfigure, XdgRequest,
                XdgSurfaceRole,
            },
        },
    },
};

use crate::window_map::{Kind as SurfaceKind, WindowMap};

define_roles!(Roles =>
    [ XdgSurface, XdgSurfaceRole ]
    [ ShellSurface, ShellSurfaceRole]
    [ DnDIcon, DnDIconRole ]
    [ CursorImage, CursorImageRole ]
);

pub type MyWindowMap = WindowMap<Roles, fn(&SurfaceAttributes) -> Option<(i32, i32)>>;

pub type MyCompositorToken = CompositorToken<Roles>;

pub fn init_shell(
    display: &mut Display,
    log: ::slog::Logger,
) -> (
    CompositorToken<Roles>,
    Arc<Mutex<XdgShellState<Roles>>>,
    Arc<Mutex<WlShellState<Roles>>>,
    Rc<RefCell<MyWindowMap>>,
) {
    // Create the compositor
    let (compositor_token, _, _) = compositor_init(
        display,
        move |request, surface, ctoken| match request {
            SurfaceEvent::Commit => surface_commit(&surface, ctoken),
            SurfaceEvent::Frame { callback } => callback
                .implement_closure(|_, _| unreachable!(), None::<fn(_)>, ())
                .done(0),
        },
        log.clone(),
    );

    // Init a window map, to track the location of our windows
    let window_map = Rc::new(RefCell::new(WindowMap::<_, _>::new(
        compositor_token,
        get_size as _,
    )));

    // init the xdg_shell
    let xdg_window_map = window_map.clone();
    let (xdg_shell_state, _, _) = xdg_shell_init(
        display,
        compositor_token,
        move |shell_event| match shell_event {
            XdgRequest::NewToplevel { surface } => {
                // place the window at a random location in the [0;800]x[0;800] square
                use rand::distributions::{Distribution, Uniform};
                let range = Uniform::new(0, 800);
                let mut rng = rand::thread_rng();
                let x = range.sample(&mut rng);
                let y = range.sample(&mut rng);
                surface.send_configure(ToplevelConfigure {
                    size: None,
                    states: vec![],
                    serial: 42,
                });
                xdg_window_map
                    .borrow_mut()
                    .insert(SurfaceKind::Xdg(surface), (x, y));
            }
            XdgRequest::NewPopup { surface } => surface.send_configure(PopupConfigure {
                size: (10, 10),
                position: (10, 10),
                serial: 42,
            }),
            _ => (),
        },
        log.clone(),
    );

    // init the wl_shell
    let shell_window_map = window_map.clone();
    let (wl_shell_state, _) = wl_shell_init(
        display,
        compositor_token,
        move |req: ShellRequest<_>| {
            if let ShellRequest::SetKind {
                surface,
                kind: ShellSurfaceKind::Toplevel,
            } = req
            {
                // place the window at a random location in the [0;800]x[0;800] square
                use rand::distributions::{Distribution, Uniform};
                let range = Uniform::new(0, 800);
                let mut rng = rand::thread_rng();
                let x = range.sample(&mut rng);
                let y = range.sample(&mut rng);
                surface.send_configure((0, 0), wl_shell_surface::Resize::None);
                shell_window_map
                    .borrow_mut()
                    .insert(SurfaceKind::Wl(surface), (x, y));
            }
        },
        log.clone(),
    );

    (compositor_token, xdg_shell_state, wl_shell_state, window_map)
}

#[derive(Default)]
pub struct SurfaceData {
    pub buffer: Option<wl_buffer::WlBuffer>,
    pub texture: Option<crate::glium_drawer::TextureMetadata>,
}

fn surface_commit(surface: &wl_surface::WlSurface, token: CompositorToken<Roles>) {
    // we retrieve the contents of the associated buffer and copy it
    token.with_surface_data(surface, |attributes| {
        attributes.user_data.insert_if_missing(|| SurfaceData::default());
        match attributes.buffer.take() {
            Some(Some((buffer, (_x, _y)))) => {
                // new contents
                // TODO: handle hotspot coordinates
                let data = attributes.user_data.get_mut::<SurfaceData>().unwrap();
                if let Some(old_buffer) = data.buffer.replace(buffer) {
                    old_buffer.release();
                }
                data.texture = None;
            }
            Some(None) => {
                // erase the contents
                let data = attributes.user_data.get_mut::<SurfaceData>().unwrap();
                if let Some(old_buffer) = data.buffer.take() {
                    old_buffer.release();
                }
                data.texture = None;
            }
            None => {}
        }
    });
}

fn get_size(attrs: &SurfaceAttributes) -> Option<(i32, i32)> {
    attrs.user_data.get::<SurfaceData>().and_then(|data| {
        data.texture
            .as_ref()
            .map(|ref meta| meta.dimensions)
            .map(|(x, y)| (x as i32, y as i32))
    })
}
