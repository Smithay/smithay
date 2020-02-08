use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use rand;

use smithay::{
    reexports::{
        wayland_protocols::xdg_shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_buffer, wl_pointer::ButtonState, wl_shell_surface, wl_surface},
            Display,
        },
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

pub type MyWindowMap = WindowMap<Roles>;

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

struct ResizeSurfaceGrab {
    start_data: GrabStartData,
    toplevel: SurfaceKind<Roles>,
    edges: wl_shell_surface::Resize,
    initial_window_size: (i32, i32),
    last_window_size: (i32, i32),
}

impl PointerGrab for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle<'_>,
        location: (f64, f64),
        _focus: Option<(wl_surface::WlSurface, (f64, f64))>,
        serial: u32,
        _time: u32,
    ) {
        let mut dx = location.0 - self.start_data.location.0;
        let mut dy = location.1 - self.start_data.location.1;

        let left_right = wl_shell_surface::Resize::Left | wl_shell_surface::Resize::Right;
        let top_bottom = wl_shell_surface::Resize::Top | wl_shell_surface::Resize::Bottom;
        let new_window_width = if self.edges.intersects(left_right) {
            if self.edges.intersects(wl_shell_surface::Resize::Left) {
                dx = -dx;
            }

            ((self.initial_window_size.0 as f64 + dx) as i32).max(1)
        } else {
            self.initial_window_size.0
        };
        let new_window_height = if self.edges.intersects(top_bottom) {
            if self.edges.intersects(wl_shell_surface::Resize::Top) {
                dy = -dy;
            }

            ((self.initial_window_size.1 as f64 + dy) as i32).max(1)
        } else {
            self.initial_window_size.1
        };

        self.last_window_size = (new_window_width, new_window_height);

        match &self.toplevel {
            SurfaceKind::Xdg(xdg) => xdg.send_configure(ToplevelConfigure {
                size: Some(self.last_window_size),
                states: vec![xdg_toplevel::State::Resizing],
                serial,
            }),
            SurfaceKind::Wl(wl) => wl.send_configure(
                (self.last_window_size.0 as u32, self.last_window_size.1 as u32),
                self.edges,
            ),
        }
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

            if let SurfaceKind::Xdg(xdg) = &self.toplevel {
                // Send the final configure without the resizing state.
                xdg.send_configure(ToplevelConfigure {
                    size: Some(self.last_window_size),
                    states: vec![],
                    serial,
                });
            }
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
    // TODO: this is awkward...
    let almost_window_map = Rc::new(RefCell::new(None::<Rc<RefCell<MyWindowMap>>>));
    let almost_window_map_compositor = almost_window_map.clone();

    // Create the compositor
    let (compositor_token, _, _) = compositor_init(
        display,
        move |request, surface, ctoken| match request {
            SurfaceEvent::Commit => {
                let window_map = almost_window_map_compositor.borrow();
                let window_map = window_map.as_ref().unwrap();
                surface_commit(&surface, ctoken, &buffer_utils, &*window_map)
            }
            SurfaceEvent::Frame { callback } => callback
                .implement_closure(|_, _| unreachable!(), None::<fn(_)>, ())
                .done(0),
        },
        log.clone(),
    );

    // Init a window map, to track the location of our windows
    let window_map = Rc::new(RefCell::new(WindowMap::new(compositor_token)));
    *almost_window_map.borrow_mut() = Some(window_map.clone());

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
            XdgRequest::Resize {
                surface,
                seat,
                serial,
                edges,
            } => {
                let seat = Seat::from_resource(&seat).unwrap();
                // TODO: touch resize.
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
                let geometry = xdg_window_map.borrow().geometry(&toplevel).unwrap();
                let initial_window_size = (geometry.width, geometry.height);

                let edges = match edges {
                    xdg_toplevel::ResizeEdge::Top => wl_shell_surface::Resize::Top,
                    xdg_toplevel::ResizeEdge::Bottom => wl_shell_surface::Resize::Bottom,
                    xdg_toplevel::ResizeEdge::Left => wl_shell_surface::Resize::Left,
                    xdg_toplevel::ResizeEdge::TopLeft => wl_shell_surface::Resize::TopLeft,
                    xdg_toplevel::ResizeEdge::BottomLeft => wl_shell_surface::Resize::BottomLeft,
                    xdg_toplevel::ResizeEdge::Right => wl_shell_surface::Resize::Right,
                    xdg_toplevel::ResizeEdge::TopRight => wl_shell_surface::Resize::TopRight,
                    xdg_toplevel::ResizeEdge::BottomRight => wl_shell_surface::Resize::BottomRight,
                    _ => return,
                };

                let grab = ResizeSurfaceGrab {
                    start_data,
                    toplevel,
                    edges,
                    initial_window_size,
                    last_window_size: initial_window_size,
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
                    surface.send_configure((0, 0), wl_shell_surface::Resize::None);
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
                ShellRequest::Resize {
                    surface,
                    seat,
                    serial,
                    edges,
                } => {
                    let seat = Seat::from_resource(&seat).unwrap();
                    // TODO: touch resize.
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
                    let geometry = shell_window_map.borrow().geometry(&toplevel).unwrap();
                    let initial_window_size = (geometry.width, geometry.height);

                    let grab = ResizeSurfaceGrab {
                        start_data,
                        toplevel,
                        edges,
                        initial_window_size,
                        last_window_size: initial_window_size,
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
    pub geometry: Option<Rectangle>,
    pub input_region: Option<RegionAttributes>,
}

impl SurfaceData {
    /// Returns the size of the surface.
    pub fn size(&self) -> Option<(i32, i32)> {
        self.dimensions
    }

    /// Checks if the surface's input region contains the point.
    pub fn contains_point(&self, point: (f64, f64)) -> bool {
        let (w, h) = match self.size() {
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
        if self.input_region.is_none() {
            return true;
        }

        self.input_region.as_ref().unwrap().contains(point)
    }
}

fn surface_commit(
    surface: &wl_surface::WlSurface,
    token: CompositorToken<Roles>,
    buffer_utils: &BufferUtils,
    window_map: &RefCell<MyWindowMap>,
) {
    let geometry = token
        .with_role_data(surface, |role: &mut XdgSurfaceRole| role.window_geometry)
        .unwrap_or(None);

    let refresh = token.with_surface_data(surface, |attributes| {
        attributes.user_data.insert_if_missing(SurfaceData::default);
        let data = attributes.user_data.get_mut::<SurfaceData>().unwrap();

        data.geometry = geometry;
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

        window_map.borrow().find(surface)
    });

    if let Some(toplevel) = refresh {
        let mut window_map = window_map.borrow_mut();
        window_map.refresh_toplevel(&toplevel);
    }
}
