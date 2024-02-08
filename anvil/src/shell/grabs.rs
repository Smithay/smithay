use std::cell::RefCell;

use smithay::{
    desktop::{space::SpaceElement, WindowSurface},
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{IsAlive, Logical, Point, Serial, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};
#[cfg(feature = "xwayland")]
use smithay::{utils::Rectangle, xwayland::xwm::ResizeEdge as X11ResizeEdge};

use super::{SurfaceData, WindowElement};
use crate::{
    focus::FocusTarget,
    state::{AnvilState, Backend},
};

pub struct MoveSurfaceGrab<B: Backend + 'static> {
    pub start_data: PointerGrabStartData<AnvilState<B>>,
    pub window: WindowElement,
    pub initial_window_location: Point<i32, Logical>,
}

impl<BackendData: Backend> PointerGrab<AnvilState<BackendData>> for MoveSurfaceGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        _focus: Option<(FocusTarget, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;

        data.space
            .map_element(self.window.clone(), new_location.to_i32_round(), true);
    }

    fn relative_motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        focus: Option<(FocusTarget, Point<i32, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
    ) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<AnvilState<BackendData>> {
        &self.start_data
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ResizeEdge: u32 {
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
        Self::try_from(x.bits()).unwrap()
    }
}

#[cfg(feature = "xwayland")]
impl From<X11ResizeEdge> for ResizeEdge {
    fn from(edge: X11ResizeEdge) -> Self {
        match edge {
            X11ResizeEdge::Bottom => ResizeEdge::BOTTOM,
            X11ResizeEdge::BottomLeft => ResizeEdge::BOTTOM_LEFT,
            X11ResizeEdge::BottomRight => ResizeEdge::BOTTOM_RIGHT,
            X11ResizeEdge::Left => ResizeEdge::LEFT,
            X11ResizeEdge::Right => ResizeEdge::RIGHT,
            X11ResizeEdge::Top => ResizeEdge::TOP,
            X11ResizeEdge::TopLeft => ResizeEdge::TOP_LEFT,
            X11ResizeEdge::TopRight => ResizeEdge::TOP_RIGHT,
        }
    }
}

/// Information about the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    /// The edges the surface is being resized with.
    pub edges: ResizeEdge,
    /// The initial window location.
    pub initial_window_location: Point<i32, Logical>,
    /// The initial window size (geometry width and height).
    pub initial_window_size: Size<i32, Logical>,
}

/// State of the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum ResizeState {
    /// The surface is not being resized.
    #[default]
    NotResizing,
    /// The surface is currently being resized.
    Resizing(ResizeData),
    /// The resize has finished, and the surface needs to ack the final configure.
    WaitingForFinalAck(ResizeData, Serial),
    /// The resize has finished, and the surface needs to commit its final state.
    WaitingForCommit(ResizeData),
}

pub struct ResizeSurfaceGrab<B: Backend + 'static> {
    pub start_data: PointerGrabStartData<AnvilState<B>>,
    pub window: WindowElement,
    pub edges: ResizeEdge,
    pub initial_window_location: Point<i32, Logical>,
    pub initial_window_size: Size<i32, Logical>,
    pub last_window_size: Size<i32, Logical>,
}

impl<BackendData: Backend> PointerGrab<AnvilState<BackendData>> for ResizeSurfaceGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        _focus: Option<(FocusTarget, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        // It is impossible to get `min_size` and `max_size` of dead toplevel, so we return early.
        if !self.window.alive() {
            handle.unset_grab(data, event.serial, event.time, true);
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

        let (min_size, max_size) = if let Some(surface) = self.window.wl_surface() {
            with_states(&surface, |states| {
                let data = states.cached_state.current::<SurfaceCachedState>();
                (data.min_size, data.max_size)
            })
        } else {
            ((0, 0).into(), (0, 0).into())
        };

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

        match &self.window.0.underlying_surface() {
            WindowSurface::Wayland(xdg) => {
                xdg.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                xdg.send_pending_configure();
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11) => {
                let location = data.space.element_location(&self.window).unwrap();
                x11.configure(Rectangle::from_loc_and_size(location, self.last_window_size))
                    .unwrap();
            }
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        focus: Option<(FocusTarget, Point<i32, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(data, event.serial, event.time, true);

            // If toplevel is dead, we can't resize it, so we return early.
            if !self.window.alive() {
                return;
            }

            match &self.window.0.underlying_surface() {
                WindowSurface::Wayland(xdg) => {
                    xdg.with_pending_state(|state| {
                        state.states.unset(xdg_toplevel::State::Resizing);
                        state.size = Some(self.last_window_size);
                    });
                    xdg.send_pending_configure();
                    if self.edges.intersects(ResizeEdge::TOP_LEFT) {
                        let geometry = self.window.geometry();
                        let mut location = data.space.element_location(&self.window).unwrap();

                        if self.edges.intersects(ResizeEdge::LEFT) {
                            location.x = self.initial_window_location.x
                                + (self.initial_window_size.w - geometry.size.w);
                        }
                        if self.edges.intersects(ResizeEdge::TOP) {
                            location.y = self.initial_window_location.y
                                + (self.initial_window_size.h - geometry.size.h);
                        }

                        data.space.map_element(self.window.clone(), location, true);
                    }

                    with_states(&self.window.wl_surface().unwrap(), |states| {
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
                }
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(x11) => {
                    let mut location = data.space.element_location(&self.window).unwrap();
                    if self.edges.intersects(ResizeEdge::TOP_LEFT) {
                        let geometry = self.window.geometry();

                        if self.edges.intersects(ResizeEdge::LEFT) {
                            location.x = self.initial_window_location.x
                                + (self.initial_window_size.w - geometry.size.w);
                        }
                        if self.edges.intersects(ResizeEdge::TOP) {
                            location.y = self.initial_window_location.y
                                + (self.initial_window_size.h - geometry.size.h);
                        }

                        data.space.map_element(self.window.clone(), location, true);
                    }
                    x11.configure(Rectangle::from_loc_and_size(location, self.last_window_size))
                        .unwrap();

                    let Some(surface) = self.window.wl_surface() else {
                        // X11 Window got unmapped, abort
                        return;
                    };
                    with_states(&surface, |states| {
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
    }

    fn axis(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
    ) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<AnvilState<BackendData>> {
        &self.start_data
    }
}
