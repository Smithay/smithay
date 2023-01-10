use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    input::Seat,
    utils::{Logical, Point, Rectangle, Serial},
    wayland::shell::xdg::XdgShellHandler,
};

use std::cell::{RefCell, RefMut};

use crate::AnvilState;

use super::WindowElement;

pub struct WindowState {
    pub is_ssd: bool,
    pub ptr_entered_window: bool,
    pub header_bar: HeaderBar,
}

#[derive(Debug, Clone)]
pub struct HeaderBar {
    pub pointer_loc: Option<Point<f64, Logical>>,
    pub width: u32,
    pub close_button_hover: bool,
    pub maximize_button_hover: bool,
    pub buffer: MemoryRenderBuffer,
}

const BG_COLOR: &[u8] = &[255, 231, 199, 255];
const MAX_COLOR: &[u8] = &[255, 246, 181, 255];
const CLOSE_COLOR: &[u8] = &[255, 169, 156, 255];
const MAX_COLOR_HOVER: &[u8] = &[181, 159, 0, 255];
const CLOSE_COLOR_HOVER: &[u8] = &[191, 28, 4, 255];

pub const HEADER_BAR_HEIGHT: i32 = 32;
const BUTTON_HEIGHT: u32 = HEADER_BAR_HEIGHT as u32;
const BUTTON_WIDTH: u32 = 32;

impl HeaderBar {
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

        let mut render_context = self.buffer.render();
        render_context.resize((width as i32, HEADER_BAR_HEIGHT));

        let mut needs_redraw_buttons = false;
        if width != self.width {
            render_context
                .draw(|buffer| {
                    buffer.chunks_exact_mut(4).for_each(|chunk| {
                        chunk.copy_from_slice(BG_COLOR);
                    });
                    Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                        (0, 0),
                        (width as i32, HEADER_BAR_HEIGHT),
                    )])
                })
                .unwrap();
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
            render_context
                .draw(|buffer| {
                    buffer
                        .chunks_exact_mut((width * 4) as usize)
                        .flat_map(|x| {
                            x[((width - BUTTON_WIDTH) * 4) as usize..(width * 4) as usize].chunks_exact_mut(4)
                        })
                        .for_each(|chunk| chunk.copy_from_slice(CLOSE_COLOR_HOVER));
                    Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                        ((width - BUTTON_WIDTH) as i32, 0),
                        (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
                    )])
                })
                .unwrap();
            self.close_button_hover = true;
        } else if !self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || self.close_button_hover)
        {
            render_context
                .draw(|buffer| {
                    buffer
                        .chunks_exact_mut((width * 4) as usize)
                        .flat_map(|x| {
                            x[((width - 32) * 4) as usize..(width * 4) as usize].chunks_exact_mut(4)
                        })
                        .for_each(|chunk| chunk.copy_from_slice(CLOSE_COLOR));
                    Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                        ((width - BUTTON_WIDTH) as i32, 0),
                        (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
                    )])
                })
                .unwrap();

            self.close_button_hover = false;
        }

        if self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH * 2) as f64 && l.x <= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || !self.maximize_button_hover)
        {
            render_context
                .draw(|buffer| {
                    buffer
                        .chunks_exact_mut((width * 4) as usize)
                        .flat_map(|x| {
                            x[((width - (BUTTON_WIDTH * 2)) * 4) as usize
                                ..((width - BUTTON_WIDTH) * 4) as usize]
                                .chunks_exact_mut(4)
                        })
                        .for_each(|chunk| chunk.copy_from_slice(MAX_COLOR_HOVER));
                    Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                        ((width - BUTTON_WIDTH * 2) as i32, 0),
                        (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
                    )])
                })
                .unwrap();

            self.maximize_button_hover = true;
        } else if !self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH * 2) as f64 && l.x <= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || self.maximize_button_hover)
        {
            render_context
                .draw(|buffer| {
                    buffer
                        .chunks_exact_mut((width * 4) as usize)
                        .flat_map(|x| {
                            x[((width - (BUTTON_WIDTH * 2)) * 4) as usize
                                ..((width - BUTTON_WIDTH) * 4) as usize]
                                .chunks_exact_mut(4)
                        })
                        .for_each(|chunk| chunk.copy_from_slice(MAX_COLOR));
                    Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                        ((width - BUTTON_WIDTH * 2) as i32, 0),
                        (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
                    )])
                })
                .unwrap();

            self.maximize_button_hover = false;
        }
    }
}

impl WindowElement {
    pub fn decoration_state(&self) -> RefMut<'_, WindowState> {
        self.user_data().insert_if_missing(|| {
            RefCell::new(WindowState {
                is_ssd: false,
                ptr_entered_window: false,
                header_bar: HeaderBar {
                    pointer_loc: None,
                    width: 0,
                    close_button_hover: false,
                    maximize_button_hover: false,
                    buffer: MemoryRenderBuffer::default(),
                },
            })
        });

        self.user_data()
            .get::<RefCell<WindowState>>()
            .unwrap()
            .borrow_mut()
    }

    pub fn set_ssd(&self, ssd: bool) {
        self.decoration_state().is_ssd = ssd;
    }
}
