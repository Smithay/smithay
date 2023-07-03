use crate::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            AsRenderElements,
        },
        ImportAll, Renderer,
    },
    desktop::{space::SpaceElement, PopupManager, Window, WindowSurfaceType},
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale},
    wayland::compositor::{with_surface_tree_downward, TraversalAction},
};

use super::{output_leave, output_surfaces, output_update, WindowOutputUserData};

impl SpaceElement for Window {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        self.geometry()
    }

    fn bbox(&self) -> Rectangle<i32, Logical> {
        self.bbox_with_popups()
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        self.surface_under(*point, WindowSurfaceType::ALL).is_some()
    }

    fn z_index(&self) -> u8 {
        self.0.z_index.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn set_activate(&self, activated: bool) {
        self.set_activated(activated);
    }
    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        self.user_data().insert_if_missing(WindowOutputUserData::default);
        {
            let mut state = self
                .user_data()
                .get::<WindowOutputUserData>()
                .unwrap()
                .borrow_mut();
            state.output_overlap.insert(output.downgrade(), overlap);
            state.output_overlap.retain(|weak, _| weak.upgrade().is_some());
        }
        self.refresh()
    }
    fn output_leave(&self, output: &Output) {
        if let Some(state) = self.user_data().get::<WindowOutputUserData>() {
            state.borrow_mut().output_overlap.retain(|weak, _| weak != output);
        }

        let mut surface_list = output_surfaces(output);
        let surface = self.toplevel().wl_surface();
        with_surface_tree_downward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |wl_surface, _, _| {
                output_leave(output, &mut surface_list, wl_surface);
            },
            |_, _, _| true,
        );
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            with_surface_tree_downward(
                popup.wl_surface(),
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |wl_surface, _, _| {
                    output_leave(output, &mut surface_list, wl_surface);
                },
                |_, _, _| true,
            );
        }
    }

    fn refresh(&self) {
        self.user_data().insert_if_missing(WindowOutputUserData::default);
        let state = self.user_data().get::<WindowOutputUserData>().unwrap().borrow();

        let surface = self.toplevel().wl_surface();
        for (weak, overlap) in state.output_overlap.iter() {
            if let Some(output) = weak.upgrade() {
                output_update(&output, *overlap, surface);
                for (popup, location) in PopupManager::popups_for_surface(surface) {
                    let mut overlap = *overlap;
                    overlap.loc -= location;
                    output_update(&output, overlap, popup.wl_surface());
                }
            }
        }
    }
}

impl<R> AsRenderElements<R> for Window
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;

    #[profiling::function]
    fn render_elements<C: From<WaylandSurfaceRenderElement<R>>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        let surface = self.toplevel().wl_surface();

        let mut render_elements: Vec<C> = Vec::new();
        let popup_render_elements =
            PopupManager::popups_for_surface(surface).flat_map(|(popup, popup_offset)| {
                let offset = (self.geometry().loc + popup_offset - popup.geometry().loc)
                    .to_physical_precise_round(scale);

                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    location + offset,
                    scale,
                    alpha,
                )
            });

        render_elements.extend(popup_render_elements);

        render_elements.extend(render_elements_from_surface_tree(
            renderer, surface, location, scale, alpha,
        ));

        render_elements
    }
}
