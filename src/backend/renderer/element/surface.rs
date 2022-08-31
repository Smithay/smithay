//! TODO: Docs

use wayland_server::protocol::wl_surface;

use crate::{
    backend::renderer::{utils::RendererSurfaceStateUserData, Frame, ImportAll, Renderer, Texture},
    utils::{Physical, Point, Rectangle, Scale, Size},
    wayland::compositor::{self, TraversalAction},
};

use super::{CommitCounter, Id, RenderElement, UnderlyingStorage};

/// Retrieve the render surfaces for a surface tree
pub fn render_elements_from_surface_tree<E>(
    surface: &wl_surface::WlSurface,
    location: impl Into<Point<i32, Physical>>,
    scale: impl Into<Scale<f64>>,
) -> Vec<E>
where
    E: From<WaylandSurfaceRenderElement>,
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

                    let surface = WaylandSurfaceRenderElement::from_surface(surface, location);
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
pub struct WaylandSurfaceRenderElement {
    id: Id,
    location: Point<f64, Physical>,
    surface: wl_surface::WlSurface,
}

impl WaylandSurfaceRenderElement {
    /// Create a render element from a surface
    pub fn from_surface(surface: &wl_surface::WlSurface, location: Point<f64, Physical>) -> Self {
        let id = Id::from_wayland_resource(surface);

        Self {
            id,
            location,
            surface: surface.clone(),
        }
    }

    fn size(&self, scale: impl Into<Scale<f64>>) -> Size<i32, Physical> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.and_then(|d| d.borrow().view()).map(|surface_view| {
                ((surface_view.dst.to_f64().to_physical(scale).to_point() + self.location).to_i32_round()
                    - self.location.to_i32_round())
                .to_size()
            })
        })
        .unwrap_or_default()
    }
}

impl<R> RenderElement<R> for WaylandSurfaceRenderElement
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
{
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| d.borrow().current_commit())
        })
        .unwrap_or_default()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(self.location.to_i32_round(), self.size(scale))
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let dst_size = self.size(scale);

        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.and_then(|d| {
                let data = d.borrow();
                if let Some(surface_view) = data.view() {
                    let damage = data
                        .damage_since(commit)
                        .iter()
                        .filter_map(|rect| {
                            rect.to_f64()
                                // first bring the damage into logical space
                                // Note: We use f64 for this as the damage could
                                // be not dividable by the buffer scale without
                                // a rest
                                .to_logical(
                                    data.buffer_scale as f64,
                                    data.buffer_transform,
                                    &data.buffer_dimensions.unwrap().to_f64(),
                                )
                                // then crop by the surface view (viewporter for example could define a src rect)
                                .intersection(surface_view.src)
                                // move and scale the cropped rect (viewporter could define a dst size)
                                .map(|rect| surface_view.rect_to_global(rect).to_i32_up::<i32>())
                                // now bring the damage to physical space
                                .map(|rect| {
                                    // We calculate the scale between to rounded
                                    // surface size and the scaled surface size
                                    // and use it to scale the damage to the rounded
                                    // surface size by multiplying the output scale
                                    // with the result.
                                    let surface_scale =
                                        dst_size.to_f64() / surface_view.dst.to_f64().to_physical(scale);
                                    rect.to_physical_precise_up(surface_scale * scale)
                                })
                        })
                        .collect::<Vec<_>>();

                    Some(damage)
                } else {
                    None
                }
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

        let dst_size = self.size(scale);
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            if let Some(data) = data {
                let data = data.borrow();

                if let Some(texture) = data.texture(renderer) {
                    if let Some(surface_view) = data.view() {
                        let src = surface_view.src.to_buffer(
                            data.buffer_scale as f64,
                            data.buffer_transform,
                            &data.buffer_size().unwrap().to_f64(),
                        );

                        let dst = Rectangle::from_loc_and_size(self.location.to_i32_round(), dst_size);

                        frame.render_texture_from_to(
                            texture,
                            src,
                            dst,
                            damage,
                            data.buffer_transform,
                            1.0f32,
                        )?;
                    }
                }
            }

            Ok(())
        })
    }
}
