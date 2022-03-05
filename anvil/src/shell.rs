use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    desktop::{
        layer_map_for_output, Kind as SurfaceKind, LayerSurface, PopupKeyboardGrab, PopupKind, PopupManager,
        PopupPointerGrab, PopupUngrabStrategy, Space, Window,
    },
    reexports::{
        wayland_protocols::xdg_shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_output, wl_pointer::ButtonState, wl_shell_surface, wl_surface},
            Display,
        },
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::{
        compositor::{compositor_init, with_states, with_surface_tree_upward, TraversalAction},
        output::Output,
        seat::{AxisFrame, PointerGrab, PointerGrabStartData, PointerInnerHandle, Seat},
        shell::{
            wlr_layer::{LayerShellRequest, LayerShellState, LayerSurfaceAttributes},
            xdg::{
                xdg_shell_init, Configure, ShellState as XdgShellState, SurfaceCachedState,
                XdgPopupSurfaceRoleAttributes, XdgRequest, XdgToplevelSurfaceRoleAttributes,
            },
        },
        Serial,
    },
};

use crate::state::{AnvilState, Backend};

struct MoveSurfaceGrab {
    start_data: PointerGrabStartData,
    space: Rc<RefCell<Space>>,
    window: Window,
    initial_window_location: Point<i32, Logical>,
}

impl PointerGrab for MoveSurfaceGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle<'_>,
        location: Point<f64, Logical>,
        _focus: Option<(wl_surface::WlSurface, Point<i32, Logical>)>,
        serial: Serial,
        time: u32,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(location, None, serial, time);

        let delta = location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;

        self.space
            .borrow_mut()
            .map_window(&self.window, new_location.to_i32_round(), true);
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

    fn start_data(&self) -> &PointerGrabStartData {
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
    start_data: PointerGrabStartData,
    window: Window,
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
        if !self.window.toplevel().alive() | self.window.toplevel().get_surface().is_none() {
            handle.unset_grab(serial, time);
            return;
        }

        // While the grab is active, no client has pointer focus
        handle.motion(location, None, serial, time);

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

        let (min_size, max_size) = with_states(self.window.toplevel().get_surface().unwrap(), |states| {
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

        match &self.window.toplevel() {
            SurfaceKind::Xdg(xdg) => {
                let ret = xdg.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                if ret.is_ok() {
                    xdg.send_configure();
                }
            }
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
            if !self.window.toplevel().alive() | self.window.toplevel().get_surface().is_none() {
                return;
            }

            #[cfg_attr(not(feature = "xwayland"), allow(irrefutable_let_patterns))]
            if let SurfaceKind::Xdg(xdg) = &self.window.toplevel() {
                let ret = xdg.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                if ret.is_ok() {
                    xdg.send_configure();
                }

                with_states(self.window.toplevel().get_surface().unwrap(), |states| {
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
                with_states(self.window.toplevel().get_surface().unwrap(), |states| {
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

    fn start_data(&self) -> &PointerGrabStartData {
        &self.start_data
    }
}

#[derive(Debug, Clone)]
pub struct ShellHandles {
    pub xdg_state: Arc<Mutex<XdgShellState>>,
    //pub wl_state: Arc<Mutex<WlShellState>>,
    pub layer_state: Arc<Mutex<LayerShellState>>,
}

fn fullscreen_output_geometry(
    wl_surface: &wl_surface::WlSurface,
    wl_output: Option<&wl_output::WlOutput>,
    space: &mut Space,
) -> Option<Rectangle<i32, Logical>> {
    // First test if a specific output has been requested
    // if the requested output is not found ignore the request
    wl_output
        .and_then(Output::from_resource)
        .or_else(|| {
            let w = space.window_for_surface(wl_surface).cloned();
            w.and_then(|w| space.outputs_for_window(&w).get(0).cloned())
        })
        .and_then(|o| space.output_geometry(&o))
}

#[derive(Default)]
pub struct FullscreenSurface(RefCell<Option<Window>>);

impl FullscreenSurface {
    pub fn set(&self, window: Window) {
        *self.0.borrow_mut() = Some(window);
    }

    pub fn get(&self) -> Option<Window> {
        self.0.borrow().clone()
    }

    pub fn clear(&self) -> Option<Window> {
        self.0.borrow_mut().take()
    }
}

pub fn init_shell<BackendData: Backend + 'static>(
    display: Rc<RefCell<Display>>,
    log: ::slog::Logger,
) -> ShellHandles {
    // Create the compositor
    compositor_init(
        &mut *display.borrow_mut(),
        move |surface, mut ddata| {
            on_commit_buffer_handler(&surface);
            let anvil_state = ddata.get::<AnvilState<BackendData>>().unwrap();
            let mut popups = anvil_state.popups.borrow_mut();
            let space = anvil_state.space.as_ref();
            space.borrow_mut().commit(&surface);
            surface_commit(&surface, &*space, &mut *popups)
        },
        log.clone(),
    );

    let log_ref = log.clone();
    // init the xdg_shell
    let (xdg_shell_state, _) = xdg_shell_init(
        &mut *display.borrow_mut(),
        move |shell_event, mut ddata| {
            let state = ddata.get::<AnvilState<BackendData>>().unwrap();

            match shell_event {
                XdgRequest::NewToplevel { surface } => {
                    // Do not send a configure here, the initial configure
                    // of a xdg_surface has to be sent during the commit if
                    // the surface is not already configured
                    let window = Window::new(SurfaceKind::Xdg(surface));
                    place_new_window(&mut *state.space.borrow_mut(), &window, true);
                }

                XdgRequest::NewPopup { surface, positioner } => {
                    // Do not send a configure here, the initial configure
                    // of a xdg_surface has to be sent during the commit if
                    // the surface is not already configured

                    // TODO: properly recompute the geometry with the whole of positioner state
                    surface
                        .with_pending_state(|state| {
                            // NOTE: This is not really necessary as the default geometry
                            // is already set the same way, but for demonstrating how
                            // to set the initial popup geometry this code is left as
                            // an example
                            state.geometry = positioner.get_geometry();
                        })
                        .unwrap();
                    if let Err(err) = state.popups.borrow_mut().track_popup(PopupKind::from(surface)) {
                        slog::warn!(log_ref, "Failed to track popup: {}", err);
                    }
                }

                XdgRequest::RePosition {
                    surface,
                    positioner,
                    token,
                } => {
                    let result = surface.with_pending_state(|state| {
                        // NOTE: This is again a simplification, a proper compositor would
                        // calculate the geometry of the popup here. For simplicity we just
                        // use the default implementation here that does not take the
                        // window position and output constraints into account.
                        let geometry = positioner.get_geometry();
                        state.geometry = geometry;
                        state.positioner = positioner;
                    });

                    if result.is_ok() {
                        surface.send_repositioned(token);
                    }
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
                            .same_client_as(surface.wl_surface().unwrap().as_ref())
                    {
                        return;
                    }

                    let space = state.space.clone();
                    let window = space
                        .borrow_mut()
                        .window_for_surface(surface.wl_surface().unwrap())
                        .unwrap()
                        .clone();
                    let mut initial_window_location = space.borrow().window_location(&window).unwrap();

                    // If surface is maximized then unmaximize it
                    if let Some(current_state) = surface.current_state() {
                        if current_state.states.contains(xdg_toplevel::State::Maximized) {
                            let fs_changed = surface.with_pending_state(|state| {
                                state.states.unset(xdg_toplevel::State::Maximized);
                                state.size = None;
                            });

                            if fs_changed.is_ok() {
                                surface.send_configure();

                                // NOTE: In real compositor mouse location should be mapped to a new window size
                                // For example, you could:
                                // 1) transform mouse pointer position from compositor space to window space (location relative)
                                // 2) divide the x coordinate by width of the window to get the percentage
                                //   - 0.0 would be on the far left of the window
                                //   - 0.5 would be in middle of the window
                                //   - 1.0 would be on the far right of the window
                                // 3) multiply the percentage by new window width
                                // 4) by doing that, drag will look a lot more natural
                                //
                                // but for anvil needs setting location to pointer location is fine
                                let pos = pointer.current_location();
                                initial_window_location = (pos.x as i32, pos.y as i32).into();
                            }
                        }
                    }

                    let grab = MoveSurfaceGrab {
                        start_data,
                        space,
                        window,
                        initial_window_location,
                    };

                    pointer.set_grab(grab, serial, 0);
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
                            .same_client_as(surface.wl_surface().unwrap().as_ref())
                    {
                        return;
                    }

                    let space = state.space.clone();
                    let window = space
                        .borrow_mut()
                        .window_for_surface(surface.wl_surface().unwrap())
                        .unwrap()
                        .clone();
                    let geometry = window.geometry();
                    let loc = space.borrow().window_location(&window).unwrap();
                    let (initial_window_location, initial_window_size) = (loc, geometry.size);

                    with_states(surface.wl_surface().unwrap(), move |states| {
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
                        window,
                        edges: edges.into(),
                        initial_window_size,
                        last_window_size: initial_window_size,
                    };

                    pointer.set_grab(grab, serial, 0);
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

                XdgRequest::Fullscreen {
                    surface,
                    output: mut wl_output,
                    ..
                } => {
                    // NOTE: This is only one part of the solution. We can set the
                    // location and configure size here, but the surface should be rendered fullscreen
                    // independently from its buffer size
                    let wl_surface = if let Some(surface) = surface.wl_surface() {
                        surface
                    } else {
                        // If there is no underlying surface just ignore the request
                        return;
                    };

                    let output_geometry = fullscreen_output_geometry(
                        wl_surface,
                        wl_output.as_ref(),
                        &mut *state.space.borrow_mut(),
                    );

                    if let Some(geometry) = output_geometry {
                        let space = state.space.borrow_mut();
                        let output = wl_output
                            .as_ref()
                            .and_then(Output::from_resource)
                            .unwrap_or_else(|| space.outputs().next().unwrap().clone());
                        output.with_client_outputs(wl_surface.as_ref().client().unwrap(), |output| {
                            wl_output = Some(output.clone());
                        });

                        let ret = surface.with_pending_state(|state| {
                            state.states.set(xdg_toplevel::State::Fullscreen);
                            state.size = Some(geometry.size);
                            state.fullscreen_output = wl_output;
                        });

                        if ret.is_ok() {
                            let window = space.window_for_surface(wl_surface).unwrap();
                            window.configure();
                            output.user_data().insert_if_missing(FullscreenSurface::default);
                            output
                                .user_data()
                                .get::<FullscreenSurface>()
                                .unwrap()
                                .set(window.clone());
                            slog::trace!(log_ref, "Fullscreening: {:?}", window);
                        }
                    }
                }

                XdgRequest::UnFullscreen { surface } => {
                    let ret = surface.with_pending_state(|state| {
                        state.states.unset(xdg_toplevel::State::Fullscreen);
                        state.size = None;
                        state.fullscreen_output.take()
                    });
                    if let Ok(output) = ret {
                        if let Some(output) = output {
                            let output = Output::from_resource(&output).unwrap();
                            if let Some(fullscreen) = output.user_data().get::<FullscreenSurface>() {
                                slog::trace!(log_ref, "Unfullscreening: {:?}", fullscreen.get());
                                fullscreen.clear();
                                state.backend_data.reset_buffers(&output);
                            }
                        }

                        surface.send_configure();
                    }
                }

                XdgRequest::Maximize { surface } => {
                    // NOTE: This should use layer-shell when it is implemented to
                    // get the correct maximum size
                    let mut space = state.space.borrow_mut();
                    let window = space
                        .window_for_surface(surface.wl_surface().unwrap())
                        .unwrap()
                        .clone();
                    let output = &space.outputs_for_window(&window)[0];
                    let geometry = space.output_geometry(output).unwrap();

                    space.map_window(&window, geometry.loc, true);
                    let ret = surface.with_pending_state(|state| {
                        state.states.set(xdg_toplevel::State::Maximized);
                        state.size = Some(geometry.size);
                    });

                    if ret.is_ok() {
                        window.configure();
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
                XdgRequest::Grab {
                    serial,
                    surface,
                    seat,
                } => {
                    let seat = Seat::from_resource(&seat).unwrap();
                    let ret = state
                        .popups
                        .borrow_mut()
                        .grab_popup(surface.into(), &seat, serial);

                    if let Ok(mut grab) = ret {
                        if let Some(keyboard) = seat.get_keyboard() {
                            if keyboard.is_grabbed()
                                && !(keyboard.has_grab(serial)
                                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                            {
                                grab.ungrab(PopupUngrabStrategy::All);
                                return;
                            }
                            keyboard.set_focus(grab.current_grab().as_ref(), serial);
                            keyboard.set_grab(PopupKeyboardGrab::new(&grab), serial);
                        }
                        if let Some(pointer) = seat.get_pointer() {
                            if pointer.is_grabbed()
                                && !(pointer.has_grab(serial)
                                    || pointer
                                        .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                            {
                                grab.ungrab(PopupUngrabStrategy::All);
                                return;
                            }
                            pointer.set_grab(PopupPointerGrab::new(&grab), serial, 0);
                        }
                    }
                }
                _ => (),
            }
        },
        log.clone(),
    );

    let (layer_shell_state, _) = smithay::wayland::shell::wlr_layer::wlr_layer_shell_init(
        &mut *display.borrow_mut(),
        move |event, mut ddata| match event {
            LayerShellRequest::NewLayerSurface {
                surface,
                output: wl_output,
                namespace,
                ..
            } => {
                let state = ddata.get::<AnvilState<BackendData>>().unwrap();
                let space = state.space.borrow();
                let output = wl_output
                    .as_ref()
                    .and_then(Output::from_resource)
                    .unwrap_or_else(|| space.outputs().next().unwrap().clone());

                let mut map = layer_map_for_output(&output);
                map.map_layer(&LayerSurface::new(surface, namespace)).unwrap();
            }

            LayerShellRequest::AckConfigure { .. } => {}
        },
        log.clone(),
    );

    ShellHandles {
        xdg_state: xdg_shell_state,
        layer_state: layer_shell_state,
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
    pub geometry: Option<Rectangle<i32, Logical>>,
    pub resize_state: ResizeState,
}

fn surface_commit(surface: &wl_surface::WlSurface, space: &RefCell<Space>, popups: &mut PopupManager) {
    #[cfg(feature = "xwayland")]
    super::xwayland::commit_hook(surface);

    popups.commit(surface);
    let mut space = space.borrow_mut();

    with_surface_tree_upward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| {
            states
                .data_map
                .insert_if_missing(|| RefCell::new(SurfaceData::default()));
        },
        |_, _, _| true,
    );

    if let Some(window) = space.window_for_surface(surface).cloned() {
        // send the initial configure if relevant
        #[cfg_attr(not(feature = "xwayland"), allow(irrefutable_let_patterns))]
        if let SurfaceKind::Xdg(ref toplevel) = window.toplevel() {
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

        let geometry = window.geometry();
        let window_loc = space.window_location(&window).unwrap();
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
                        let mut location = window_loc;

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
            space.map_window(&window, location, true);
        }

        return;
    }

    if let Some(popup) = popups.find_popup(surface) {
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
            // NOTE: This should never fail as the initial configure is always
            // allowed.
            popup.send_configure().expect("initial configure failed");
        }

        return;
    };

    if let Some(output) = space.outputs().find(|o| {
        let map = layer_map_for_output(o);
        map.layer_for_surface(surface).is_some()
    }) {
        let mut map = layer_map_for_output(output);
        let layer = map.layer_for_surface(surface).unwrap();

        // send the initial configure if relevant
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        })
        .unwrap();
        if !initial_configure_sent {
            layer.layer_surface().send_configure();
        }

        map.arrange();
    };
}

fn place_new_window(space: &mut Space, window: &Window, activate: bool) {
    // place the window at a random location on the primary output
    // or if there is not output in a [0;800]x[0;800] square
    use rand::distributions::{Distribution, Uniform};

    let output = space.outputs().next().cloned();
    let output_geometry = output
        .and_then(|o| {
            let geo = space.output_geometry(&o)?;
            let map = layer_map_for_output(&o);
            let zone = map.non_exclusive_zone();
            Some(Rectangle::from_loc_and_size(geo.loc + zone.loc, zone.size))
        })
        .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (800, 800)));

    let max_x = output_geometry.loc.x + (((output_geometry.size.w as f32) / 3.0) * 2.0) as i32;
    let max_y = output_geometry.loc.y + (((output_geometry.size.h as f32) / 3.0) * 2.0) as i32;
    let x_range = Uniform::new(output_geometry.loc.x, max_x);
    let y_range = Uniform::new(output_geometry.loc.y, max_y);
    let mut rng = rand::thread_rng();
    let x = x_range.sample(&mut rng);
    let y = y_range.sample(&mut rng);

    space.map_window(window, (x, y), activate);
}

pub fn fixup_positions(space: &mut Space) {
    // fixup outputs
    let mut offset = Point::<i32, Logical>::from((0, 0));
    for output in space.outputs().cloned().collect::<Vec<_>>().into_iter() {
        let size = space
            .output_geometry(&output)
            .map(|geo| geo.size)
            .unwrap_or_else(|| Size::from((0, 0)));
        let scale = space.output_scale(&output).unwrap_or(1.0);
        space.map_output(&output, scale, offset);
        layer_map_for_output(&output).arrange();
        offset.x += size.w;
    }

    // fixup windows
    let mut orphaned_windows = Vec::new();
    let outputs = space
        .outputs()
        .flat_map(|o| {
            let geo = space.output_geometry(o)?;
            let map = layer_map_for_output(o);
            let zone = map.non_exclusive_zone();
            Some(Rectangle::from_loc_and_size(geo.loc + zone.loc, zone.size))
        })
        .collect::<Vec<_>>();
    for window in space.windows() {
        let window_location = match space.window_location(window) {
            Some(loc) => loc,
            None => continue,
        };
        let geo_loc = window.bbox().loc + window_location;

        if !outputs.iter().any(|o_geo| o_geo.contains(geo_loc)) {
            orphaned_windows.push(window.clone());
        }
    }
    for window in orphaned_windows.into_iter() {
        place_new_window(space, &window, false);
    }
}
