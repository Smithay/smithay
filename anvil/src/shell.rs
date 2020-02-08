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
                XdgSurfacePendingState, XdgSurfaceRole,
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
    ctoken: MyCompositorToken,
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

        let mut new_window_width = self.initial_window_size.0;
        let mut new_window_height = self.initial_window_size.1;

        let left_right = wl_shell_surface::Resize::Left | wl_shell_surface::Resize::Right;
        let top_bottom = wl_shell_surface::Resize::Top | wl_shell_surface::Resize::Bottom;

        if self.edges.intersects(left_right) {
            if self.edges.intersects(wl_shell_surface::Resize::Left) {
                dx = -dx;
            }

            new_window_width = (self.initial_window_size.0 as f64 + dx) as i32;
        }

        if self.edges.intersects(top_bottom) {
            if self.edges.intersects(wl_shell_surface::Resize::Top) {
                dy = -dy;
            }

            new_window_height = (self.initial_window_size.1 as f64 + dy) as i32;
        }

        let (min_size, max_size) =
            self.ctoken
                .with_surface_data(self.toplevel.get_surface().unwrap(), |attrs| {
                    let data = attrs.user_data.get::<SurfaceData>().unwrap();
                    (data.min_size, data.max_size)
                });

        let min_width = min_size.0.max(1);
        let min_height = min_size.1.max(1);
        let max_width = if max_size.0 == 0 {
            i32::max_value()
        } else {
            max_size.0
        };
        let max_height = if max_size.1 == 0 {
            i32::max_value()
        } else {
            max_size.1
        };

        new_window_width = new_window_width.max(min_width).min(max_width);
        new_window_height = new_window_height.max(min_height).min(max_height);

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

                self.ctoken
                    .with_surface_data(self.toplevel.get_surface().unwrap(), |attrs| {
                        let data = attrs.user_data.get_mut::<SurfaceData>().unwrap();
                        if let ResizeState::Resizing(resize_data) = data.resize_state {
                            data.resize_state = ResizeState::WaitingForFinalAck(resize_data, serial);
                        } else {
                            panic!("invalid resize state: {:?}", data.resize_state);
                        }
                    });
            } else {
                self.ctoken
                    .with_surface_data(self.toplevel.get_surface().unwrap(), |attrs| {
                        let data = attrs.user_data.get_mut::<SurfaceData>().unwrap();
                        if let ResizeState::Resizing(resize_data) = data.resize_state {
                            data.resize_state = ResizeState::WaitingForCommit(resize_data);
                        } else {
                            panic!("invalid resize state: {:?}", data.resize_state);
                        }
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

                let toplevel = SurfaceKind::Xdg(surface.clone());
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

                compositor_token.with_surface_data(surface.get_surface().unwrap(), move |attrs| {
                    attrs.user_data.get_mut::<SurfaceData>().unwrap().resize_state =
                        ResizeState::Resizing(ResizeData {
                            edges,
                            initial_window_location,
                            initial_window_size,
                        });
                });

                let grab = ResizeSurfaceGrab {
                    start_data,
                    ctoken: compositor_token,
                    toplevel,
                    edges,
                    initial_window_size,
                    last_window_size: initial_window_size,
                };

                pointer.set_grab(grab, serial);
            }
            XdgRequest::AckConfigure { surface, .. } => {
                let waiting_for_serial = compositor_token.with_surface_data(&surface, |attrs| {
                    if let Some(data) = attrs.user_data.get_mut::<SurfaceData>() {
                        if let ResizeState::WaitingForFinalAck(_, serial) = data.resize_state {
                            return Some(serial);
                        }
                    }

                    None
                });

                if let Some(serial) = waiting_for_serial {
                    let acked = compositor_token
                        .with_role_data(&surface, |role: &mut XdgSurfaceRole| {
                            !role.pending_configures.contains(&serial)
                        })
                        .unwrap();

                    if acked {
                        compositor_token.with_surface_data(&surface, |attrs| {
                            let data = attrs.user_data.get_mut::<SurfaceData>().unwrap();
                            if let ResizeState::WaitingForFinalAck(resize_data, _) = data.resize_state {
                                data.resize_state = ResizeState::WaitingForCommit(resize_data);
                            } else {
                                unreachable!()
                            }
                        })
                    }
                }
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

                    let toplevel = SurfaceKind::Wl(surface.clone());
                    let initial_window_location = shell_window_map.borrow().location(&toplevel).unwrap();
                    let geometry = shell_window_map.borrow().geometry(&toplevel).unwrap();
                    let initial_window_size = (geometry.width, geometry.height);

                    compositor_token.with_surface_data(surface.get_surface().unwrap(), move |attrs| {
                        attrs.user_data.get_mut::<SurfaceData>().unwrap().resize_state =
                            ResizeState::Resizing(ResizeData {
                                edges,
                                initial_window_location,
                                initial_window_size,
                            });
                    });

                    let grab = ResizeSurfaceGrab {
                        start_data,
                        ctoken: compositor_token,
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

/// Information about the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    /// The edges the surface is being resized with.
    edges: wl_shell_surface::Resize,
    /// The initial window location.
    initial_window_location: (i32, i32),
    /// The initial window size (geometry width and height).
    initial_window_size: (i32, i32),
}

/// State of the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ResizeState {
    /// The surface is not being resized.
    NotResizing,
    /// The surface is currently being resized.
    Resizing(ResizeData),
    /// The resize has finished, and the surface needs to ack the final configure.
    WaitingForFinalAck(ResizeData, u32),
    /// The resize has finished, and the surface needs to commit its final state.
    WaitingForCommit(ResizeData),
}

impl Default for ResizeState {
    fn default() -> Self {
        ResizeState::NotResizing
    }
}

#[derive(Default)]
pub struct SurfaceData {
    pub buffer: Option<wl_buffer::WlBuffer>,
    pub texture: Option<crate::glium_drawer::TextureMetadata>,
    pub dimensions: Option<(i32, i32)>,
    pub geometry: Option<Rectangle>,
    pub input_region: Option<RegionAttributes>,
    pub resize_state: ResizeState,
    /// Minimum width and height, as requested by the surface.
    ///
    /// `0` means unlimited.
    pub min_size: (i32, i32),
    /// Maximum width and height, as requested by the surface.
    ///
    /// `0` means unlimited.
    pub max_size: (i32, i32),
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
    let mut geometry = None;
    let mut min_size = (0, 0);
    let mut max_size = (0, 0);
    let _ = token.with_role_data(surface, |role: &mut XdgSurfaceRole| {
        if let XdgSurfacePendingState::Toplevel(ref state) = role.pending_state {
            min_size = state.min_size;
            max_size = state.max_size;
        }

        geometry = role.window_geometry;
    });

    let refresh = token.with_surface_data(surface, |attributes| {
        attributes.user_data.insert_if_missing(SurfaceData::default);
        let data = attributes.user_data.get_mut::<SurfaceData>().unwrap();

        data.geometry = geometry;
        data.input_region = attributes.input_region.clone();
        data.min_size = min_size;
        data.max_size = max_size;

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
        // Get the geometry outside since it uses the token, and so would block inside.
        let Rectangle { width, height, .. } = window_map.geometry(&toplevel).unwrap();

        let new_location = token.with_surface_data(surface, |attributes| {
            let data = attributes.user_data.get_mut::<SurfaceData>().unwrap();

            let mut new_location = None;

            // If the window is being resized by top or left, its location must be adjusted
            // accordingly.
            match data.resize_state {
                ResizeState::Resizing(resize_data)
                | ResizeState::WaitingForFinalAck(resize_data, _)
                | ResizeState::WaitingForCommit(resize_data) => {
                    let ResizeData {
                        edges,
                        initial_window_location,
                        initial_window_size,
                    } = resize_data;

                    if edges.intersects(wl_shell_surface::Resize::TopLeft) {
                        let mut location = window_map.location(&toplevel).unwrap();

                        if edges.intersects(wl_shell_surface::Resize::Left) {
                            location.0 = initial_window_location.0 + (initial_window_size.0 - width);
                        }
                        if edges.intersects(wl_shell_surface::Resize::Top) {
                            location.1 = initial_window_location.1 + (initial_window_size.1 - height);
                        }

                        new_location = Some(location);
                    }
                }
                ResizeState::NotResizing => (),
            }

            // Finish resizing.
            if let ResizeState::WaitingForCommit(_) = data.resize_state {
                data.resize_state = ResizeState::NotResizing;
            }

            new_location
        });

        if let Some(location) = new_location {
            window_map.set_location(&toplevel, location);
        }
    }
}
