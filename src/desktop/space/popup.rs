use crate::{
    backend::renderer::{utils::draw_surface_tree, ImportAll, Renderer},
    desktop::{
        layer::LayerSurface,
        popup::{PopupKind, PopupManager},
        space::Space,
        utils::{bbox_from_surface_tree, damage_from_surface_tree},
        window::Window,
    },
    utils::{Logical, Point, Rectangle},
    wayland::{output::Output, shell::wlr_layer::Layer},
};
use std::any::TypeId;

use super::RenderZindex;

#[derive(Debug)]
pub struct RenderPopup {
    location: Point<i32, Logical>,
    popup: PopupKind,
    z_index: u8,
}

impl Window {
    pub(super) fn popup_elements(&self, space_id: usize) -> impl Iterator<Item = RenderPopup> {
        let loc = self.elem_location(space_id);
        let geo_loc = self.geometry().loc;
        self.toplevel()
            .get_surface()
            .map(move |surface| {
                PopupManager::popups_for_surface(surface)
                    .ok()
                    .into_iter()
                    .flatten()
                    .map(move |(popup, location)| {
                        let offset = loc + geo_loc + location - popup.geometry().loc;
                        RenderPopup {
                            location: offset,
                            popup,
                            z_index: RenderZindex::Popups as u8,
                        }
                    })
            })
            .into_iter()
            .flatten()
    }
}

impl LayerSurface {
    pub(super) fn popup_elements(&self, space_id: usize) -> impl Iterator<Item = RenderPopup> + '_ {
        let loc = self.elem_geometry(space_id).loc;
        self.get_surface()
            .map(move |surface| {
                PopupManager::popups_for_surface(surface)
                    .ok()
                    .into_iter()
                    .flatten()
                    .map(move |(popup, location)| {
                        let offset = loc + location - popup.geometry().loc;
                        let z_index = if let Some(layer) = self.layer() {
                            if layer == Layer::Overlay {
                                RenderZindex::PopupsOverlay as u8
                            } else {
                                RenderZindex::Popups as u8
                            }
                        } else {
                            0
                        };

                        RenderPopup {
                            location: offset,
                            popup,
                            z_index,
                        }
                    })
            })
            .into_iter()
            .flatten()
    }
}

impl RenderPopup {
    pub(super) fn elem_id(&self) -> usize {
        self.popup
            .get_surface()
            .map(|s| s.as_ref().id() as usize)
            .unwrap_or(0)
    }

    pub(super) fn elem_type_of(&self) -> TypeId {
        TypeId::of::<RenderPopup>()
    }

    pub(super) fn elem_geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        if let Some(surface) = self.popup.get_surface() {
            bbox_from_surface_tree(surface, self.location)
        } else {
            Rectangle::from_loc_and_size((0, 0), (0, 0))
        }
    }

    pub(super) fn elem_accumulated_damage(
        &self,
        for_values: Option<(&Space, &Output)>,
    ) -> Vec<Rectangle<i32, Logical>> {
        if let Some(surface) = self.popup.get_surface() {
            damage_from_surface_tree(surface, (0, 0), for_values)
        } else {
            Vec::new()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn elem_draw<R>(
        &self,
        _space_id: usize,
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
        if let Some(surface) = self.popup.get_surface() {
            draw_surface_tree(renderer, frame, surface, scale, location, damage, log)?;
        }
        Ok(())
    }

    pub(super) fn elem_z_index(&self) -> u8 {
        self.z_index
    }
}
