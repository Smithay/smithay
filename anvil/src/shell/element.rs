use std::time::Duration;

use smithay::{
    backend::{
        input::KeyState,
        renderer::{
            element::{
                solid::SolidColorRenderElement, surface::WaylandSurfaceRenderElement, AsRenderElements,
            },
            ImportAll, ImportMem, Renderer, Texture,
        },
    },
    desktop::{space::SpaceElement, utils::OutputPresentationFeedback, Window, WindowSurfaceType},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
        },
        Seat,
    },
    output::Output,
    reexports::{
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    render_elements,
    utils::{user_data::UserDataMap, IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial},
    wayland::{compositor::SurfaceData as WlSurfaceData, dmabuf::DmabufFeedback, seat::WaylandFocus},
};

use super::ssd::HEADER_BAR_HEIGHT;
use crate::AnvilState;

#[derive(Debug, Clone, PartialEq)]
pub struct WindowElement(pub Window);

impl WindowElement {
    pub fn surface_under(
        &self,
        location: Point<f64, Logical>,
        window_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        self.0.surface_under(location, window_type)
    }

    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&WlSurface, &WlSurfaceData),
    {
        self.0.with_surfaces(processor);
    }

    pub fn send_frame<T, F>(
        &self,
        output: &Output,
        time: T,
        throttle: Option<Duration>,
        primary_scan_out_output: F,
    ) where
        T: Into<Duration>,
        F: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
    {
        self.0.send_frame(output, time, throttle, primary_scan_out_output)
    }

    pub fn send_dmabuf_feedback<'a, P, F>(
        &self,
        output: &Output,
        primary_scan_out_output: P,
        select_dmabuf_feedback: F,
    ) where
        P: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F: Fn(&WlSurface, &WlSurfaceData) -> &'a DmabufFeedback + Copy,
    {
        self.0
            .send_dmabuf_feedback(output, primary_scan_out_output, select_dmabuf_feedback)
    }

    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&WlSurface, &WlSurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        self.0.take_presentation_feedback(
            output_feedback,
            primary_scan_out_output,
            presentation_feedback_flags,
        )
    }

    #[cfg(feature = "xwayland")]
    pub fn is_x11(&self) -> bool {
        self.0.is_x11()
    }

    pub fn is_wayland(&self) -> bool {
        self.0.is_wayland()
    }

    pub fn wl_surface(&self) -> Option<WlSurface> {
        self.0.wl_surface()
    }

    pub fn user_data(&self) -> &UserDataMap {
        self.0.user_data()
    }
}

impl IsAlive for WindowElement {
    fn alive(&self) -> bool {
        self.0.alive()
    }
}

impl<Backend: crate::state::Backend> PointerTarget<AnvilState<Backend>> for WindowElement {
    fn enter(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, event: &MotionEvent) {
        let mut state = self.decoration_state();
        if state.is_ssd {
            if event.location.y < HEADER_BAR_HEIGHT as f64 {
                state.header_bar.pointer_enter(event.location);
            } else {
                state.header_bar.pointer_leave();
                let mut event = event.clone();
                event.location.y -= HEADER_BAR_HEIGHT as f64;
                PointerTarget::enter(&self.0, seat, data, &event);
                state.ptr_entered_window = true;
            }
        } else {
            state.ptr_entered_window = true;
            PointerTarget::enter(&self.0, seat, data, event);
        }
    }
    fn motion(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, event: &MotionEvent) {
        let mut state = self.decoration_state();
        if state.is_ssd {
            if event.location.y < HEADER_BAR_HEIGHT as f64 {
                PointerTarget::leave(&self.0, seat, data, event.serial, event.time);
                state.ptr_entered_window = false;
                state.header_bar.pointer_enter(event.location);
            } else {
                state.header_bar.pointer_leave();
                let mut event = event.clone();
                event.location.y -= HEADER_BAR_HEIGHT as f64;
                if state.ptr_entered_window {
                    PointerTarget::motion(&self.0, seat, data, &event);
                } else {
                    state.ptr_entered_window = true;
                    PointerTarget::enter(&self.0, seat, data, &event);
                }
            }
        } else {
            PointerTarget::motion(&self.0, seat, data, event);
        }
    }
    fn relative_motion(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &RelativeMotionEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::relative_motion(&self.0, seat, data, event);
        }
    }
    fn button(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, event: &ButtonEvent) {
        let mut state = self.decoration_state();
        if state.is_ssd {
            if state.ptr_entered_window {
                PointerTarget::button(&self.0, seat, data, event);
            } else {
                state.header_bar.clicked(seat, data, self, event.serial);
            }
        } else {
            PointerTarget::button(&self.0, seat, data, event);
        }
    }
    fn axis(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, frame: AxisFrame) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::axis(&self.0, seat, data, frame);
        }
    }
    fn frame(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::frame(&self.0, seat, data);
        }
    }
    fn leave(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        serial: Serial,
        time: u32,
    ) {
        let mut state = self.decoration_state();
        if state.is_ssd {
            state.header_bar.pointer_leave();
            if state.ptr_entered_window {
                PointerTarget::leave(&self.0, seat, data, serial, time);

                state.ptr_entered_window = false;
            }
        } else {
            PointerTarget::leave(&self.0, seat, data, serial, time);
            state.ptr_entered_window = false;
        }
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GestureSwipeBeginEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_swipe_begin(&self.0, seat, data, event);
        }
    }
    fn gesture_swipe_update(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GestureSwipeUpdateEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_swipe_update(&self.0, seat, data, event);
        }
    }
    fn gesture_swipe_end(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GestureSwipeEndEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_swipe_end(&self.0, seat, data, event);
        }
    }
    fn gesture_pinch_begin(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GesturePinchBeginEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_pinch_begin(&self.0, seat, data, event);
        }
    }
    fn gesture_pinch_update(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GesturePinchUpdateEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_pinch_update(&self.0, seat, data, event);
        }
    }
    fn gesture_pinch_end(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GesturePinchEndEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_pinch_end(&self.0, seat, data, event);
        }
    }
    fn gesture_hold_begin(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GestureHoldBeginEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_hold_begin(&self.0, seat, data, event);
        }
    }
    fn gesture_hold_end(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        event: &GestureHoldEndEvent,
    ) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::gesture_hold_end(&self.0, seat, data, event);
        }
    }
}

impl<Backend: crate::state::Backend> KeyboardTarget<AnvilState<Backend>> for WindowElement {
    fn enter(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        KeyboardTarget::enter(&self.0, seat, data, keys, serial);
    }
    fn leave(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, serial: Serial) {
        KeyboardTarget::leave(&self.0, seat, data, serial);
    }
    fn key(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        KeyboardTarget::key(&self.0, seat, data, key, state, serial, time);
    }
    fn modifiers(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        KeyboardTarget::modifiers(&self.0, seat, data, modifiers, serial);
    }
}

impl SpaceElement for WindowElement {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = SpaceElement::geometry(&self.0);
        if self.decoration_state().is_ssd {
            geo.size.h += HEADER_BAR_HEIGHT;
        }
        geo
    }
    fn bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = SpaceElement::bbox(&self.0);
        if self.decoration_state().is_ssd {
            bbox.size.h += HEADER_BAR_HEIGHT;
        }
        bbox
    }
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        if self.decoration_state().is_ssd {
            point.y < HEADER_BAR_HEIGHT as f64
                || SpaceElement::is_in_input_region(
                    &self.0,
                    &(*point - Point::from((0.0, HEADER_BAR_HEIGHT as f64))),
                )
        } else {
            SpaceElement::is_in_input_region(&self.0, point)
        }
    }
    fn z_index(&self) -> u8 {
        SpaceElement::z_index(&self.0)
    }

    fn set_activate(&self, activated: bool) {
        SpaceElement::set_activate(&self.0, activated);
    }
    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        SpaceElement::output_enter(&self.0, output, overlap);
    }
    fn output_leave(&self, output: &Output) {
        SpaceElement::output_leave(&self.0, output);
    }
    #[profiling::function]
    fn refresh(&self) {
        SpaceElement::refresh(&self.0);
    }
}

render_elements!(
    pub WindowRenderElement<R> where R: ImportAll + ImportMem;
    Window=WaylandSurfaceRenderElement<R>,
    Decoration=SolidColorRenderElement,
);

impl<R: Renderer> std::fmt::Debug for WindowRenderElement<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Window(arg0) => f.debug_tuple("Window").field(arg0).finish(),
            Self::Decoration(arg0) => f.debug_tuple("Decoration").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

impl<R> AsRenderElements<R> for WindowElement
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Texture + 'static,
{
    type RenderElement = WindowRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        mut location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        let window_bbox = SpaceElement::bbox(&self.0);

        if self.decoration_state().is_ssd && !window_bbox.is_empty() {
            let window_geo = SpaceElement::geometry(&self.0);

            let mut state = self.decoration_state();
            let width = window_geo.size.w;
            state.header_bar.redraw(width as u32);
            let mut vec = AsRenderElements::<R>::render_elements::<WindowRenderElement<R>>(
                &state.header_bar,
                renderer,
                location,
                scale,
                alpha,
            );

            location.y += (scale.y * HEADER_BAR_HEIGHT as f64) as i32;

            let window_elements =
                AsRenderElements::render_elements(&self.0, renderer, location, scale, alpha);
            vec.extend(window_elements);
            vec.into_iter().map(C::from).collect()
        } else {
            AsRenderElements::render_elements(&self.0, renderer, location, scale, alpha)
                .into_iter()
                .map(C::from)
                .collect()
        }
    }
}
