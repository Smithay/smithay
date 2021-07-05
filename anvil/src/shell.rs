use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use smithay::{
    backend::renderer::buffer_dimensions,
    reexports::{
        wayland_protocols::xdg_shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_buffer, wl_output, wl_pointer::ButtonState, wl_shell_surface, wl_surface},
            Display,
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Size},
    wayland::{
        compositor::{
            compositor_init, is_sync_subsurface, with_states, with_surface_tree_upward, BufferAssignment,
            SurfaceAttributes, TraversalAction,
        },
        seat::{AxisFrame, GrabStartData, PointerGrab, PointerInnerHandle, Seat},
        shell::{
            legacy::{wl_shell_init, ShellRequest, ShellState as WlShellState, ShellSurfaceKind},
            xdg::{
                xdg_shell_init, Configure, ShellState as XdgShellState, SurfaceCachedState,
                XdgPopupSurfaceRoleAttributes, XdgRequest, XdgToplevelSurfaceRoleAttributes,
            },
        },
        Serial,
    },
};

use crate::{
    output_map::OutputMap,
    state::AnvilState,
    window_map::{Kind as SurfaceKind, PopupKind, WindowMap},
};

struct MoveSurfaceGrab {
    start_data: GrabStartData,
    window_map: Rc<RefCell<WindowMap>>,
    toplevel: SurfaceKind,
    initial_window_location: Point<i32, Logical>,
}

impl PointerGrab for MoveSurfaceGrab {
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        _focus: Option<(wl_surface::WlSurface, Point<i32, Logical>)>,
        _serial: Serial,
        _time: u32,
    ) {
        let delta = location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;

        self.window_map.borrow_mut().set_location(
            &self.toplevel,
            (new_location.x as i32, new_location.y as i32).into(),
        );
    }

    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
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

bitflags::bitflags! {
    struct ResizeEdge: u32 {
        const NONE = 0;
        const TOP = 1;
        const BOTTOM = 2;
        const LEFT = 4;
        const TOP_LEFT = 5;
        const BOTTOM_LEFT = 6;
        const RIGHT = 8;
        const TOP_RIGHT = 9;
        const BOTTOM_RIGHT = 10;
    }
}

impl From<wl_shell_surface::Resize> for ResizeEdge {
    #[inline]
    fn from(x: wl_shell_surface::Resize) -> Self {
        Self::from_bits(x.bits()).unwrap()
    }
}

impl From<ResizeEdge> for wl_shell_surface::Resize {
    #[inline]
    fn from(x: ResizeEdge) -> Self {
        Self::from_bits(x.bits()).unwrap()
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    #[inline]
    fn from(x: xdg_toplevel::ResizeEdge) -> Self {
        Self::from_bits(x.to_raw()).unwrap()
    }
}

impl From<ResizeEdge> for xdg_toplevel::ResizeEdge {
    #[inline]
    fn from(x: ResizeEdge) -> Self {
        Self::from_raw(x.bits()).unwrap()
    }
}

struct ResizeSurfaceGrab {
    start_data: GrabStartData,
    toplevel: SurfaceKind,
    edges: ResizeEdge,
    initial_window_size: Size<i32, Logical>,
    last_window_size: Size<i32, Logical>,
}

impl PointerGrab for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        _focus: Option<(wl_surface::WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        // It is impossible to get `min_size` and `max_size` of dead toplevel, so we return early.
        if !self.toplevel.alive() | self.toplevel.get_surface().is_none() {
            handle.unset_grab(serial, time);
            return;
        }

        let (mut dx, mut dy) = (location - self.start_data.location).into();

        let mut new_window_width = self.initial_window_size.w;
        let mut new_window_height = self.initial_window_size.h;

        let left_right = ResizeEdge::LEFT | ResizeEdge::RIGHT;
        let top_bottom = ResizeEdge::TOP | ResizeEdge::BOTTOM;

        if self.edges.intersects(left_right) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                dx = -dx;
            }

            new_window_width = (self.initial_window_size.w as f64 + dx) as i32;
        }

        if self.edges.intersects(top_bottom) {
            if self.edges.intersects(ResizeEdge::TOP) {
                dy = -dy;
            }

            new_window_height = (self.initial_window_size.h as f64 + dy) as i32;
        }

        let (min_size, max_size) = with_states(self.toplevel.get_surface().unwrap(), |states| {
            let data = states.cached_state.current::<SurfaceCachedState>();
            (data.min_size, data.max_size)
        })
        .unwrap();

        let min_width = min_size.w.max(1);
        let min_height = min_size.h.max(1);
        let max_width = if max_size.w == 0 {
            i32::max_value()
        } else {
            max_size.w
        };
        let max_height = if max_size.h == 0 {
            i32::max_value()
        } else {
            max_size.h
        };

        new_window_width = new_window_width.max(min_width).min(max_width);
        new_window_height = new_window_height.max(min_height).min(max_height);

        self.last_window_size = (new_window_width, new_window_height).into();

        match &self.toplevel {
            SurfaceKind::Xdg(xdg) => {
                let ret = xdg.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                if ret.is_ok() {
                    xdg.send_configure();
                }
            }
            SurfaceKind::Wl(wl) => wl.send_configure(self.last_window_size, self.edges.into()),
            #[cfg(feature = "xwayland")]
            SurfaceKind::X11(_) => {
                // TODO: What to do here? Send the update via X11?
            }
        }
    }

    fn button(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        button: u32,
        state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        handle.button(button, state, serial, time);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(serial, time);

            // If toplevel is dead, we can't resize it, so we return early.
            if !self.toplevel.alive() | self.toplevel.get_surface().is_none() {
                return;
            }

            if let SurfaceKind::Xdg(xdg) = &self.toplevel {
                let ret = xdg.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                if ret.is_ok() {
                    xdg.send_configure();
                }

                with_states(self.toplevel.get_surface().unwrap(), |states| {
                    let mut data = states
                        .data_map
                        .get::<RefCell<SurfaceData>>()
                        .unwrap()
                        .borrow_mut();
                    if let ResizeState::Resizing(resize_data) = data.resize_state {
                        data.resize_state = ResizeState::WaitingForFinalAck(resize_data, serial);
                    } else {
                        panic!("invalid resize state: {:?}", data.resize_state);
                    }
                })
                .unwrap();
            } else {
                with_states(self.toplevel.get_surface().unwrap(), |states| {
                    let mut data = states
                        .data_map
                        .get::<RefCell<SurfaceData>>()
                        .unwrap()
                        .borrow_mut();
                    if let ResizeState::Resizing(resize_data) = data.resize_state {
                        data.resize_state = ResizeState::WaitingForCommit(resize_data);
                    } else {
                        panic!("invalid resize state: {:?}", data.resize_state);
                    }
                })
                .unwrap();
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

#[derive(Clone)]
pub struct ShellHandles {
    pub xdg_state: Arc<Mutex<XdgShellState>>,
    pub wl_state: Arc<Mutex<WlShellState>>,
    pub window_map: Rc<RefCell<WindowMap>>,
    pub output_map: Rc<RefCell<OutputMap>>,
}

fn fullscreen_output_geometry(
    wl_surface: &wl_surface::WlSurface,
    wl_output: Option<&wl_output::WlOutput>,
    window_map: &WindowMap,
    output_map: &OutputMap,
) -> Option<Rectangle<i32, Logical>> {
    // First test if a specific output has been requested
    // if the requested output is not found ignore the request
    if let Some(wl_output) = wl_output {
        return output_map.find(&wl_output, |_, geometry| geometry).ok();
    }

    // There is no output preference, try to find the output
    // where the window is currently active
    let window_location = window_map
        .find(wl_surface)
        .and_then(|kind| window_map.location(&kind));

    if let Some(location) = window_location {
        let window_output = output_map.find_by_position(location, |_, geometry| geometry).ok();

        if let Some(result) = window_output {
            return Some(result);
        }
    }

    // Fallback to primary output
    output_map.with_primary(|_, geometry| geometry).ok()
}

pub fn init_shell<BackendData: 'static>(display: Rc<RefCell<Display>>, log: ::slog::Logger) -> ShellHandles {
    // Create the compositor
    compositor_init(
        &mut *display.borrow_mut(),
        move |surface, mut ddata| {
            let anvil_state = ddata.get::<AnvilState<BackendData>>().unwrap();
            let window_map = anvil_state.window_map.as_ref();
            surface_commit(&surface, &*window_map)
        },
        log.clone(),
    );

    // Init a window map, to track the location of our windows
    let window_map = Rc::new(RefCell::new(WindowMap::new()));
    let output_map = Rc::new(RefCell::new(OutputMap::new(
        display.clone(),
        window_map.clone(),
        log.clone(),
    )));

    // init the xdg_shell
    let xdg_window_map = window_map.clone();
    let xdg_output_map = output_map.clone();
    let (xdg_shell_state, _, _) = xdg_shell_init(
        &mut *display.borrow_mut(),
        move |shell_event, _dispatch_data| match shell_event {
            XdgRequest::NewToplevel { surface } => {
                // place the window at a random location on the primary output
                // or if there is not output in a [0;800]x[0;800] square
                use rand::distributions::{Distribution, Uniform};

                let output_geometry = xdg_output_map
                    .borrow()
                    .with_primary(|_, geometry| geometry)
                    .ok()
                    .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (800, 800)));
                let max_x = output_geometry.loc.x + (((output_geometry.size.w as f32) / 3.0) * 2.0) as i32;
                let max_y = output_geometry.loc.y + (((output_geometry.size.h as f32) / 3.0) * 2.0) as i32;
                let x_range = Uniform::new(output_geometry.loc.x, max_x);
                let y_range = Uniform::new(output_geometry.loc.y, max_y);
                let mut rng = rand::thread_rng();
                let x = x_range.sample(&mut rng);
                let y = y_range.sample(&mut rng);
                // Do not send a configure here, the initial configure
                // of a xdg_surface has to be sent during the commit if
                // the surface is not already configured
                xdg_window_map
                    .borrow_mut()
                    .insert(SurfaceKind::Xdg(surface), (x, y).into());
            }
            XdgRequest::NewPopup { surface } => {
                // Do not send a configure here, the initial configure
                // of a xdg_surface has to be sent during the commit if
                // the surface is not already configured
                xdg_window_map.borrow_mut().insert_popup(PopupKind::Xdg(surface));
            }
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
                let initial_window_size = geometry.size;

                with_states(surface.get_surface().unwrap(), move |states| {
                    states
                        .data_map
                        .get::<RefCell<SurfaceData>>()
                        .unwrap()
                        .borrow_mut()
                        .resize_state = ResizeState::Resizing(ResizeData {
                        edges: edges.into(),
                        initial_window_location,
                        initial_window_size,
                    });
                })
                .unwrap();

                let grab = ResizeSurfaceGrab {
                    start_data,
                    toplevel,
                    edges: edges.into(),
                    initial_window_size,
                    last_window_size: initial_window_size,
                };

                pointer.set_grab(grab, serial);
            }
            XdgRequest::AckConfigure {
                surface,
                configure: Configure::Toplevel(configure),
                ..
            } => {
                let waiting_for_serial = with_states(&surface, |states| {
                    if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>() {
                        if let ResizeState::WaitingForFinalAck(_, serial) = data.borrow().resize_state {
                            return Some(serial);
                        }
                    }

                    None
                })
                .unwrap();

                if let Some(serial) = waiting_for_serial {
                    // When the resize grab is released the surface
                    // resize state will be set to WaitingForFinalAck
                    // and the client will receive a configure request
                    // without the resize state to inform the client
                    // resizing has finished. Here we will wait for
                    // the client to acknowledge the end of the
                    // resizing. To check if the surface was resizing
                    // before sending the configure we need to use
                    // the current state as the received acknowledge
                    // will no longer have the resize state set
                    let is_resizing = with_states(&surface, |states| {
                        states
                            .data_map
                            .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .current
                            .states
                            .contains(xdg_toplevel::State::Resizing)
                    })
                    .unwrap();

                    if configure.serial >= serial && is_resizing {
                        with_states(&surface, |states| {
                            let mut data = states
                                .data_map
                                .get::<RefCell<SurfaceData>>()
                                .unwrap()
                                .borrow_mut();
                            if let ResizeState::WaitingForFinalAck(resize_data, _) = data.resize_state {
                                data.resize_state = ResizeState::WaitingForCommit(resize_data);
                            } else {
                                unreachable!()
                            }
                        })
                        .unwrap();
                    }
                }
            }
            XdgRequest::Fullscreen { surface, output, .. } => {
                // NOTE: This is only one part of the solution. We can set the
                // location and configure size here, but the surface should be rendered fullscreen
                // independently from its buffer size
                let wl_surface = if let Some(surface) = surface.get_surface() {
                    surface
                } else {
                    // If there is no underlying surface just ignore the request
                    return;
                };

                let output_geometry = fullscreen_output_geometry(
                    wl_surface,
                    output.as_ref(),
                    &xdg_window_map.borrow(),
                    &xdg_output_map.borrow(),
                );

                if let Some(geometry) = output_geometry {
                    if let Some(surface) = surface.get_surface() {
                        let mut xdg_window_map = xdg_window_map.borrow_mut();
                        if let Some(kind) = xdg_window_map.find(surface) {
                            xdg_window_map.set_location(&kind, geometry.loc);
                        }
                    }

                    let ret = surface.with_pending_state(|state| {
                        state.states.set(xdg_toplevel::State::Fullscreen);
                        state.size = Some(geometry.size);
                        state.fullscreen_output = output;
                    });
                    if ret.is_ok() {
                        surface.send_configure();
                    }
                }
            }
            XdgRequest::UnFullscreen { surface } => {
                let ret = surface.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.size = None;
                    state.fullscreen_output = None;
                });
                if ret.is_ok() {
                    surface.send_configure();
                }
            }
            XdgRequest::Maximize { surface } => {
                // NOTE: This should use layer-shell when it is implemented to
                // get the correct maximum size
                let output_geometry = {
                    let xdg_window_map = xdg_window_map.borrow();
                    surface
                        .get_surface()
                        .and_then(|s| xdg_window_map.find(s))
                        .and_then(|k| xdg_window_map.location(&k))
                        .and_then(|position| {
                            xdg_output_map
                                .borrow()
                                .find_by_position(position, |_, geometry| geometry)
                                .ok()
                        })
                };

                if let Some(geometry) = output_geometry {
                    if let Some(surface) = surface.get_surface() {
                        let mut xdg_window_map = xdg_window_map.borrow_mut();
                        if let Some(kind) = xdg_window_map.find(surface) {
                            xdg_window_map.set_location(&kind, geometry.loc);
                        }
                    }
                    let ret = surface.with_pending_state(|state| {
                        state.states.set(xdg_toplevel::State::Maximized);
                        state.size = Some(geometry.size);
                    });
                    if ret.is_ok() {
                        surface.send_configure();
                    }
                }
            }
            XdgRequest::UnMaximize { surface } => {
                let ret = surface.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.size = None;
                });
                if ret.is_ok() {
                    surface.send_configure();
                }
            }
            _ => (),
        },
        log.clone(),
    );

    // init the wl_shell
    let shell_window_map = window_map.clone();
    let shell_output_map = output_map.clone();
    let (wl_shell_state, _) = wl_shell_init(
        &mut *display.borrow_mut(),
        move |req: ShellRequest, _dispatch_data| {
            match req {
                ShellRequest::SetKind {
                    surface,
                    kind: ShellSurfaceKind::Toplevel,
                } => {
                    // place the window at a random location on the primary output
                    // or if there is not output in a [0;800]x[0;800] square
                    use rand::distributions::{Distribution, Uniform};

                    let output_geometry = shell_output_map
                        .borrow()
                        .with_primary(|_, geometry| geometry)
                        .ok()
                        .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (800, 800)));
                    let max_x =
                        output_geometry.loc.x + (((output_geometry.size.w as f32) / 3.0) * 2.0) as i32;
                    let max_y =
                        output_geometry.loc.y + (((output_geometry.size.h as f32) / 3.0) * 2.0) as i32;
                    let x_range = Uniform::new(output_geometry.loc.x, max_x);
                    let y_range = Uniform::new(output_geometry.loc.y, max_y);
                    let mut rng = rand::thread_rng();
                    let x = x_range.sample(&mut rng);
                    let y = y_range.sample(&mut rng);
                    shell_window_map
                        .borrow_mut()
                        .insert(SurfaceKind::Wl(surface), (x, y).into());
                }
                ShellRequest::SetKind {
                    surface,
                    kind: ShellSurfaceKind::Fullscreen { output, .. },
                } => {
                    // NOTE: This is only one part of the solution. We can set the
                    // location and configure size here, but the surface should be rendered fullscreen
                    // independently from its buffer size
                    let wl_surface = if let Some(surface) = surface.get_surface() {
                        surface
                    } else {
                        // If there is no underlying surface just ignore the request
                        return;
                    };

                    let output_geometry = fullscreen_output_geometry(
                        wl_surface,
                        output.as_ref(),
                        &shell_window_map.borrow(),
                        &shell_output_map.borrow(),
                    );

                    if let Some(geometry) = output_geometry {
                        shell_window_map
                            .borrow_mut()
                            .insert(SurfaceKind::Wl(surface), geometry.loc);
                    }
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
                    let initial_window_size = geometry.size;

                    with_states(surface.get_surface().unwrap(), move |states| {
                        states
                            .data_map
                            .get::<RefCell<SurfaceData>>()
                            .unwrap()
                            .borrow_mut()
                            .resize_state = ResizeState::Resizing(ResizeData {
                            edges: edges.into(),
                            initial_window_location,
                            initial_window_size,
                        });
                    })
                    .unwrap();

                    let grab = ResizeSurfaceGrab {
                        start_data,
                        toplevel,
                        edges: edges.into(),
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

    ShellHandles {
        xdg_state: xdg_shell_state,
        wl_state: wl_shell_state,
        window_map,
        output_map,
    }
}

/// Information about the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    /// The edges the surface is being resized with.
    edges: ResizeEdge,
    /// The initial window location.
    initial_window_location: Point<i32, Logical>,
    /// The initial window size (geometry width and height).
    initial_window_size: Size<i32, Logical>,
}

/// State of the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ResizeState {
    /// The surface is not being resized.
    NotResizing,
    /// The surface is currently being resized.
    Resizing(ResizeData),
    /// The resize has finished, and the surface needs to ack the final configure.
    WaitingForFinalAck(ResizeData, Serial),
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
    pub texture: Option<Box<dyn std::any::Any + 'static>>,
    pub geometry: Option<Rectangle<i32, Logical>>,
    pub resize_state: ResizeState,
    pub buffer_dimensions: Option<Size<i32, Physical>>,
    pub buffer_scale: i32,
}

impl SurfaceData {
    pub fn update_buffer(&mut self, attrs: &mut SurfaceAttributes) {
        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer { buffer, .. }) => {
                // new contents
                self.buffer_dimensions = buffer_dimensions(&buffer);
                self.buffer_scale = attrs.buffer_scale;
                if let Some(old_buffer) = std::mem::replace(&mut self.buffer, Some(buffer)) {
                    old_buffer.release();
                }
                self.texture = None;
            }
            Some(BufferAssignment::Removed) => {
                // remove the contents
                self.buffer = None;
                self.buffer_dimensions = None;
                self.texture = None;
            }
            None => {}
        }
    }

    /// Returns the size of the surface.
    pub fn size(&self) -> Option<Size<i32, Logical>> {
        self.buffer_dimensions
            .map(|dims| dims.to_logical(self.buffer_scale))
    }

    /// Checks if the surface's input region contains the point.
    pub fn contains_point(&self, attrs: &SurfaceAttributes, point: Point<f64, Logical>) -> bool {
        let size = match self.size() {
            None => return false, // If the surface has no size, it can't have an input region.
            Some(size) => size,
        };

        let rect = Rectangle {
            loc: (0, 0).into(),
            size,
        }
        .to_f64();

        // The input region is always within the surface itself, so if the surface itself doesn't contain the
        // point we can return false.
        if !rect.contains(point) {
            return false;
        }

        // If there's no input region, we're done.
        if attrs.input_region.is_none() {
            return true;
        }

        attrs
            .input_region
            .as_ref()
            .unwrap()
            .contains(point.to_i32_floor())
    }

    /// Send the frame callback if it had been requested
    pub fn send_frame(attrs: &mut SurfaceAttributes, time: u32) {
        for callback in attrs.frame_callbacks.drain(..) {
            callback.done(time);
        }
    }
}

fn surface_commit(surface: &wl_surface::WlSurface, window_map: &RefCell<WindowMap>) {
    #[cfg(feature = "xwayland")]
    super::xwayland::commit_hook(surface);

    let mut window_map = window_map.borrow_mut();

    if !is_sync_subsurface(surface) {
        // Update the buffer of all child surfaces
        with_surface_tree_upward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_, states, _| {
                states
                    .data_map
                    .insert_if_missing(|| RefCell::new(SurfaceData::default()));
                let mut data = states
                    .data_map
                    .get::<RefCell<SurfaceData>>()
                    .unwrap()
                    .borrow_mut();
                data.update_buffer(&mut *states.cached_state.current::<SurfaceAttributes>());
            },
            |_, _, _| true,
        );
    }

    if let Some(toplevel) = window_map.find(surface) {
        // send the initial configure if relevant
        if let SurfaceKind::Xdg(ref toplevel) = toplevel {
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .initial_configure_sent
            })
            .unwrap();
            if !initial_configure_sent {
                toplevel.send_configure();
            }
        }

        window_map.refresh_toplevel(&toplevel);

        let geometry = window_map.geometry(&toplevel).unwrap();
        let new_location = with_states(surface, |states| {
            let mut data = states
                .data_map
                .get::<RefCell<SurfaceData>>()
                .unwrap()
                .borrow_mut();

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

                    if edges.intersects(ResizeEdge::TOP_LEFT) {
                        let mut location = window_map.location(&toplevel).unwrap();

                        if edges.intersects(ResizeEdge::LEFT) {
                            location.x =
                                initial_window_location.x + (initial_window_size.w - geometry.size.w);
                        }
                        if edges.intersects(ResizeEdge::TOP) {
                            location.y =
                                initial_window_location.y + (initial_window_size.h - geometry.size.h);
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
        })
        .unwrap();

        if let Some(location) = new_location {
            window_map.set_location(&toplevel, location);
        }
    }

    if let Some(popup) = window_map.find_popup(surface) {
        let PopupKind::Xdg(ref popup) = popup;
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        })
        .unwrap();
        if !initial_configure_sent {
            // TODO: properly recompute the geometry with the whole of positioner state
            popup.send_configure();
        }
    }
}
