use crate::{
    backend::renderer::{Frame, ImportAll, Renderer, Texture},
    desktop::{
        layer::{layer_state as output_layer_state, *},
        space::{Space, SpaceElement},
    },
    utils::{Logical, Point, Rectangle},
    wayland::output::Output,
};
use std::{
    any::TypeId,
    cell::{RefCell, RefMut},
    collections::HashMap,
};

#[derive(Default)]
pub struct LayerState {
    pub drawn: bool,
}

type LayerUserdata = RefCell<HashMap<usize, LayerState>>;
pub fn layer_state(space: usize, l: &LayerSurface) -> RefMut<'_, LayerState> {
    let userdata = l.user_data();
    userdata.insert_if_missing(LayerUserdata::default);
    RefMut::map(userdata.get::<LayerUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}

impl<R, F, E, T> SpaceElement<R, F, E, T> for LayerSurface
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
{
    fn id(&self) -> usize {
        self.0.id
    }

    fn type_of(&self) -> TypeId {
        TypeId::of::<LayerSurface>()
    }

    fn geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        let mut bbox = self.bbox_with_popups();
        let state = output_layer_state(self);
        bbox.loc += state.location;
        bbox
    }

    fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>> {
        self.accumulated_damage(for_values)
    }

    fn draw(
        &self,
        space_id: usize,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        let res = draw_layer_surface(renderer, frame, self, scale, location, damage, log);
        if res.is_ok() {
            layer_state(space_id, self).drawn = true;
        }
        res
    }
}
