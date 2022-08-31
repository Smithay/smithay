use crate::{
    backend::renderer::{ImportAll, Renderer},
    desktop::{
        layer::{layer_state as output_layer_state, *},
        space::Space,
    },
    output::Output,
    utils::{Physical, Point, Rectangle, Scale},
    wayland::shell::wlr_layer::Layer,
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

    pub(super) fn elem_location(
        &self,
        _space_id: usize,
        scale: impl Into<Scale<f64>>,
    ) -> Point<f64, Physical> {
        let state = output_layer_state(self);
        state.location.to_f64().to_physical(scale)
    }

    pub(super) fn elem_geometry(
        &self,
        _space_id: usize,
        scale: impl Into<Scale<f64>>,
    ) -> Rectangle<i32, Physical> {
        let scale = scale.into();
        let state = output_layer_state(self);
        self.physical_bbox_with_popups(state.location.to_f64().to_physical(scale), scale)
    }

    pub(super) fn elem_accumulated_damage(
        &self,
        _space_id: usize,
        scale: impl Into<Scale<f64>>,
        for_values: Option<(&Space, &Output)>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let scale = scale.into();
        let state = output_layer_state(self);
        self.accumulated_damage(state.location.to_f64().to_physical(scale), scale, for_values)
    }

    pub(super) fn elem_opaque_regions(
        &self,
        _space_id: usize,
        scale: impl Into<Scale<f64>>,
    ) -> Option<Vec<Rectangle<i32, Physical>>> {
        let scale = scale.into();
        let state = output_layer_state(self);
        self.opaque_regions(state.location.to_f64().to_physical(scale), scale)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn elem_draw<R>(
        &self,
        space_id: usize,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: impl Into<Scale<f64>>,
        location: Point<f64, Physical>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error>
    where
        R: Renderer + ImportAll,
        <R as Renderer>::TextureId: 'static,
    {
        let res = draw_layer_surface(renderer, frame, self, scale, location, damage, log);
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
