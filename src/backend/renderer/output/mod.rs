use std::collections::VecDeque;

use indexmap::IndexMap;

use crate::{
    backend::renderer::Frame,
    utils::{Physical, Rectangle, Scale, Size},
    wayland::output::Output,
};

use self::element::{Id, RenderElement};

use super::{ImportAll, Renderer, Texture};

pub mod element;

#[derive(Debug, Clone, Copy)]
struct ElementState {
    last_commit: usize,
    last_geometry: Rectangle<i32, Physical>,
    last_z_index: usize,
}

#[derive(Debug)]
struct OutputRenderState {
    size: Option<Size<i32, Physical>>,
    elements: IndexMap<Id, ElementState>,
}

/// Rendering for a single output
#[derive(Debug)]
pub struct OutputRender {
    output: Output,
    last_states: VecDeque<OutputRenderState>,
    empty_state: OutputRenderState,
}

impl OutputRender {
    /// Initialize a new [`OutputRender`]
    pub fn new(output: &Output) -> Self {
        Self {
            output: output.clone(),
            last_states: VecDeque::new(),
            empty_state: OutputRenderState {
                size: None,
                elements: IndexMap::new(),
            },
        }
    }

    /// Render this output
    pub fn render_output<E, R>(&mut self, renderer: &mut R, age: usize, elements: &[E], log: &slog::Logger)
    where
        E: RenderElement<R>,
        R: Renderer + ImportAll + std::fmt::Debug,
        <R as Renderer>::TextureId: Texture + std::fmt::Debug + 'static,
    {
        let output_size = self.output.current_mode().unwrap().size;
        let output_transform = self.output.current_transform();
        let output_scale = Scale::from(self.output.current_scale().fractional_scale());
        let output_geo = Rectangle::from_loc_and_size((0, 0), output_size);

        // This will hold all the damage we need for this rendering step
        let mut damage: Vec<Rectangle<i32, Physical>> = Vec::new();
        let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
        let mut opaque_regions: Vec<Rectangle<i32, Physical>> = Vec::new();

        let last_state = if age > 0 && self.last_states.len() >= age {
            self.last_states.truncate(age);
            self.last_states.get(age - 1).unwrap_or(&self.empty_state)
        } else {
            damage = vec![output_geo];
            &self.empty_state
        };

        // If the output size changed damage everything
        if last_state.size.map(|s| s != output_size).unwrap_or(true) {
            damage = vec![output_geo];
        }

        for element in elements {
            let element_geometry = element.geometry(output_scale);

            // First test if the element overlaps with the output
            // if not we can skip ip
            if !element_geometry.overlaps(output_geo) {
                continue;
            }

            // Then test if the element is completely hidden behind opaque regions
            let is_hidden = opaque_regions
                .iter()
                .fold([element_geometry].to_vec(), |geometry, opaque_region| {
                    geometry
                        .into_iter()
                        .flat_map(|g| g.subtract_rect(*opaque_region))
                        .collect::<Vec<_>>()
                })
                .is_empty();

            if is_hidden {
                // No need to draw a completely hidden element
                continue;
            }

            let element_damage = element
                .damage_since(
                    output_scale,
                    last_state.elements.get(element.id()).map(|s| s.last_commit),
                )
                .into_iter()
                .map(|mut d| {
                    d.loc += element_geometry.loc;
                    d
                })
                .collect::<Vec<_>>();

            let element_output_damage =
                opaque_regions
                    .iter()
                    .fold(element_damage, |damage, opaque_region| {
                        damage
                            .into_iter()
                            .flat_map(|damage| damage.subtract_rect(*opaque_region))
                            .collect::<Vec<_>>()
                    });

            damage.extend(element_output_damage);
            opaque_regions.extend(
                element
                    .opaque_regions(output_scale)
                    .into_iter()
                    .map(|mut region| {
                        region.loc += element_geometry.loc;
                        region
                    }),
            );
            render_elements.insert(0, element);
        }

        // add the damage for elements gone that are not covered by
        // by an opaque region
        // TODO: actually filter the damage with the opaque regions
        let elements_gone = last_state
            .elements
            .iter()
            .filter_map(|(id, state)| {
                if !render_elements.iter().any(|e| e.id() == id) {
                    Some(state.last_geometry)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        damage.extend(elements_gone);

        // if the element has been moved or it's z index changed damage it
        for (z_index, element) in render_elements.iter().enumerate() {
            let element_geometry = element.geometry(output_scale);
            let element_last_state = last_state.elements.get(element.id());

            if element_last_state
                .map(|s| s.last_geometry != element_geometry || s.last_z_index != z_index)
                .unwrap_or(false)
            {
                if let Some(old_geo) = element_last_state.map(|s| s.last_geometry) {
                    damage.push(old_geo);
                }
                damage.push(element_geometry);
            }
        }

        // Optimize the damage for rendering
        damage.dedup();
        damage.retain(|rect| rect.overlaps(output_geo));
        damage.retain(|rect| !rect.is_empty());
        // filter damage outside of the output gep and merge overlapping rectangles
        damage = damage
            .into_iter()
            .filter_map(|rect| rect.intersection(output_geo))
            .fold(Vec::new(), |new_damage, mut rect| {
                // replace with drain_filter, when that becomes stable to reuse the original Vec's memory
                let (overlapping, mut new_damage): (Vec<_>, Vec<_>) =
                    new_damage.into_iter().partition(|other| other.overlaps(rect));

                for overlap in overlapping {
                    rect = rect.merge(overlap);
                }
                new_damage.push(rect);
                new_damage
            });

        if damage.is_empty() {
            return;
        }

        dbg!(elements.len());
        dbg!(render_elements.len());

        let mut elements_drawn = 0;

        renderer
            .render(output_size, output_transform.into(), |renderer, frame| {
                frame
                    .clear([1.0f32, 0.0f32, 0.0f32, 1.0f32], &*damage)
                    .expect("failed to clear the frame");

                for element in render_elements.iter() {
                    let element_geometry = element.geometry(output_scale);

                    let element_damage = damage
                        .iter()
                        .filter_map(|d| d.intersection(element_geometry))
                        .map(|mut d| {
                            d.loc -= element_geometry.loc;
                            d
                        })
                        .collect::<Vec<_>>();

                    if element_damage.is_empty() {
                        continue;
                    }

                    element.draw(renderer, frame, output_scale, &*element_damage, log);
                    elements_drawn += 1;
                }
            })
            .expect("failed to render");

        dbg!(elements_drawn);

        let new_elements_state = render_elements
            .iter()
            .enumerate()
            .map(|(zindex, elem)| {
                let id = elem.id().clone();
                let current_commit = elem.current_commit();
                let elem_geometry = elem.geometry(output_scale);
                let state = ElementState {
                    last_commit: current_commit,
                    last_geometry: elem_geometry,
                    last_z_index: zindex,
                };
                (id, state)
            })
            .collect();
        self.last_states.push_front(OutputRenderState {
            size: Some(output_size),
            elements: new_elements_state,
        });
    }
}