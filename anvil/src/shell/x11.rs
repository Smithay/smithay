use std::cell::RefCell;

use smithay::{
    desktop::space::SpaceElement,
    input::pointer::Focus,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::compositor::with_states,
    xwayland::{
        xwm::{ResizeEdge as X11ResizeEdge, XwmId},
        X11Surface, XwmHandler, X11WM,
    },
};

use crate::{state::Backend, AnvilState, CalloopData};

use super::{
    place_new_window, FullscreenSurface, MoveSurfaceGrab, ResizeData, ResizeState, ResizeSurfaceGrab,
    SurfaceData, WindowElement,
};

#[derive(Debug, Default)]
struct OldGeometry(RefCell<Option<Rectangle<i32, Logical>>>);
impl OldGeometry {
    pub fn save(&self, geo: Rectangle<i32, Logical>) {
        *self.0.borrow_mut() = Some(geo);
    }

    pub fn restore(&self) -> Option<Rectangle<i32, Logical>> {
        self.0.borrow_mut().take()
    }
}

impl<BackendData: Backend> XwmHandler for CalloopData<BackendData> {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11WM {
        self.state.xwm.as_mut().unwrap()
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        window.set_mapped(true).unwrap();
        let window = WindowElement::X11(window);
        place_new_window(&mut self.state.space, &window, true);
        let bbox = self.state.space.element_bbox(&window).unwrap();
        let WindowElement::X11(xsurface) = &window else { unreachable!() };
        xsurface.configure(Some(bbox)).unwrap();
        window.set_ssd(!xsurface.is_decorated());
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let location = window.geometry().loc;
        let window = WindowElement::X11(window);
        self.state.space.map_element(window, location, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let maybe = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == &window))
            .cloned();
        if let Some(elem) = maybe {
            self.state.space.unmap_elem(&elem)
        }
        if !window.is_override_redirect() {
            window.set_mapped(false).unwrap();
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
    ) {
        // Nope
        let mut geo = window.geometry();
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(&mut self, _xwm: XwmId, window: X11Surface, x: i32, y: i32, _w: u32, _h: u32) {
        let Some(elem) = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == &window))
            .cloned()
        else { return };
        self.state.space.map_element(elem, (x, y), false);
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.state.maximize_request_x11(&window);
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(elem) = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == &window))
            .cloned()
        else { return };

        window.set_maximized(false).unwrap();
        if let Some(old_geo) = window
            .user_data()
            .get::<OldGeometry>()
            .and_then(|data| data.restore())
        {
            window.configure(old_geo).unwrap();
            self.state.space.map_element(elem, old_geo.loc, false);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(elem) = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == &window))
        {
            let outputs_for_window = self.state.space.outputs_for_element(elem);
            let output = outputs_for_window
                .first()
                // The window hasn't been mapped yet, use the primary output instead
                .or_else(|| self.state.space.outputs().next())
                // Assumes that at least one output exists
                .expect("No outputs found");
            let geometry = self.state.space.output_geometry(output).unwrap();

            window.set_fullscreen(true).unwrap();
            window.configure(geometry).unwrap();
            output.user_data().insert_if_missing(FullscreenSurface::default);
            output
                .user_data()
                .get::<FullscreenSurface>()
                .unwrap()
                .set(elem.clone());
            slog::trace!(self.state.log, "Fullscreening: {:?}", elem);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(elem) = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == &window))
        {
            window.set_fullscreen(false).unwrap();
            if let Some(output) = self.state.space.outputs().find(|o| {
                o.user_data()
                    .get::<FullscreenSurface>()
                    .and_then(|f| f.get())
                    .map(|w| &w == elem)
                    .unwrap_or(false)
            }) {
                slog::trace!(self.state.log, "Unfullscreening: {:?}", elem);
                output.user_data().get::<FullscreenSurface>().unwrap().clear();
                window.configure(self.state.space.element_bbox(elem)).unwrap();
                self.state.backend_data.reset_buffers(output);
            }
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, edges: X11ResizeEdge) {
        let seat = &self.state.seat; // luckily anvil only supports one seat anyway...
        let pointer = seat.get_pointer().unwrap();
        let start_data = pointer.grab_start_data().unwrap();

        let Some(element) = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == &window)) else { return };

        let geometry = element.geometry();
        let loc = self.state.space.element_location(element).unwrap();
        let (initial_window_location, initial_window_size) = (loc, geometry.size);

        with_states(&element.wl_surface().unwrap(), move |states| {
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
            window: element.clone(),
            edges: edges.into(),
            initial_window_location,
            initial_window_size,
            last_window_size: initial_window_size,
        };

        pointer.set_grab(&mut self.state, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        self.state.move_request_x11(&window)
    }
}

impl<BackendData: Backend> AnvilState<BackendData> {
    pub fn maximize_request_x11(&mut self, window: &X11Surface) {
        let Some(elem) = self
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == window))
            .cloned()
        else { return };

        let old_geo = self.space.element_bbox(&elem).unwrap();
        let outputs_for_window = self.space.outputs_for_element(&elem);
        let output = outputs_for_window
            .first()
            // The window hasn't been mapped yet, use the primary output instead
            .or_else(|| self.space.outputs().next())
            // Assumes that at least one output exists
            .expect("No outputs found");
        let geometry = self.space.output_geometry(output).unwrap();

        window.set_maximized(true).unwrap();
        window.configure(geometry).unwrap();
        window.user_data().insert_if_missing(OldGeometry::default);
        window.user_data().get::<OldGeometry>().unwrap().save(old_geo);
        self.space.map_element(elem, geometry.loc, false);
    }

    pub fn move_request_x11(&mut self, window: &X11Surface) {
        let seat = &self.seat; // luckily anvil only supports one seat anyway...
        let pointer = seat.get_pointer().unwrap();
        let Some(start_data) = pointer.grab_start_data() else { return };

        let Some(element) = self
            .space
            .elements()
            .find(|e| matches!(e, WindowElement::X11(w) if w == window)) else { return };

        let mut initial_window_location = self.space.element_location(element).unwrap();

        // If surface is maximized then unmaximize it
        if window.is_maximized() {
            window.set_maximized(false).unwrap();
            let pos = pointer.current_location();
            initial_window_location = (pos.x as i32, pos.y as i32).into();
            if let Some(old_geo) = window
                .user_data()
                .get::<OldGeometry>()
                .and_then(|data| data.restore())
            {
                window
                    .configure(Rectangle::from_loc_and_size(
                        initial_window_location,
                        old_geo.size,
                    ))
                    .unwrap();
            }
        }

        let grab = MoveSurfaceGrab {
            start_data,
            window: element.clone(),
            initial_window_location,
        };

        pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }
}
