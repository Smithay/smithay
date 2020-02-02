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
    utils::Rectangle,
    wayland::{
        compositor::{compositor_init, CompositorToken, RegionAttributes, SurfaceAttributes, SurfaceEvent},
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

pub type MyWindowMap = WindowMap<
    Roles,
    fn(&SurfaceAttributes) -> Option<(i32, i32)>,
    fn(&SurfaceAttributes, (f64, f64)) -> bool,
>;

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
    let window_map = Rc::new(RefCell::new(WindowMap::new(
        compositor_token,
        get_size as _,
        contains_point as _,
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
    pub input_region: Option<RegionAttributes>,
}

fn surface_commit(surface: &wl_surface::WlSurface, token: CompositorToken<Roles>) {
    token.with_surface_data(surface, |attributes| {
        attributes.user_data.insert_if_missing(SurfaceData::default);
        let data = attributes.user_data.get_mut::<SurfaceData>().unwrap();

        data.input_region = attributes.input_region.clone();

        // we retrieve the contents of the associated buffer and copy it
        match attributes.buffer.take() {
            Some(Some((buffer, (_x, _y)))) => {
                // new contents
                // TODO: handle hotspot coordinates
                if let Some(old_buffer) = data.buffer.replace(buffer) {
                    old_buffer.release();
                }
                data.texture = None;
            }
            Some(None) => {
                // erase the contents
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

fn contains_point(attrs: &SurfaceAttributes, point: (f64, f64)) -> bool {
    let (w, h) = match get_size(attrs) {
        None => return false, // If the surface has no size, it can't have an input region.
        Some(wh) => wh,
    };

    let rect = Rectangle {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };

    let point = (point.0 as i32, point.1 as i32);

    // The input region is always within the surface itself, so if the surface itself doesn't contain the
    // point we can return false.
    if !rect.contains(point) {
        return false;
    }

    // If there's no input region, we're done.
    if attrs.input_region.is_none() {
        return true;
    }

    attrs.input_region.as_ref().unwrap().contains(point)
}
