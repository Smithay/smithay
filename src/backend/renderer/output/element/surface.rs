use std::marker::PhantomData;

use wayland_server::protocol::wl_surface;

use crate::{
    backend::renderer::{utils::RendererSurfaceStateUserData, Frame, ImportAll, Renderer, Texture},
    utils::{Physical, Point, Rectangle, Scale},
    wayland::compositor::{self, TraversalAction},
};

use super::{Id, RenderElement, UnderlyingStorage};

/// Retrieve the render surfaces for a surface tree
pub fn surfaces_from_surface_tree<R, E>(
    surface: &wl_surface::WlSurface,
    location: impl Into<Point<i32, Physical>>,
    scale: impl Into<Scale<f64>>,
) -> Vec<E>
where
    E: From<WaylandSurfaceRenderElement<R>>,
{
    let location = location.into().to_f64();
    let scale = scale.into();
    let mut surfaces: Vec<E> = Vec::new();

    compositor::with_surface_tree_downward(
        surface,
        location,
        |_, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let data = &*data.borrow();

                if let Some(view) = data.view() {
                    location += view.offset.to_f64().to_physical(scale);
                    TraversalAction::DoChildren(location)
                } else {
                    TraversalAction::SkipChildren
                }
            } else {
                TraversalAction::SkipChildren
            }
        },
        |surface, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let data = &*data.borrow();

                if let Some(view) = data.view() {
                    location += view.offset.to_f64().to_physical(scale);

                    let surface = WaylandSurfaceRenderElement::from_surface(surface, location.to_i32_round());
                    surfaces.push(surface.into());
                }
            }
        },
        |_, _, _| true,
    );

    surfaces
}

/// A single surface render element
#[derive(Debug)]
pub struct WaylandSurfaceRenderElement<R> {
    id: Id,
    location: Point<i32, Physical>,
    surface: wl_surface::WlSurface,
    _phantom: PhantomData<R>,
}

impl<R> WaylandSurfaceRenderElement<R> {
    /// Create a render element from a surface
    pub fn from_surface(surface: &wl_surface::WlSurface, location: Point<i32, Physical>) -> Self {
        let id = Id::from_wayland_resource(surface);

        Self {
            id,
            location,
            surface: surface.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<R> RenderElement<R> for WaylandSurfaceRenderElement<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
{
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> usize {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| d.borrow().current_commit()).unwrap()
        })
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.location
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| d.borrow().view().unwrap().dst)
                .map(|d| Rectangle::from_loc_and_size(self.location, d.to_physical_precise_round(scale)))
                .unwrap()
        })
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<usize>) -> Vec<Rectangle<i32, Physical>> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| {
                let data = d.borrow();
                data.damage_since(commit)
                    .iter()
                    .map(|d| {
                        d.to_f64()
                            .to_logical(
                                data.buffer_scale as f64,
                                data.buffer_transform,
                                &data.buffer_dimensions.unwrap().to_f64(),
                            )
                            .to_physical(scale)
                            .to_i32_up()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        })
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| {
                let data = d.borrow();
                data.opaque_regions()
                    .map(|r| {
                        r.iter()
                            .map(|r| r.to_physical_precise_up(scale))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
        })
    }

    fn underlying_storage(&self, _renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.and_then(|d| d.borrow().wl_buffer().cloned())
                .map(|b| UnderlyingStorage::Wayland(b))
        })
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        crate::backend::renderer::utils::import_surface_tree(renderer, &self.surface, log)?;

        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            if let Some(data) = data {
                let data = data.borrow();
                frame.render_texture_at(
                    data.texture(renderer).unwrap(),
                    self.location,
                    data.buffer_scale,
                    scale,
                    data.buffer_transform,
                    damage,
                    1.0f32,
                )
            } else {
                Ok(())
            }
        })
    }
}
