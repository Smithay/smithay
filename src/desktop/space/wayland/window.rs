use crate::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            AsRenderElements,
        },
        utils::RendererSurfaceStateUserData,
        ImportAll, Renderer,
    },
    desktop::{space::SpaceElement, PopupManager, Window, WindowSurfaceType},
    output::{Output, WeakOutput},
    utils::{Logical, Physical, Point, Rectangle, Scale},
    wayland::compositor::{with_surface_tree_downward, TraversalAction},
};
use wayland_server::{protocol::wl_surface::WlSurface, Resource, Weak as WlWeak};

use std::{
    cell::{RefCell, RefMut},
    collections::{HashMap, HashSet},
};

type OutputSurfacesUserdata = RefCell<HashSet<WlWeak<WlSurface>>>;
fn output_surfaces(o: &Output) -> RefMut<'_, HashSet<WlWeak<WlSurface>>> {
    let userdata = o.user_data();
    userdata.insert_if_missing(OutputSurfacesUserdata::default);
    let mut surfaces = userdata.get::<OutputSurfacesUserdata>().unwrap().borrow_mut();
    surfaces.retain(|s| s.upgrade().is_ok());
    surfaces
}

fn output_update(
    output: &Output,
    output_overlap: Rectangle<i32, Logical>,
    surface: &WlSurface,
    logger: &slog::Logger,
) {
    let mut surface_list = output_surfaces(output);

    with_surface_tree_downward(
        surface,
        (Point::from((0, 0)), false),
        |_, states, (location, parent_unmapped)| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            // If the parent is unmapped we still have to traverse
            // our children to send a leave events
            if *parent_unmapped {
                TraversalAction::DoChildren((location, true))
            } else if let Some(surface_view) = data.and_then(|d| d.borrow().surface_view) {
                location += surface_view.offset;
                TraversalAction::DoChildren((location, false))
            } else {
                // If we are unmapped we still have to traverse
                // our children to send leave events
                TraversalAction::DoChildren((location, true))
            }
        },
        |wl_surface, states, (location, parent_unmapped)| {
            let mut location = *location;

            if *parent_unmapped {
                // The parent is unmapped, just send a leave event
                // if we were previously mapped and exit early
                output_leave(output, &mut surface_list, wl_surface, logger);
                return;
            }
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(surface_view) = data.and_then(|d| d.borrow().surface_view) {
                location += surface_view.offset;
                let surface_rectangle = Rectangle::from_loc_and_size(location, surface_view.dst);
                if output_overlap.overlaps(surface_rectangle) {
                    // We found a matching output, check if we already sent enter
                    output_enter(output, &mut surface_list, wl_surface, logger);
                } else {
                    // Surface does not match output, if we sent enter earlier
                    // we should now send leave
                    output_leave(output, &mut surface_list, wl_surface, logger);
                }
            } else {
                // Maybe the the surface got unmapped, send leave on output
                output_leave(output, &mut surface_list, wl_surface, logger);
            }
        },
        |_, _, _| true,
    );
}

fn output_enter(
    output: &Output,
    surface_list: &mut HashSet<WlWeak<WlSurface>>,
    surface: &WlSurface,
    logger: &slog::Logger,
) {
    let weak = surface.downgrade();
    if !surface_list.contains(&weak) {
        slog::debug!(
            logger,
            "surface ({:?}) entering output {:?}",
            surface,
            output.name()
        );
        output.enter(surface);
        surface_list.insert(weak);
    }
}

fn output_leave(
    output: &Output,
    surface_list: &mut HashSet<WlWeak<WlSurface>>,
    surface: &WlSurface,
    logger: &slog::Logger,
) {
    let weak = surface.downgrade();
    if surface_list.contains(&weak) {
        slog::debug!(
            logger,
            "surface ({:?}) leaving output {:?}",
            surface,
            output.name()
        );
        output.leave(surface);
        surface_list.remove(&weak);
    }
}

#[derive(Debug, Default)]
struct WindowOutputState {
    output_overlap: HashMap<WeakOutput, Rectangle<i32, Logical>>,
}

type WindowOutputUserData = RefCell<WindowOutputState>;

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
        with_surface_tree_downward(
            self.toplevel().wl_surface(),
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |wl_surface, _, _| {
                output_leave(
                    output,
                    &mut surface_list,
                    wl_surface,
                    &crate::slog_or_fallback(None),
                );
            },
            |_, _, _| true,
        );
        for (popup, _) in PopupManager::popups_for_surface(self.toplevel().wl_surface()) {
            with_surface_tree_downward(
                popup.wl_surface(),
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |wl_surface, _, _| {
                    output_leave(
                        output,
                        &mut surface_list,
                        wl_surface,
                        &crate::slog_or_fallback(None),
                    );
                },
                |_, _, _| true,
            );
        }
    }

    fn refresh(&self) {
        self.user_data().insert_if_missing(WindowOutputUserData::default);
        let state = self.user_data().get::<WindowOutputUserData>().unwrap().borrow();

        for (weak, overlap) in state.output_overlap.iter() {
            if let Some(output) = weak.upgrade() {
                output_update(
                    &output,
                    *overlap,
                    self.toplevel().wl_surface(),
                    &crate::slog_or_fallback(None),
                );
                for (popup, location) in PopupManager::popups_for_surface(self.toplevel().wl_surface()) {
                    let mut overlap = *overlap;
                    overlap.loc -= location;
                    output_update(
                        &output,
                        overlap,
                        popup.wl_surface(),
                        &crate::slog_or_fallback(None),
                    );
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
    type RenderElement = WaylandSurfaceRenderElement;

    fn render_elements<C: From<WaylandSurfaceRenderElement>>(
        &self,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<C> {
        let surface = self.toplevel().wl_surface();

        let mut render_elements: Vec<C> = Vec::new();
        let popup_render_elements =
            PopupManager::popups_for_surface(surface).flat_map(|(popup, popup_offset)| {
                let offset = (self.geometry().loc + popup_offset - popup.geometry().loc)
                    .to_physical_precise_round(scale);

                render_elements_from_surface_tree(popup.wl_surface(), location + offset, scale)
            });

        render_elements.extend(popup_render_elements);

        render_elements.extend(render_elements_from_surface_tree(surface, location, scale));

        render_elements
    }
}
