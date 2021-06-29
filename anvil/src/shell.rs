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
            protocol::{wl_buffer, wl_pointer::ButtonState, wl_shell_surface, wl_surface},
            Display,
        },
    },
    utils::Rectangle,
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
    state::AnvilState,
    window_map::{Kind as SurfaceKind, PopupKind, WindowMap},
};

struct MoveSurfaceGrab {
    start_data: GrabStartData,
    window_map: Rc<RefCell<WindowMap>>,
    toplevel: SurfaceKind,
    initial_window_location: (i32, i32),
}

impl PointerGrab for MoveSurfaceGrab {
    fn motion(
        &mut self,
        _handle: &mut PointerInnerHandle<'_>,
        location: (f64, f64),
        _focus: Option<(wl_surface::WlSurface, (f64, f64))>,
        _serial: Serial,
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
    initial_window_size: (i32, i32),
    last_window_size: (i32, i32),
}

impl PointerGrab for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: (f64, f64),
        _focus: Option<(wl_surface::WlSurface, (f64, f64))>,
        serial: Serial,
        time: u32,
    ) {
        // It is impossible to get `min_size` and `max_size` of dead toplevel, so we return early.
        if !self.toplevel.alive() | self.toplevel.get_surface().is_none() {
            handle.unset_grab(serial, time);
            return;
        }

        let mut dx = location.0 - self.start_data.location.0;
        let mut dy = location.1 - self.start_data.location.1;

        let mut new_window_width = self.initial_window_size.0;
        let mut new_window_height = self.initial_window_size.1;

        let left_right = ResizeEdge::LEFT | ResizeEdge::RIGHT;
        let top_bottom = ResizeEdge::TOP | ResizeEdge::BOTTOM;

        if self.edges.intersects(left_right) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                dx = -dx;
            }

            new_window_width = (self.initial_window_size.0 as f64 + dx) as i32;
        }

        if self.edges.intersects(top_bottom) {
            if self.edges.intersects(ResizeEdge::TOP) {
                dy = -dy;
            }

            new_window_height = (self.initial_window_size.1 as f64 + dy) as i32;
        }

        let (min_size, max_size) = with_states(self.toplevel.get_surface().unwrap(), |states| {
            let data = states.cached_state.current::<SurfaceCachedState>();
            (data.min_size, data.max_size)
        })
        .unwrap();

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
            SurfaceKind::Xdg(xdg) => {
                let ret = xdg.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                if ret.is_ok() {
                    xdg.send_configure();
                }
            }
            SurfaceKind::Wl(wl) => wl.send_configure(
                (self.last_window_size.0 as u32, self.last_window_size.1 as u32),
                self.edges.into(),
            ),
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
}

pub fn init_shell<BackendData: 'static>(display: &mut Display, log: ::slog::Logger) -> ShellHandles {
    // Create the compositor
    compositor_init(
        display,
        move |surface, mut ddata| {
            let anvil_state = ddata.get::<AnvilState<BackendData>>().unwrap();
            let window_map = anvil_state.window_map.as_ref();
            surface_commit(&surface, &*window_map)
        },
        log.clone(),
    );

    // Init a window map, to track the location of our windows
    let window_map = Rc::new(RefCell::new(WindowMap::new()));

    // init the xdg_shell
    let xdg_window_map = window_map.clone();
    let (xdg_shell_state, _, _) = xdg_shell_init(
        display,
        move |shell_event| match shell_event {
            XdgRequest::NewToplevel { surface } => {
                // place the window at a random location in the [0;800]x[0;800] square
                use rand::distributions::{Distribution, Uniform};
                let range = Uniform::new(0, 800);
                let mut rng = rand::thread_rng();
                let x = range.sample(&mut rng);
                let y = range.sample(&mut rng);
                // Do not send a configure here, the initial configure
                // of a xdg_surface has to be sent during the commit if
                // the surface is not already configured
                xdg_window_map
                    .borrow_mut()
                    .insert(SurfaceKind::Xdg(surface), (x, y));
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
                let initial_window_size = (geometry.width, geometry.height);

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
                    if configure.serial > serial {
                        // TODO: huh, we have missed the serial somehow.
                        // this should not happen, but it may be better to handle
                        // this case anyway
                    }

                    if serial == configure.serial
                        && configure.state.states.contains(xdg_toplevel::State::Resizing)
                    {
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
                let ret = surface.with_pending_state(|state| {
                    // TODO: Use size of current output the window is on and set position to (0,0)
                    state.states.set(xdg_toplevel::State::Fullscreen);
                    state.size = Some((800, 600));
                    // TODO: If the provided output is None, use the output where
                    // the toplevel is currently shown
                    state.fullscreen_output = output;
                });
                if ret.is_ok() {
                    surface.send_configure();
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
                let ret = surface.with_pending_state(|state| {
                    // TODO: Use size of current output the window is on and set position to (0,0)
                    state.states.set(xdg_toplevel::State::Maximized);
                    state.size = Some((800, 600));
                });
                if ret.is_ok() {
                    surface.send_configure();
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
    let (wl_shell_state, _) = wl_shell_init(
        display,
        move |req: ShellRequest| {
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
    }
}

/// Information about the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    /// The edges the surface is being resized with.
    edges: ResizeEdge,
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
    pub geometry: Option<Rectangle>,
    pub resize_state: ResizeState,
    pub dimensions: Option<(i32, i32)>,
}

impl SurfaceData {
    pub fn update_buffer(&mut self, attrs: &mut SurfaceAttributes) {
        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer { buffer, .. }) => {
                // new contents
                self.dimensions = buffer_dimensions(&buffer);
                if let Some(old_buffer) = std::mem::replace(&mut self.buffer, Some(buffer)) {
                    old_buffer.release();
                }
                self.texture = None;
            }
            Some(BufferAssignment::Removed) => {
                // remove the contents
                self.buffer = None;
                self.dimensions = None;
                self.texture = None;
            }
            None => {}
        }
    }

    /// Returns the size of the surface.
    pub fn size(&self) -> Option<(i32, i32)> {
        self.dimensions
    }

    /// Checks if the surface's input region contains the point.
    pub fn contains_point(&self, attrs: &SurfaceAttributes, point: (f64, f64)) -> bool {
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
        if attrs.input_region.is_none() {
            return true;
        }

        attrs.input_region.as_ref().unwrap().contains(point)
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
        // Get the geometry outside since it uses the token, and so would block inside.
        let Rectangle { width, height, .. } = window_map.geometry(&toplevel).unwrap();

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
                            location.0 = initial_window_location.0 + (initial_window_size.0 - width);
                        }
                        if edges.intersects(ResizeEdge::TOP) {
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
