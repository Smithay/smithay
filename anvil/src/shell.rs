use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use rand;

use smithay::{
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_pointer::ButtonState, wl_shell_surface, wl_surface},
        Display,
    },
    utils::Rectangle,
    wayland::{
        compositor::{compositor_init, CompositorToken, RegionAttributes, SurfaceAttributes, SurfaceEvent},
        data_device::DnDIconRole,
        seat::{AxisFrame, CursorImageRole, GrabStartData, PointerGrab, PointerInnerHandle, Seat},
        shell::{
            legacy::{
                wl_shell_init, ShellRequest, ShellState as WlShellState, ShellSurfaceKind, ShellSurfaceRole,
            },
            xdg::{
                xdg_shell_init, PopupConfigure, ShellState as XdgShellState, ToplevelConfigure, XdgRequest,
                XdgSurfaceRole,
            },
        },
        SERIAL_COUNTER as SCOUNTER,
    },
};

use crate::{
    buffer_utils::BufferUtils,
    window_map::{Kind as SurfaceKind, WindowMap},
};

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

struct MoveSurfaceGrab {
    start_data: GrabStartData,
    window_map: Rc<RefCell<MyWindowMap>>,
    toplevel: SurfaceKind<Roles>,
    initial_window_location: (i32, i32),
}

impl PointerGrab for MoveSurfaceGrab {
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle<'_>,
        location: (f64, f64),
        _focus: Option<(wl_surface::WlSurface, (f64, f64))>,
        _serial: u32,
        _time: u32,
    ) {
        let dx = location.0 - self.start_data.location.0;
        let dy = location.1 - self.start_data.location.1;
        let new_window_x = (self.initial_window_location.0 as f64 + dx) as i32;
        let new_window_y = (self.initial_window_location.1 as f64 + dy) as i32;

        self.window_map
            .borrow_mut()
            .set_location(&self.toplevel, (new_window_x, new_window_y));
    }

    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: u32,
        time: u32,
    ) {
        handle.button(button, state, serial, time);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(serial, time);
        }
    }

    fn axis(&mut self, handle: &mut PointerInnerHandle<'_>, details: AxisFrame) {
        handle.axis(details)
    }

    fn start_data(&self) -> &GrabStartData {
        &self.start_data
    }
}

pub fn init_shell(
    display: &mut Display,
    buffer_utils: BufferUtils,
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
            SurfaceEvent::Commit => surface_commit(&surface, ctoken, &buffer_utils),
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
            XdgRequest::Move {
                surface,
                seat,
                serial,
            } => {
                let seat = Seat::from_resource(&seat).unwrap();
                // TODO: touch move.
                let pointer = seat.get_pointer().unwrap();

                // Check that this surface has a click grab.
                if !pointer.has_grab(serial) {
                    return;
                }

                let start_data = pointer.grab_start_data().unwrap();

                // If the focus was for a different surface, ignore the request.
                if start_data.focus.is_none()
                    || !start_data
                        .focus
                        .as_ref()
                        .unwrap()
                        .0
                        .as_ref()
                        .same_client_as(surface.get_surface().unwrap().as_ref())
                {
                    return;
                }

                let toplevel = SurfaceKind::Xdg(surface);
                let initial_window_location = xdg_window_map.borrow().location(&toplevel).unwrap();

                let grab = MoveSurfaceGrab {
                    start_data,
                    window_map: xdg_window_map.clone(),
                    toplevel,
                    initial_window_location,
                };

                pointer.set_grab(grab, serial);
            }
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
            match req {
                ShellRequest::SetKind {
                    surface,
                    kind: ShellSurfaceKind::Toplevel,
                } => {
                    // place the window at a random location in the [0;800]x[0;800] square
                    use rand::distributions::{Distribution, Uniform};
                    let range = Uniform::new(0, 800);
                    let mut rng = rand::thread_rng();
                    let x = range.sample(&mut rng);
                    let y = range.sample(&mut rng);
                    shell_window_map
                        .borrow_mut()
                        .insert(SurfaceKind::Wl(surface), (x, y));
                }
                ShellRequest::Move {
                    surface,
                    seat,
                    serial,
                } => {
                    let seat = Seat::from_resource(&seat).unwrap();
                    // TODO: touch move.
                    let pointer = seat.get_pointer().unwrap();

                    // Check that this surface has a click grab.
                    if !pointer.has_grab(serial) {
                        return;
                    }

                    let start_data = pointer.grab_start_data().unwrap();

                    // If the focus was for a different surface, ignore the request.
                    if start_data.focus.is_none()
                        || !start_data
                            .focus
                            .as_ref()
                            .unwrap()
                            .0
                            .as_ref()
                            .same_client_as(surface.get_surface().unwrap().as_ref())
                    {
                        return;
                    }

                    let toplevel = SurfaceKind::Wl(surface);
                    let initial_window_location = shell_window_map.borrow().location(&toplevel).unwrap();

                    let grab = MoveSurfaceGrab {
                        start_data,
                        window_map: shell_window_map.clone(),
                        toplevel,
                        initial_window_location,
                    };

                    pointer.set_grab(grab, serial);
                }
                _ => (),
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
    pub dimensions: Option<(i32, i32)>,
    pub input_region: Option<RegionAttributes>,
}

fn surface_commit(
    surface: &wl_surface::WlSurface,
    token: CompositorToken<Roles>,
    buffer_utils: &BufferUtils,
) {
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
                // If this fails, the buffer will be discarded later by the drawing code.
                data.dimensions = buffer_utils.dimensions(data.buffer.as_ref().unwrap());
            }
            Some(None) => {
                // erase the contents
                if let Some(old_buffer) = data.buffer.take() {
                    old_buffer.release();
                }
                data.texture = None;
                data.dimensions = None;
            }
            None => {}
        }
    });
}

fn get_size(attrs: &SurfaceAttributes) -> Option<(i32, i32)> {
    attrs
        .user_data
        .get::<SurfaceData>()
        .and_then(|data| data.dimensions)
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

    let input_region = &attrs.user_data.get::<SurfaceData>().unwrap().input_region;

    // If there's no input region, we're done.
    if input_region.is_none() {
        return true;
    }

    input_region.as_ref().unwrap().contains(point)
}
