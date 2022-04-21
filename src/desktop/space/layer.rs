use wayland_server::DisplayHandle;

use crate::{
    backend::renderer::{ImportAll, Renderer},
    desktop::{
        layer::{layer_state as output_layer_state, *},
        space::Space,
    },
    utils::{Logical, Point, Rectangle},
    wayland::{output::Output, shell::wlr_layer::Layer},
};
use std::{
    any::TypeId,
    cell::{RefCell, RefMut},
    collections::HashMap,
};

use super::RenderZindex;

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

impl LayerSurface {
    pub(super) fn elem_id(&self) -> usize {
        self.0.id
    }

    pub(super) fn elem_type_of(&self) -> TypeId {
        TypeId::of::<LayerSurface>()
    }

    pub(super) fn elem_geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        let mut bbox = self.bbox_with_popups();
        let state = output_layer_state(self);
        bbox.loc += state.location;
        bbox
    }

    pub(super) fn elem_accumulated_damage(
        &self,
        for_values: Option<(&Space, &Output)>,
    ) -> Vec<Rectangle<i32, Logical>> {
        self.accumulated_damage(for_values)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn elem_draw<R>(
        &self,
        dh: &mut DisplayHandle<'_>,
        space_id: usize,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error>
    where
        R: Renderer + ImportAll,
        <R as Renderer>::TextureId: 'static,
    {
        let res = draw_layer_surface(dh, renderer, frame, self, scale, location, damage, log);
        if res.is_ok() {
            layer_state(space_id, self).drawn = true;
        }
        res
    }

    pub(super) fn elem_z_index(&self) -> u8 {
        let layer = self.layer();
        let z_index = match layer {
            Layer::Background => RenderZindex::Background,
            Layer::Bottom => RenderZindex::Bottom,
            Layer::Top => RenderZindex::Top,
            Layer::Overlay => RenderZindex::Overlay,
        };
        z_index as u8
    }
}
