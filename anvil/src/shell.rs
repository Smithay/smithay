use std::{cell::RefCell, sync::Mutex};

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    desktop::{
        layer_map_for_output, Kind as SurfaceKind, LayerSurface, PopupKeyboardGrab, PopupKind, PopupManager,
        PopupPointerGrab, PopupUngrabStrategy, Space, Window, WindowSurfaceType,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_buffer::WlBuffer, wl_output, wl_seat, wl_surface::WlSurface},
            DisplayHandle, Resource,
        },
    },
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_states, with_surface_tree_upward, CompositorHandler, CompositorState, TraversalAction,
        },
        output::Output,
        seat::{
            AxisFrame, ButtonEvent, Focus, MotionEvent, PointerGrab, PointerGrabStartData,
            PointerInnerHandle, Seat,
        },
        shell::{
            wlr_layer::{
                Layer, LayerSurface as WlrLayerSurface, LayerSurfaceAttributes, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::{
                Configure, PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface,
                XdgPopupSurfaceRoleAttributes, XdgShellHandler, XdgShellState,
                XdgToplevelSurfaceRoleAttributes,
            },
        },
        Serial,
    },
};

use crate::state::{AnvilState, Backend};

struct MoveSurfaceGrab {
    start_data: PointerGrabStartData,
    window: Window,
    initial_window_location: Point<i32, Logical>,
}

impl<BackendData> PointerGrab<AnvilState<BackendData>> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        _dh: &DisplayHandle,
        _handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &MotionEvent,
    ) {
        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;

        data.space
            .map_window(&self.window, new_location.to_i32_round(), None, true);
    }

    fn button(
        &mut self,
        _data: &mut AnvilState<BackendData>,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &ButtonEvent,
    ) {
        handle.button(event.button, event.state, event.serial, event.time);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(event.serial, event.time);
        }
    }

    fn axis(
        &mut self,
        _data: &mut AnvilState<BackendData>,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        details: AxisFrame,
    ) {
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

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    #[inline]
    fn from(x: xdg_toplevel::ResizeEdge) -> Self {
        Self::from_bits(x as u32).unwrap()
    }
}

impl From<ResizeEdge> for xdg_toplevel::ResizeEdge {
    #[inline]
    fn from(x: ResizeEdge) -> Self {
        use std::convert::TryFrom;

        Self::try_from(x.bits()).unwrap()
    }
}

struct ResizeSurfaceGrab {
    start_data: PointerGrabStartData,
    window: Window,
    edges: ResizeEdge,
    initial_window_location: Point<i32, Logical>,
    initial_window_size: Size<i32, Logical>,
    last_window_size: Size<i32, Logical>,
}

impl<BackendData> PointerGrab<AnvilState<BackendData>> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        _data: &mut AnvilState<BackendData>,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &MotionEvent,
    ) {
        // It is impossible to get `min_size` and `max_size` of dead toplevel, so we return early.
        if !self.window.toplevel().alive() {
            handle.unset_grab(event.serial, event.time);
            return;
        }

        let (mut dx, mut dy) = (event.location - self.start_data.location).into();

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

        let (min_size, max_size) = with_states(self.window.toplevel().wl_surface(), |states| {
            let data = states.cached_state.current::<SurfaceCachedState>();
            (data.min_size, data.max_size)
        });

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
                xdg.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                xdg.send_configure();
            }
            #[cfg(feature = "xwayland")]
            SurfaceKind::X11(_) => {
                // TODO: What to do here? Send the update via X11?
            }
        }
    }

    fn button(
        &mut self,
        data: &mut AnvilState<BackendData>,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &ButtonEvent,
    ) {
        handle.button(event.button, event.state, event.serial, event.time);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(event.serial, event.time);

            // If toplevel is dead, we can't resize it, so we return early.
            if !self.window.toplevel().alive() {
                return;
            }

            #[cfg_attr(not(feature = "xwayland"), allow(irrefutable_let_patterns))]
            if let SurfaceKind::Xdg(xdg) = &self.window.toplevel() {
                xdg.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                xdg.send_configure();
                if self.edges.intersects(ResizeEdge::TOP_LEFT) {
                    let geometry = self.window.geometry();
                    let mut location = data.space.window_location(&self.window).unwrap();

                    if self.edges.intersects(ResizeEdge::LEFT) {
                        location.x =
                            self.initial_window_location.x + (self.initial_window_size.w - geometry.size.w);
                    }
                    if self.edges.intersects(ResizeEdge::TOP) {
                        location.y =
                            self.initial_window_location.y + (self.initial_window_size.h - geometry.size.h);
                    }

                    data.space.map_window(&self.window, location, None, true);
                }

                with_states(self.window.toplevel().wl_surface(), |states| {
                    let mut data = states
                        .data_map
                        .get::<RefCell<SurfaceData>>()
                        .unwrap()
                        .borrow_mut();
                    if let ResizeState::Resizing(resize_data) = data.resize_state {
                        data.resize_state = ResizeState::WaitingForFinalAck(resize_data, event.serial);
                    } else {
                        panic!("invalid resize state: {:?}", data.resize_state);
                    }
                });
            } else {
                with_states(self.window.toplevel().wl_surface(), |states| {
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
                });
            }
        }
    }

    fn axis(
        &mut self,
        _data: &mut AnvilState<BackendData>,
        _dh: &DisplayHandle,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        details: AxisFrame,
    ) {
        handle.axis(details)
    }

    fn start_data(&self) -> &PointerGrabStartData {
        &self.start_data
    }
}

fn fullscreen_output_geometry(
    wl_surface: &WlSurface,
    wl_output: Option<&wl_output::WlOutput>,
    space: &mut Space,
) -> Option<Rectangle<i32, Logical>> {
    // First test if a specific output has been requested
    // if the requested output is not found ignore the request
    wl_output
        .and_then(Output::from_resource)
        .or_else(|| {
            let w = space
                .window_for_surface(wl_surface, WindowSurfaceType::TOPLEVEL)
                .cloned();
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

impl<BackendData> BufferHandler for AnvilState<BackendData> {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl<BackendData: Backend> CompositorHandler for AnvilState<BackendData> {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn commit(&mut self, dh: &DisplayHandle, surface: &WlSurface) {
        on_commit_buffer_handler(surface);
        self.backend_data.early_import(surface);

        #[cfg(feature = "xwayland")]
        if let Some(x11) = self.x11_state.as_mut() {
            super::xwayland::commit_hook(surface, dh, x11, &mut self.space);
        }

        self.space.commit(surface);
        self.popups.commit(surface);

        ensure_initial_configure(dh, surface, &self.space, &mut self.popups)
    }
}

impl<BackendData: Backend> XdgShellHandler for AnvilState<BackendData> {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, _dh: &DisplayHandle, surface: ToplevelSurface) {
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured
        let window = Window::new(SurfaceKind::Xdg(surface));
        place_new_window(&mut self.space, &window, true);
    }

    fn new_popup(&mut self, _dh: &DisplayHandle, surface: PopupSurface, positioner: PositionerState) {
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured

        // TODO: properly recompute the geometry with the whole of positioner state
        surface.with_pending_state(|state| {
            // NOTE: This is not really necessary as the default geometry
            // is already set the same way, but for demonstrating how
            // to set the initial popup geometry this code is left as
            // an example
            state.geometry = positioner.get_geometry();
        });
        if let Err(err) = self.popups.track_popup(PopupKind::from(surface)) {
            slog::warn!(self.log, "Failed to track popup: {}", err);
        }
    }

    fn reposition_request(
        &mut self,
        _dh: &DisplayHandle,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            // NOTE: This is again a simplification, a proper compositor would
            // calculate the geometry of the popup here. For simplicity we just
            // use the default implementation here that does not take the
            // window position and output constraints into account.
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn move_request(
        &mut self,
        _dh: &DisplayHandle,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
    ) {
        let seat: Seat<AnvilState<BackendData>> = Seat::from_resource(&seat).unwrap();
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
                .id()
                .same_client_as(&surface.wl_surface().id())
        {
            return;
        }

        let window = self
            .space
            .window_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL)
            .unwrap()
            .clone();
        let mut initial_window_location = self.space.window_location(&window).unwrap();

        // If surface is maximized then unmaximize it
        let current_state = surface.current_state();
        if current_state.states.contains(xdg_toplevel::State::Maximized) {
            surface.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.size = None;
            });

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

        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
        };

        pointer.set_grab(grab, serial, Focus::Clear);
    }

    fn resize_request(
        &mut self,
        _dh: &DisplayHandle,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let seat: Seat<AnvilState<BackendData>> = Seat::from_resource(&seat).unwrap();
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
                .id()
                .same_client_as(&surface.wl_surface().id())
        {
            return;
        }

        let window = self
            .space
            .window_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL)
            .unwrap()
            .clone();
        let geometry = window.geometry();
        let loc = self.space.window_location(&window).unwrap();
        let (initial_window_location, initial_window_size) = (loc, geometry.size);

        with_states(surface.wl_surface(), move |states| {
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
        });

        let grab = ResizeSurfaceGrab {
            start_data,
            window,
            edges: edges.into(),
            initial_window_location,
            initial_window_size,
            last_window_size: initial_window_size,
        };

        pointer.set_grab(grab, serial, Focus::Clear);
    }

    fn ack_configure(&mut self, _dh: &DisplayHandle, surface: WlSurface, configure: Configure) {
        if let Configure::Toplevel(configure) = configure {
            if let Some(serial) = with_states(&surface, |states| {
                if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>() {
                    if let ResizeState::WaitingForFinalAck(_, serial) = data.borrow().resize_state {
                        return Some(serial);
                    }
                }

                None
            }) {
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
                });

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
                    });
                }
            }
        }
    }

    fn fullscreen_request(
        &mut self,
        dh: &DisplayHandle,
        surface: ToplevelSurface,
        mut wl_output: Option<wl_output::WlOutput>,
    ) {
        // NOTE: This is only one part of the solution. We can set the
        // location and configure size here, but the surface should be rendered fullscreen
        // independently from its buffer size
        let wl_surface = surface.wl_surface();

        let output_geometry = fullscreen_output_geometry(wl_surface, wl_output.as_ref(), &mut self.space);

        if let Some(geometry) = output_geometry {
            let output = wl_output
                .as_ref()
                .and_then(Output::from_resource)
                .unwrap_or_else(|| self.space.outputs().next().unwrap().clone());
            let client = dh.get_client(wl_surface.id()).unwrap();
            output.with_client_outputs(dh, &client, |_dh, output| {
                wl_output = Some(output.clone());
            });

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Fullscreen);
                state.size = Some(geometry.size);
                state.fullscreen_output = wl_output;
            });

            let window = self
                .space
                .window_for_surface(wl_surface, WindowSurfaceType::TOPLEVEL)
                .unwrap();
            window.configure();
            output.user_data().insert_if_missing(FullscreenSurface::default);
            output
                .user_data()
                .get::<FullscreenSurface>()
                .unwrap()
                .set(window.clone());
            slog::trace!(self.log, "Fullscreening: {:?}", window);
        }
    }

    fn unfullscreen_request(&mut self, _dh: &DisplayHandle, surface: ToplevelSurface) {
        let ret = surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = None;
            state.fullscreen_output.take()
        });
        if let Some(output) = ret {
            let output = Output::from_resource(&output).unwrap();
            if let Some(fullscreen) = output.user_data().get::<FullscreenSurface>() {
                slog::trace!(self.log, "Unfullscreening: {:?}", fullscreen.get());
                fullscreen.clear();
                self.backend_data.reset_buffers(&output);
            }

            surface.send_configure();
        }
    }

    fn maximize_request(&mut self, _dh: &DisplayHandle, surface: ToplevelSurface) {
        // NOTE: This should use layer-shell when it is implemented to
        // get the correct maximum size
        let window = self
            .space
            .window_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL)
            .unwrap()
            .clone();
        let output = &self.space.outputs_for_window(&window)[0];
        let geometry = self.space.output_geometry(output).unwrap();

        self.space.map_window(&window, geometry.loc, None, true);
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some(geometry.size);
        });
        window.configure();
    }

    fn unmaximize_request(&mut self, _dh: &DisplayHandle, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = None;
        });
        surface.send_configure();
    }

    fn grab(&mut self, dh: &DisplayHandle, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<AnvilState<BackendData>> = Seat::from_resource(&seat).unwrap();
        let ret = self.popups.grab_popup(dh, surface.into(), &seat, serial);

        if let Ok(mut grab) = ret {
            if let Some(keyboard) = seat.get_keyboard() {
                if keyboard.is_grabbed()
                    && !(keyboard.has_grab(serial)
                        || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                {
                    grab.ungrab(dh, PopupUngrabStrategy::All);
                    return;
                }
                keyboard.set_focus(dh, grab.current_grab().as_ref(), serial);
                keyboard.set_grab(PopupKeyboardGrab::new(&grab), serial);
            }
            if let Some(pointer) = seat.get_pointer() {
                if pointer.is_grabbed()
                    && !(pointer.has_grab(serial)
                        || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                {
                    grab.ungrab(dh, PopupUngrabStrategy::All);
                    return;
                }
                pointer.set_grab(PopupPointerGrab::new(&grab), serial, Focus::Keep);
            }
        }
    }
}

impl<BackendData> WlrLayerShellHandler for AnvilState<BackendData> {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        dh: &DisplayHandle,
        surface: WlrLayerSurface,
        wl_output: Option<wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| self.space.outputs().next().unwrap().clone());
        let mut map = layer_map_for_output(&output);
        map.map_layer(dh, &LayerSurface::new(surface, namespace)).unwrap();
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

fn ensure_initial_configure(
    dh: &DisplayHandle,
    surface: &WlSurface,
    space: &Space,
    popups: &mut PopupManager,
) {
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

    if let Some(window) = space
        .window_for_surface(surface, WindowSurfaceType::TOPLEVEL)
        .cloned()
    {
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
            });
            if !initial_configure_sent {
                toplevel.send_configure();
            }
        }

        with_states(surface, |states| {
            let mut data = states
                .data_map
                .get::<RefCell<SurfaceData>>()
                .unwrap()
                .borrow_mut();

            // Finish resizing.
            if let ResizeState::WaitingForCommit(_) = data.resize_state {
                data.resize_state = ResizeState::NotResizing;
            }
        });

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
        });
        if !initial_configure_sent {
            // NOTE: This should never fail as the initial configure is always
            // allowed.
            popup.send_configure().expect("initial configure failed");
        }

        return;
    };

    if let Some(output) = space.outputs().find(|o| {
        let map = layer_map_for_output(o);
        map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .is_some()
    }) {
        let mut map = layer_map_for_output(output);
        let layer = map
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .unwrap();

        // send the initial configure if relevant
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_configure_sent {
            layer.layer_surface().send_configure();
        }

        map.arrange(dh);
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

    space.map_window(window, (x, y), None, activate);
}

pub fn fixup_positions(dh: &DisplayHandle, space: &mut Space) {
    // fixup outputs
    let mut offset = Point::<i32, Logical>::from((0, 0));
    for output in space.outputs().cloned().collect::<Vec<_>>().into_iter() {
        let size = space
            .output_geometry(&output)
            .map(|geo| geo.size)
            .unwrap_or_else(|| Size::from((0, 0)));
        space.map_output(&output, offset);
        layer_map_for_output(&output).arrange(dh);
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
