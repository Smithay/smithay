use smithay::{
    backend::renderer::{
        element::{
            solid::{SolidColorBuffer, SolidColorRenderElement},
            AsRenderElements, Kind,
        },
        Renderer,
    }, input::Seat, reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode, utils::{Logical, Point, Serial}, wayland::shell::xdg::XdgShellHandler
};

use std::cell::{RefCell, RefMut};

use crate::AnvilState;

use super::WindowElement;

pub struct WindowState {
    pub ssd_mode: Mode,
    pub ptr_entered_window: bool,
    pub decorations: Decorations,
}

#[derive(Debug, Clone)]
pub struct Decorations {
    pub pointer_loc: Option<Point<f64, Logical>>,
    pub width: u32,
    pub close_button_hover: bool,
    pub maximize_button_hover: bool,
    pub background: Option<SolidColorBuffer>,
    pub close_button: SolidColorBuffer,
    pub maximize_button: SolidColorBuffer,
}

const BG_COLOR: [f32; 4] = [0.75f32, 0.9f32, 0.78f32, 1f32];
const MAX_COLOR: [f32; 4] = [1f32, 0.965f32, 0.71f32, 1f32];
const CLOSE_COLOR: [f32; 4] = [1f32, 0.66f32, 0.612f32, 1f32];
const MAX_COLOR_HOVER: [f32; 4] = [0.71f32, 0.624f32, 0f32, 1f32];
const CLOSE_COLOR_HOVER: [f32; 4] = [0.75f32, 0.11f32, 0.016f32, 1f32];

pub const HEADER_BAR_HEIGHT: i32 = 32;
const BUTTON_HEIGHT: u32 = HEADER_BAR_HEIGHT as u32;
pub const BUTTON_WIDTH: u32 = 32;

impl Decorations {
    pub fn pointer_enter(&mut self, loc: Point<f64, Logical>) {
        self.pointer_loc = Some(loc);
    }

    pub fn pointer_leave(&mut self) {
        self.pointer_loc = None;
    }

    pub fn clicked<B: crate::state::Backend>(
        &mut self,
        seat: &Seat<AnvilState<B>>,
        state: &mut AnvilState<B>,
        window: &WindowElement,
        serial: Serial,
    ) {
        match self.pointer_loc.as_ref() {
            Some(loc) if loc.x >= (self.width - BUTTON_WIDTH) as f64 => {
                match window {
                    WindowElement::Wayland(w) => w.toplevel().send_close(),
                    #[cfg(feature = "xwayland")]
                    WindowElement::X11(w) => {
                        let _ = w.close();
                    }
                };
            }
            Some(loc) if loc.x >= (self.width - (BUTTON_WIDTH * 2)) as f64 => {
                match window {
                    WindowElement::Wayland(w) => state.maximize_request(w.toplevel().clone()),
                    #[cfg(feature = "xwayland")]
                    WindowElement::X11(w) => {
                        let surface = w.clone();
                        state
                            .handle
                            .insert_idle(move |data| data.state.maximize_request_x11(&surface));
                    }
                };
            }
            Some(_) => {
                match window {
                    WindowElement::Wayland(w) => {
                        let seat = seat.clone();
                        let toplevel = w.toplevel().clone();
                        state
                            .handle
                            .insert_idle(move |data| data.state.move_request_xdg(&toplevel, &seat, serial));
                    }
                    #[cfg(feature = "xwayland")]
                    WindowElement::X11(w) => {
                        let window = w.clone();
                        state
                            .handle
                            .insert_idle(move |data| data.state.move_request_x11(&window));
                    }
                };
            }
            _ => {}
        };
    }

    pub fn redraw(&mut self, width: u32) {
        if width == 0 {
            self.width = 0;
            return;
        }

        if let Some(background) = &mut self.background {
            background.update((width as i32, HEADER_BAR_HEIGHT), BG_COLOR);
        }

        let mut needs_redraw_buttons = false;
        if width != self.width {
            needs_redraw_buttons = true;
            self.width = width;
        }

        if self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || !self.close_button_hover)
        {
            self.close_button
                .update((BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32), CLOSE_COLOR_HOVER);
            self.close_button_hover = true;
        } else if !self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || self.close_button_hover)
        {
            self.close_button
                .update((BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32), CLOSE_COLOR);
            self.close_button_hover = false;
        }

        if self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH * 2) as f64 && l.x <= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || !self.maximize_button_hover)
        {
            self.maximize_button
                .update((BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32), MAX_COLOR_HOVER);
            self.maximize_button_hover = true;
        } else if !self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH * 2) as f64 && l.x <= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || self.maximize_button_hover)
        {
            self.maximize_button
                .update((BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32), MAX_COLOR);
            self.maximize_button_hover = false;
        }
    }
}

impl<R: Renderer> AsRenderElements<R> for Decorations {
    type RenderElement = SolidColorRenderElement;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        _renderer: &mut R,
        location: Point<i32, smithay::utils::Physical>,
        scale: smithay::utils::Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        let header_end_offset: Point<i32, Logical> = Point::from((self.width as i32, 0));
        let button_offset: Point<i32, Logical> = Point::from((BUTTON_WIDTH as i32, 0));

        let mut elements = vec![
            SolidColorRenderElement::from_buffer(
                &self.close_button,
                location + (header_end_offset - button_offset).to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::Unspecified,
            )
            .into(),
            SolidColorRenderElement::from_buffer(
                &self.maximize_button,
                location + (header_end_offset - button_offset.upscale(2)).to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::Unspecified,
            )
            .into()
        ];

        if let Some(background) = &self.background {
            elements.push(
                SolidColorRenderElement::from_buffer(
                    background,
                    location,
                    scale,
                    alpha,
                    Kind::Unspecified,
                )
                .into(),
            );
        }

        elements
    }
}

impl WindowElement {
    pub fn decoration_state(&self) -> RefMut<'_, WindowState> {
        self.user_data().insert_if_missing(|| {
            RefCell::new(WindowState {
                ssd_mode: Mode::ClientSide,
                ptr_entered_window: false,
                decorations: Decorations {
                    pointer_loc: None,
                    width: 0,
                    close_button_hover: false,
                    maximize_button_hover: false,
                    background: Some(SolidColorBuffer::default()),
                    close_button: SolidColorBuffer::default(),
                    maximize_button: SolidColorBuffer::default(),
                },
            })
        });

        self.user_data()
            .get::<RefCell<WindowState>>()
            .unwrap()
            .borrow_mut()
    }

    pub fn set_no_ssd(&self) {
        self.decoration_state().ssd_mode = Mode::ClientSide;
    }

    pub fn set_ssd(&self) {
        let mut state = self.decoration_state();
        state.ssd_mode = Mode::ServerSide;
        state.decorations.background = Some(SolidColorBuffer::default());
    }

    pub fn set_ssd_overlay(&self) {
        let mut state = self.decoration_state();
        state.ssd_mode = Mode::ServerSideOverlay;
        state.decorations.background = None;
    }
}
