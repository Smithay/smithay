use smithay::{
    backend::{
        input::KeyState,
        renderer::{
            element::{
                render_elements, surface::WaylandSurfaceRenderElement, AsRenderElements, Id, RenderElement,
                UnderlyingStorage,
            },
            Frame, ImportAll, ImportMem, Renderer, Texture,
        },
    },
    desktop::{space::SpaceElement, Kind, Window},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget},
        Seat,
    },
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial, Size, Transform},
    wayland::{output::Output, shell::xdg::XdgShellHandler},
};

use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
};

use crate::AnvilState;

#[derive(Debug, PartialEq, Clone)]
pub struct DecoratedWindow {
    pub window: Window,
}

struct State {
    is_ssd: bool,
    ptr_entered_window: bool,
    header_bar: HeaderBar,
}

#[derive(Debug, PartialEq, Clone)]
struct HeaderBar {
    id: Id,
    pointer_loc: Option<Point<f64, Logical>>,
    width: u32,
    close_button_hover: bool,
    maximize_button_hover: bool,
    last_draw: (Vec<u8>, Option<Rectangle<i32, Logical>>),
}

const BG_COLOR: &[u8] = &[255, 231, 199, 255];
const MAX_COLOR: &[u8] = &[255, 246, 181, 255];
const CLOSE_COLOR: &[u8] = &[255, 169, 156, 255];
const MAX_COLOR_HOVER: &[u8] = &[181, 159, 0, 255];
const CLOSE_COLOR_HOVER: &[u8] = &[191, 28, 4, 255];

const HEADER_BAR_HEIGHT: i32 = 32;
const BUTTON_HEIGHT: u32 = HEADER_BAR_HEIGHT as u32;
const BUTTON_WIDTH: u32 = 32;

impl HeaderBar {
    fn pointer_enter(&mut self, loc: Point<f64, Logical>) {
        self.pointer_loc = Some(loc);
    }

    fn pointer_leave(&mut self) {
        self.pointer_loc = None;
    }

    fn clicked<B: crate::state::Backend>(
        &mut self,
        seat: &Seat<AnvilState<B>>,
        state: &mut AnvilState<B>,
        window: &Window,
        serial: Serial,
    ) {
        match self.pointer_loc.as_ref() {
            Some(loc) if loc.x >= (self.width - BUTTON_WIDTH) as f64 => {
                match window.toplevel() {
                    Kind::Xdg(toplevel) => toplevel.send_close(),
                    _ => {}
                };
            }
            Some(loc) if loc.x >= (self.width - (BUTTON_WIDTH * 2)) as f64 => {
                match window.toplevel() {
                    // I don't feel like rewriting the maximize request at this point, also these are custom decorations! So lets have a fullscreen button
                    Kind::Xdg(toplevel) => state.fullscreen_request(toplevel.clone(), None),
                    _ => {}
                };
            }
            Some(_) => {
                match window.toplevel() {
                    Kind::Xdg(toplevel) => {
                        let seat = seat.clone();
                        let toplevel = toplevel.clone();
                        state
                            .handle
                            .insert_idle(move |data| data.state.move_request(toplevel, &seat, serial));
                    }
                    _ => {}
                };
            }
            _ => {}
        };
    }

    fn redraw(&mut self, width: u32) {
        let buffer = &mut self.last_draw.0;
        if width == 0 {
            buffer.clear();
            self.width = 0;
            return;
        }

        let mut damage = None;
        if width != self.width {
            *buffer = vec![0; (4 * HEADER_BAR_HEIGHT as u32 * width) as usize];
            buffer.chunks_exact_mut(4).for_each(|chunk| {
                chunk.copy_from_slice(BG_COLOR);
            });
            damage = Some(Rectangle::from_loc_and_size(
                (0, 0),
                (HEADER_BAR_HEIGHT, width as i32),
            ));
            self.width = width;
        }

        let needs_redraw_buttons = damage.is_some();

        if self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || !self.close_button_hover)
        {
            buffer
                .chunks_exact_mut((width * 4) as usize)
                .flat_map(|x| {
                    x[((width - BUTTON_WIDTH) * 4) as usize..(width * 4) as usize].chunks_exact_mut(4)
                })
                .for_each(|chunk| chunk.copy_from_slice(CLOSE_COLOR_HOVER));
            self.close_button_hover = true;
            let new_damage = Rectangle::from_loc_and_size(
                ((width - BUTTON_WIDTH) as i32, 0),
                (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
            );
            damage = Some(match damage {
                Some(rect) => rect.merge(new_damage),
                None => new_damage,
            });
        } else if !self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || self.close_button_hover)
        {
            buffer
                .chunks_exact_mut((width * 4) as usize)
                .flat_map(|x| x[((width - 32) * 4) as usize..(width * 4) as usize].chunks_exact_mut(4))
                .for_each(|chunk| chunk.copy_from_slice(CLOSE_COLOR));
            self.close_button_hover = false;
            let new_damage = Rectangle::from_loc_and_size(
                ((width - BUTTON_WIDTH) as i32, 0),
                (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
            );
            damage = Some(match damage {
                Some(rect) => rect.merge(new_damage),
                None => new_damage,
            });
        }

        if self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH * 2) as f64 && l.x <= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || !self.maximize_button_hover)
        {
            buffer
                .chunks_exact_mut((width * 4) as usize)
                .flat_map(|x| {
                    x[((width - (BUTTON_WIDTH * 2)) * 4) as usize..((width - BUTTON_WIDTH) * 4) as usize]
                        .chunks_exact_mut(4)
                })
                .for_each(|chunk| chunk.copy_from_slice(MAX_COLOR_HOVER));
            self.maximize_button_hover = true;
            let new_damage = Rectangle::from_loc_and_size(
                ((width - BUTTON_WIDTH * 2) as i32, 0),
                (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
            );
            damage = Some(match damage {
                Some(rect) => rect.merge(new_damage),
                None => new_damage,
            });
        } else if !self
            .pointer_loc
            .as_ref()
            .map(|l| l.x >= (width - BUTTON_WIDTH * 2) as f64 && l.x <= (width - BUTTON_WIDTH) as f64)
            .unwrap_or(false)
            && (needs_redraw_buttons || self.maximize_button_hover)
        {
            buffer
                .chunks_exact_mut((width * 4) as usize)
                .flat_map(|x| {
                    x[((width - (BUTTON_WIDTH * 2)) * 4) as usize..((width - BUTTON_WIDTH) * 4) as usize]
                        .chunks_exact_mut(4)
                })
                .for_each(|chunk| chunk.copy_from_slice(MAX_COLOR));
            self.maximize_button_hover = false;
            let new_damage = Rectangle::from_loc_and_size(
                ((width - BUTTON_WIDTH * 2) as i32, 0),
                (BUTTON_WIDTH as i32, BUTTON_HEIGHT as i32),
            );
            damage = Some(match damage {
                Some(rect) => rect.merge(new_damage),
                None => new_damage,
            });
        }

        self.last_draw.1 = damage;
    }
}

impl DecoratedWindow {
    fn decoration_state(&self) -> RefMut<'_, State> {
        self.window.user_data().insert_if_missing(|| {
            RefCell::new(State {
                is_ssd: false,
                ptr_entered_window: false,
                header_bar: HeaderBar {
                    id: Id::new(),
                    pointer_loc: None,
                    width: 128,
                    close_button_hover: false,
                    maximize_button_hover: false,
                    last_draw: (Vec::new(), None),
                },
            })
        });

        self.window
            .user_data()
            .get::<RefCell<State>>()
            .unwrap()
            .borrow_mut()
    }

    pub fn set_ssd(&self, ssd: bool) {
        self.decoration_state().is_ssd = ssd;
    }
}

impl IsAlive for DecoratedWindow {
    fn alive(&self) -> bool {
        self.window.alive()
    }
}

impl<Backend: crate::state::Backend> PointerTarget<AnvilState<Backend>> for DecoratedWindow {
    fn enter(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, event: &MotionEvent) {
        let mut state = self.decoration_state();
        if state.is_ssd {
            if event.location.y < HEADER_BAR_HEIGHT as f64 {
                state.header_bar.pointer_enter(event.location);
            } else {
                state.header_bar.pointer_leave();
                let mut event = event.clone();
                event.location.y -= HEADER_BAR_HEIGHT as f64;
                PointerTarget::<AnvilState<Backend>>::enter(&self.window, seat, data, &event);
                state.ptr_entered_window = true;
            }
        } else {
            state.ptr_entered_window = true;
            PointerTarget::<AnvilState<Backend>>::enter(&self.window, seat, data, event)
        }
    }
    fn motion(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, event: &MotionEvent) {
        let mut state = self.decoration_state();
        if state.is_ssd {
            if event.location.y < HEADER_BAR_HEIGHT as f64 {
                PointerTarget::<AnvilState<Backend>>::leave(
                    &self.window,
                    seat,
                    data,
                    event.serial,
                    event.time,
                );
                state.ptr_entered_window = false;
                state.header_bar.pointer_enter(event.location);
            } else {
                state.header_bar.pointer_leave();
                let mut event = event.clone();
                event.location.y -= HEADER_BAR_HEIGHT as f64;
                if state.ptr_entered_window {
                    PointerTarget::<AnvilState<Backend>>::motion(&self.window, seat, data, &event)
                } else {
                    state.ptr_entered_window = true;
                    PointerTarget::<AnvilState<Backend>>::enter(&self.window, seat, data, &event)
                }
            }
        } else {
            PointerTarget::<AnvilState<Backend>>::motion(&self.window, seat, data, event)
        }
    }
    fn button(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, event: &ButtonEvent) {
        let mut state = self.decoration_state();
        if state.is_ssd {
            if state.ptr_entered_window {
                PointerTarget::<AnvilState<Backend>>::button(&self.window, seat, data, event)
            } else {
                state.header_bar.clicked(seat, data, &self.window, event.serial);
            }
        } else {
            PointerTarget::<AnvilState<Backend>>::button(&self.window, seat, data, event)
        }
    }
    fn axis(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, frame: AxisFrame) {
        let state = self.decoration_state();
        if !state.is_ssd || state.ptr_entered_window {
            PointerTarget::<AnvilState<Backend>>::axis(&self.window, seat, data, frame)
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
                PointerTarget::<AnvilState<Backend>>::leave(&self.window, seat, data, serial, time);
                state.ptr_entered_window = false;
            }
        } else {
            PointerTarget::<AnvilState<Backend>>::leave(&self.window, seat, data, serial, time);
            state.ptr_entered_window = false;
        }
    }
}

impl<Backend: crate::state::Backend> KeyboardTarget<AnvilState<Backend>> for DecoratedWindow {
    fn enter(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        KeyboardTarget::<AnvilState<Backend>>::enter(&self.window, seat, data, keys, serial)
    }
    fn leave(&self, seat: &Seat<AnvilState<Backend>>, data: &mut AnvilState<Backend>, serial: Serial) {
        KeyboardTarget::<AnvilState<Backend>>::leave(&self.window, seat, data, serial)
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
        KeyboardTarget::<AnvilState<Backend>>::key(&self.window, seat, data, key, state, serial, time)
    }
    fn modifiers(
        &self,
        seat: &Seat<AnvilState<Backend>>,
        data: &mut AnvilState<Backend>,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        KeyboardTarget::<AnvilState<Backend>>::modifiers(&self.window, seat, data, modifiers, serial)
    }
}

impl SpaceElement for DecoratedWindow {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = SpaceElement::geometry(&self.window);
        geo.size.h += HEADER_BAR_HEIGHT;
        geo
    }
    fn bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = SpaceElement::bbox(&self.window);
        bbox.size.h += HEADER_BAR_HEIGHT;
        bbox
    }
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        point.y < HEADER_BAR_HEIGHT as f64
            || self
                .window
                .is_in_input_region(&(*point - Point::from((0.0, 32.0))))
    }
    fn z_index(&self) -> u8 {
        self.window.z_index()
    }

    fn set_activate(&self, activated: bool) {
        self.window.set_activate(activated)
    }
    fn output_enter(&self, output: &Output) {
        self.window.output_enter(output)
    }
    fn output_leave(&self, output: &Output) {
        self.window.output_leave(output)
    }
    fn refresh(&self) {
        self.window.refresh()
    }
}

render_elements!(
    pub DecoratedWindowElements<R>;
    Window=WaylandSurfaceRenderElement,
    Decoration=HeaderBarElement,
);

pub struct HeaderBarElement {
    window: DecoratedWindow,
    id: Id,
    location: Point<i32, Physical>,
    scale: Scale<f64>,
}

impl HeaderBarElement {
    fn new(window: &DecoratedWindow, location: Point<i32, Physical>, scale: Scale<f64>) -> HeaderBarElement {
        let width = window.window.geometry().size.w;
        window.decoration_state().header_bar.redraw(width as u32);
        HeaderBarElement {
            window: window.clone(),
            id: Id::new(),
            location,
            scale,
        }
    }
}

impl<R> RenderElement<R> for HeaderBarElement
where
    R: Renderer + ImportMem,
    <R as Renderer>::TextureId: 'static,
{
    fn id(&self) -> &Id {
        &self.id
    }
    fn current_commit(&self) -> usize {
        1
    }
    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.location
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let width = (SpaceElement::bbox(&self.window.window).size.w as f64 * scale.x) as i32;
        Rectangle::from_loc_and_size((0, 0), (width, (HEADER_BAR_HEIGHT as f64 * scale.y) as i32))
    }
    fn damage_since(&self, scale: Scale<f64>, _commit: Option<usize>) -> Vec<Rectangle<i32, Physical>> {
        vec![Rectangle::from_loc_and_size(
            (0, 0),
            RenderElement::<R>::geometry(self, scale).size,
        )]
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        let width = (SpaceElement::bbox(&self.window.window).size.w as f64 * scale.x) as i32;
        vec![Rectangle::from_loc_and_size((0, 0), (width, HEADER_BAR_HEIGHT))]
    }
    fn underlying_storage(&self, _renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        None
    }
    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        _log: &slog::Logger,
    ) -> Result<(), R::Error> {
        let user_data = self.window.window.user_data();
        user_data.insert_if_missing(|| RefCell::new(HashMap::<usize, R::TextureId>::new()));

        let state = self.window.decoration_state();
        let (ref buffer, ref import_damage) = &state.header_bar.last_draw;
        let size: Size<i32, Logical> = (state.header_bar.width as i32, HEADER_BAR_HEIGHT).into();

        if state.header_bar.width == 0 {
            return Ok(());
        }

        let mut map = user_data
            .get::<RefCell<HashMap<usize, R::TextureId>>>()
            .unwrap()
            .borrow_mut();
        let tex = map
            .entry(renderer.id())
            .and_modify(|tex| {
                if let Some(region) = import_damage {
                    renderer
                        .update_memory(tex, buffer, dbg!(region.to_buffer(1, Transform::Normal, &size)))
                        .unwrap();
                }
            })
            .or_insert_with(|| {
                renderer
                    .import_memory(buffer, size.to_buffer(1, Transform::Normal), false)
                    .unwrap()
            });
        frame.render_texture_at(tex, self.location, 1, scale, Transform::Normal, damage, 1.0)
    }
}

impl<'a, R> AsRenderElements<R> for DecoratedWindow
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Texture + 'static,
{
    type RenderElement = DecoratedWindowElements<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<C> {
        if self.decoration_state().is_ssd {
            let header_bar = HeaderBarElement::new(self, location, scale);

            let mut location = location.clone();
            location.y += (scale.y * HEADER_BAR_HEIGHT as f64) as i32;

            let mut vec = AsRenderElements::<R>::render_elements::<DecoratedWindowElements<R>>(
                &self.window,
                location,
                scale,
            );
            vec.push(DecoratedWindowElements::Decoration(header_bar));
            vec.into_iter().map(C::from).collect()
        } else {
            AsRenderElements::<R>::render_elements::<DecoratedWindowElements<R>>(
                &self.window,
                location,
                scale,
            )
            .into_iter()
            .map(C::from)
            .collect()
        }
    }
}
