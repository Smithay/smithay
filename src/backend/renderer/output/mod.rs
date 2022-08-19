//! TODO: Docs

use std::collections::VecDeque;

use indexmap::IndexMap;

use crate::{
    backend::renderer::Frame,
    utils::{Physical, Rectangle, Scale, Size, Transform},
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

#[derive(Debug, Default)]
struct OutputRenderState {
    size: Option<Size<i32, Physical>>,
    elements: IndexMap<Id, ElementState>,
    old_damage: VecDeque<Vec<Rectangle<i32, Physical>>>,
}

/// Mode for the [`DamageTrackedRenderer`]
#[derive(Debug, Clone)]
pub enum Mode {
    /// Automatic mode based on a output
    Auto(Output),
    /// Static mode
    Static {
        /// Size of the static output
        size: Size<i32, Physical>,
        /// Scale of the static output
        scale: Scale<f64>,
        /// Transform of the static output
        transform: Transform,
    },
}

/// Output has no active mode
#[derive(Debug, thiserror::Error)]
#[error("Output has no active mode")]
pub struct OutputNoMode;

impl TryInto<(Size<i32, Physical>, Scale<f64>, Transform)> for Mode {
    type Error = OutputNoMode;

    fn try_into(self) -> Result<(Size<i32, Physical>, Scale<f64>, Transform), Self::Error> {
        match self {
            Mode::Auto(output) => Ok((
                output.current_mode().ok_or(OutputNoMode)?.size,
                output.current_scale().fractional_scale().into(),
                output.current_transform().into(),
            )),
            Mode::Static {
                size,
                scale,
                transform,
            } => Ok((size, scale, transform)),
        }
    }
}

/// Rendering for a single output
#[derive(Debug)]
pub struct DamageTrackedRenderer {
    mode: Mode,
    last_state: OutputRenderState,
}

/// Errors thrown by [`Space::render_output`]
#[derive(thiserror::Error)]
pub enum OutputRenderError<R: Renderer> {
    /// The provided [`Renderer`] did return an error during an operation
    #[error(transparent)]
    Rendering(R::Error),
    /// The given [`Output`] has no set mode
    #[error(transparent)]
    OutputNoMode(#[from] OutputNoMode),
}

impl<R: Renderer> std::fmt::Debug for OutputRenderError<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputRenderError::Rendering(err) => std::fmt::Debug::fmt(err, f),
            OutputRenderError::OutputNoMode(err) => std::fmt::Debug::fmt(err, f),
        }
    }
}

impl DamageTrackedRenderer {
    /// Initialize a new [`DamageTrackedRenderer`]
    pub fn new(size: Size<i32, Physical>, scale: impl Into<Scale<f64>>, transform: Transform) -> Self {
        Self {
            mode: Mode::Static {
                size,
                scale: scale.into(),
                transform,
            },
            last_state: Default::default(),
        }
    }

    /// Initialize a new [`DamageTrackedRenderer`]
    pub fn from_output(output: &Output) -> Self {
        Self {
            mode: Mode::Auto(output.clone()),
            last_state: Default::default(),
        }
    }

    /// Get the [`Mode`] of the [`DamageTrackedRenderer`]
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// Render this output
    pub fn render_output<E, R>(
        &mut self,
        renderer: &mut R,
        age: usize,
        elements: &[E],
        clear_color: [f32; 4],
        log: &slog::Logger,
    ) -> Result<Option<Vec<Rectangle<i32, Physical>>>, OutputRenderError<R>>
    where
        E: RenderElement<R>,
        R: Renderer + ImportAll,
        <R as Renderer>::TextureId: Texture,
    {
        let (output_size, output_scale, output_transform) = self.mode.clone().try_into()?;
        let output_geo = Rectangle::from_loc_and_size((0, 0), output_size);

        // This will hold all the damage we need for this rendering step
        let mut damage: Vec<Rectangle<i32, Physical>> = Vec::new();
        let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
        let mut opaque_regions: Vec<Rectangle<i32, Physical>> = Vec::new();

        if self.last_state.size.map(|geo| geo != output_size).unwrap_or(true) {
            // The output geometry changed, so just damage everything
            slog::trace!(log, "Output geometry changed, damaging whole output geometry. previous geometry: {:?}, current geometry: {:?}", self.last_state.size, output_geo);
            damage = vec![output_geo];
        }

        // We now add old damage states, if we have an age value
        if age > 0 && self.last_state.old_damage.len() >= age {
            // We do not need even older states anymore
            self.last_state.old_damage.truncate(age);
            damage.extend(self.last_state.old_damage.iter().flatten().copied());
        } else {
            // just damage everything, if we have no damage
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
                    self.last_state.elements.get(element.id()).map(|s| s.last_commit),
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
        let elements_gone = self
            .last_state
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
            let element_last_state = self.last_state.elements.get(element.id());

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
            return Ok(None);
        }

        let mut elements_drawn = 0;

        let res = renderer.render(output_size, output_transform.into(), |renderer, frame| {
            frame.clear(clear_color, &*damage)?;

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

                element.draw(renderer, frame, output_scale, &*element_damage, log)?;
                elements_drawn += 1;
            }

            Result::<(), R::Error>::Ok(())
        });

        if let Err(err) = res {
            // if the rendering errors on us, we need to be prepared, that this whole buffer was partially updated and thus now unusable.
            // thus clean our old states before returning
            self.last_state = Default::default();
            return Err(OutputRenderError::Rendering(err));
        }

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
        self.last_state.size = Some(output_size);
        self.last_state.elements = new_elements_state;
        self.last_state.old_damage.push_front(damage.clone());

        Ok(Some(damage))
    }
}
