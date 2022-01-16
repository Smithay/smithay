use crate::{
    backend::renderer::{Frame, ImportAll, Renderer, Texture},
    desktop::{
        layer::LayerSurface,
        popup::{PopupKind, PopupManager},
        space::{window_loc, Space, SpaceElement},
        utils::{bbox_from_surface_tree, damage_from_surface_tree},
        window::Window,
    },
    utils::{Logical, Point, Rectangle},
    wayland::output::Output,
};
use std::any::TypeId;

#[derive(Debug)]
pub struct RenderPopup {
    location: Point<i32, Logical>,
    popup: PopupKind,
}

impl Window {
    pub(super) fn popup_elements<R>(&self, space_id: usize) -> impl Iterator<Item = RenderPopup>
    where
        R: Renderer + ImportAll + 'static,
        R::TextureId: 'static,
        R::Error: 'static,
        R::Frame: 'static,
    {
        let loc = window_loc(self, &space_id) + self.geometry().loc;
        self.toplevel()
            .get_surface()
            .map(move |surface| {
                PopupManager::popups_for_surface(surface)
                    .ok()
                    .into_iter()
                    .flatten()
                    .map(move |(popup, location)| {
                        let offset = loc + location - popup.geometry().loc;
                        RenderPopup {
                            location: offset,
                            popup,
                        }
                    })
            })
            .into_iter()
            .flatten()
    }
}

impl LayerSurface {
    pub(super) fn popup_elements<R>(&self, space_id: usize) -> impl Iterator<Item = RenderPopup>
    where
        R: Renderer + ImportAll + 'static,
        R::TextureId: 'static,
        R::Error: 'static,
        R::Frame: 'static,
    {
        type SpaceElem<R> =
            dyn SpaceElement<R, <R as Renderer>::Frame, <R as Renderer>::Error, <R as Renderer>::TextureId>;

        let loc = (self as &SpaceElem<R>).geometry(space_id).loc;
        self.get_surface()
            .map(move |surface| {
                PopupManager::popups_for_surface(surface)
                    .ok()
                    .into_iter()
                    .flatten()
                    .map(move |(popup, location)| {
                        let offset = loc + location - popup.geometry().loc;
                        RenderPopup {
                            location: offset,
                            popup,
                        }
                    })
            })
            .into_iter()
            .flatten()
    }
}

impl<R, F, E, T> SpaceElement<R, F, E, T> for RenderPopup
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
{
    fn id(&self) -> usize {
        self.popup
            .get_surface()
            .map(|s| s.as_ref().id() as usize)
            .unwrap_or(0)
    }

    fn type_of(&self) -> TypeId {
        TypeId::of::<RenderPopup>()
    }

    fn geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        if let Some(surface) = self.popup.get_surface() {
            bbox_from_surface_tree(surface, self.location)
        } else {
            Rectangle::from_loc_and_size((0, 0), (0, 0))
        }
    }

    fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>> {
        if let Some(surface) = self.popup.get_surface() {
            damage_from_surface_tree(surface, (0, 0), for_values)
        } else {
            Vec::new()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        _space_id: usize,
        _renderer: &mut R,
        _frame: &mut F,
        _scale: f64,
        _location: Point<i32, Logical>,
        _damage: &[Rectangle<i32, Logical>],
        _log: &slog::Logger,
    ) -> Result<(), R::Error> {
        // popups are special, we track them, but they render with their parents
        Ok(())
    }
}
