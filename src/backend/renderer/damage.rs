//! Helper for effective damage tracked rendering
//!
//! # Why use this implementation
//!
//! The [`DamageTrackedRenderer`] in combination with the [`RenderElement`] trait
//! can help you to reduce resource consumption by tracking what elements have
//! been damaged and only redraw the damaged parts on an output.
//!
//! It does so by keeping track of the last used [`CommitCounter`] for all provided
//! [`RenderElement`]s and queries the element for new damage on each call to [`render_output`](DamageTrackedRenderer::render_output).
//!
//! You can initialize it with a static output by using [`DamageTrackedRenderer::new`] or
//! allow it to track a specific [`Output`] with [`DamageTrackedRenderer::from_output`].
//!
//! See the [`renderer::element`](crate::backend::renderer::element) module for more information
//! about how to use [`RenderElement`].
//!
//! # How to use it
//!
//! ```no_run
//! # use smithay::{
//! #     backend::renderer::{Frame, ImportMem, Renderer, Texture, TextureFilter},
//! #     utils::{Buffer, Physical, Rectangle, Size},
//! # };
//! # use slog::Drain;
//! #
//! # #[derive(Clone)]
//! # struct FakeTexture;
//! #
//! # impl Texture for FakeTexture {
//! #     fn width(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn height(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # struct FakeFrame;
//! #
//! # impl Frame for FakeFrame {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #
//! #     fn clear(&mut self, _: [f32; 4], _: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn render_texture_from_to(
//! #         &mut self,
//! #         _: &Self::TextureId,
//! #         _: Rectangle<f64, Buffer>,
//! #         _: Rectangle<i32, Physical>,
//! #         _: &[Rectangle<i32, Physical>],
//! #         _: Transform,
//! #         _: f32,
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn transformation(&self) -> Transform {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # struct FakeRenderer;
//! #
//! # impl Renderer for FakeRenderer {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #     type Frame = FakeFrame;
//! #
//! #     fn id(&self) -> usize {
//! #         unimplemented!()
//! #     }
//! #     fn downscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn upscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn render<F, R>(&mut self, _: Size<i32, Physical>, _: Transform, _: F) -> Result<R, Self::Error>
//! #     where
//! #         F: FnOnce(&mut Self, &mut Self::Frame) -> R,
//! #     {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # impl ImportMem for FakeRenderer {
//! #     fn import_memory(
//! #         &mut self,
//! #         _: &[u8],
//! #         _: Size<i32, Buffer>,
//! #         _: bool,
//! #     ) -> Result<Self::TextureId, Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn update_memory(
//! #         &mut self,
//! #         _: &Self::TextureId,
//! #         _: &[u8],
//! #         _: Rectangle<i32, Buffer>,
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! # }
//! use smithay::{
//!     backend::renderer::{
//!         damage::DamageTrackedRenderer,
//!         element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement}
//!     },
//!     utils::{Point, Transform},
//! };
//! use std::time::{Duration, Instant};
//!
//! const WIDTH: i32 = 10;
//! const HEIGHT: i32 = 10;
//! # let mut renderer = FakeRenderer;
//! # let buffer_age = 0;
//! # let log = slog::Logger::root(slog::Discard.fuse(), slog::o!());
//!
//! // Initialize a new damage tracked renderer
//! let mut damage_tracked_renderer = DamageTrackedRenderer::new((800, 600), 1.0, Transform::Normal);
//!
//! // Initialize a buffer to render
//! let mut memory_buffer = MemoryRenderBuffer::new((WIDTH, HEIGHT), 1, Transform::Normal, None);
//!
//! let mut last_update = Instant::now();
//!
//! loop {
//!     let now = Instant::now();
//!     if now.duration_since(last_update) >= Duration::from_secs(3) {
//!         let mut render_context = memory_buffer.render();
//!
//!         render_context.draw(|_buffer| {
//!             // Update the changed parts of the buffer
//!
//!             // Return the updated parts
//!             vec![Rectangle::from_loc_and_size(Point::default(), (WIDTH, HEIGHT))]
//!         });
//!
//!         last_update = now;
//!     }
//!
//!     // Create a render element from the buffer
//!     let location = Point::from((100.0, 100.0));
//!     let render_element =
//!         MemoryRenderBufferRenderElement::from_buffer(location, &memory_buffer, None, None);
//!
//!     // Render the output
//!     damage_tracked_renderer
//!         .render_output(
//!             &mut renderer,
//!             buffer_age,
//!             &[render_element],
//!             [0.8, 0.8, 0.9, 1.0],
//!             &log,
//!         )
//!         .expect("failed to render the output");
//! }
//! ```

use std::collections::VecDeque;

use indexmap::IndexMap;

use crate::{
    backend::renderer::Frame,
    output::Output,
    utils::{Physical, Rectangle, Scale, Size, Transform},
};

use super::{
    element::{Id, RenderElement},
    utils::CommitCounter,
};

use super::{Renderer, Texture};

#[derive(Debug, Clone, Copy)]
struct ElementState {
    last_commit: CommitCounter,
    last_geometry: Rectangle<i32, Physical>,
    last_z_index: usize,
}

#[derive(Debug, Default)]
struct RendererState {
    size: Option<Size<i32, Physical>>,
    elements: IndexMap<Id, ElementState>,
    old_damage: VecDeque<Vec<Rectangle<i32, Physical>>>,
}

/// Mode for the [`DamageTrackedRenderer`]
#[derive(Debug, Clone)]
pub enum DamageTrackedRendererMode {
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

impl TryInto<(Size<i32, Physical>, Scale<f64>, Transform)> for DamageTrackedRendererMode {
    type Error = OutputNoMode;

    fn try_into(self) -> Result<(Size<i32, Physical>, Scale<f64>, Transform), Self::Error> {
        match self {
            DamageTrackedRendererMode::Auto(output) => Ok((
                output.current_mode().ok_or(OutputNoMode)?.size,
                output.current_scale().fractional_scale().into(),
                output.current_transform(),
            )),
            DamageTrackedRendererMode::Static {
                size,
                scale,
                transform,
            } => Ok((size, scale, transform)),
        }
    }
}

/// Damage tracked renderer for a single output
#[derive(Debug)]
pub struct DamageTrackedRenderer {
    mode: DamageTrackedRendererMode,
    last_state: RendererState,
}

/// Errors thrown by [`DamageTrackedRenderer::render_output`]
#[derive(thiserror::Error)]
pub enum DamageTrackedRendererError<R: Renderer> {
    /// The provided [`Renderer`] returned an error
    #[error(transparent)]
    Rendering(R::Error),
    /// The given [`Output`] has no mode set
    #[error(transparent)]
    OutputNoMode(#[from] OutputNoMode),
}

impl<R: Renderer> std::fmt::Debug for DamageTrackedRendererError<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DamageTrackedRendererError::Rendering(err) => std::fmt::Debug::fmt(err, f),
            DamageTrackedRendererError::OutputNoMode(err) => std::fmt::Debug::fmt(err, f),
        }
    }
}

impl DamageTrackedRenderer {
    /// Initialize a static [`DamageTrackedRenderer`]
    pub fn new(
        size: impl Into<Size<i32, Physical>>,
        scale: impl Into<Scale<f64>>,
        transform: Transform,
    ) -> Self {
        Self {
            mode: DamageTrackedRendererMode::Static {
                size: size.into(),
                scale: scale.into(),
                transform,
            },
            last_state: Default::default(),
        }
    }

    /// Initialize a new [`DamageTrackedRenderer`] from an [`Output`]
    ///
    /// The renderer will keep track of changes to the [`Output`]
    /// and handle size and scaling changes automatically on the
    /// next call to [`render_output`]
    pub fn from_output(output: &Output) -> Self {
        Self {
            mode: DamageTrackedRendererMode::Auto(output.clone()),
            last_state: Default::default(),
        }
    }

    /// Get the [`Mode`] of the [`DamageTrackedRenderer`]
    pub fn mode(&self) -> &DamageTrackedRendererMode {
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
    ) -> Result<Option<Vec<Rectangle<i32, Physical>>>, DamageTrackedRendererError<R>>
    where
        E: RenderElement<R>,
        R: Renderer,
        <R as Renderer>::TextureId: Texture,
    {
        let (output_size, output_scale, output_transform) = self.mode.clone().try_into()?;
        let output_geo = Rectangle::from_loc_and_size((0, 0), output_size);

        // This will hold all the damage we need for this rendering step
        let mut damage: Vec<Rectangle<i32, Physical>> = Vec::new();
        let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
        let mut opaque_regions: Vec<(usize, Vec<Rectangle<i32, Physical>>)> = Vec::new();

        // We use an explicit z-index because the following loop can skip
        // elements that are completely hidden and we want the z-index to
        // match when enumerating the render elements later
        let mut z_index = 0;
        for element in elements.iter() {
            let element_geometry = element.geometry(output_scale);

            // First test if the element overlaps with the output
            // if not we can skip ip
            if !element_geometry.overlaps(output_geo) {
                continue;
            }

            // Then test if the element is completely hidden behind opaque regions
            let is_hidden = opaque_regions
                .iter()
                .flat_map(|(_, opaque_regions)| opaque_regions)
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

            let element_output_damage = opaque_regions
                .iter()
                .flat_map(|(_, opaque_regions)| opaque_regions)
                .fold(element_damage, |damage, opaque_region| {
                    damage
                        .into_iter()
                        .flat_map(|damage| damage.subtract_rect(*opaque_region))
                        .collect::<Vec<_>>()
                });

            damage.extend(element_output_damage);

            let element_opaque_regions = element
                .opaque_regions(output_scale)
                .into_iter()
                .map(|mut region| {
                    region.loc += element_geometry.loc;
                    region
                })
                .collect::<Vec<_>>();
            opaque_regions.push((z_index, element_opaque_regions));
            render_elements.push(element);
            z_index += 1;
        }

        // add the damage for elements gone that are not covered an opaque region
        let elements_gone = self
            .last_state
            .elements
            .iter()
            .filter(|(id, _)| !render_elements.iter().any(|e| e.id() == *id))
            .flat_map(|(_, state)| {
                opaque_regions
                    .iter()
                    .filter(|(z_index, _)| *z_index < state.last_z_index)
                    .flat_map(|(_, opaque_regions)| opaque_regions)
                    .fold(vec![state.last_geometry], |damage, opaque_region| {
                        damage
                            .into_iter()
                            .flat_map(|damage| damage.subtract_rect(*opaque_region))
                            .collect::<Vec<_>>()
                    })
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
                let mut element_damage = vec![element_geometry];
                if let Some(old_geo) = element_last_state.map(|s| s.last_geometry) {
                    element_damage.push(old_geo);
                }
                damage.extend(
                    opaque_regions
                        .iter()
                        .filter(|(index, _)| *index < z_index)
                        .flat_map(|(_, opaque_regions)| opaque_regions)
                        .fold(element_damage, |damage, opaque_region| {
                            damage
                                .into_iter()
                                .flat_map(|damage| damage.subtract_rect(*opaque_region))
                                .collect::<Vec<_>>()
                        }),
                );
            }
        }

        if self.last_state.size.map(|geo| geo != output_size).unwrap_or(true) {
            // The output geometry changed, so just damage everything
            slog::trace!(log, "Output geometry changed, damaging whole output geometry. previous geometry: {:?}, current geometry: {:?}", self.last_state.size, output_geo);
            damage = vec![output_geo];
        }

        // That is all completely new damage, which we need to store for subsequent renders
        let new_damage = damage.clone();

        // We now add old damage states, if we have an age value
        if age > 0 && self.last_state.old_damage.len() >= age {
            slog::trace!(log, "age of {} recent enough, using old damage", age);
            // We do not need even older states anymore
            self.last_state.old_damage.truncate(age);
            damage.extend(self.last_state.old_damage.iter().flatten().copied());
        } else {
            slog::trace!(
                log,
                "no old damage available, re-render everything. age: {} old_damage len: {}",
                age,
                self.last_state.old_damage.len(),
            );
            // just damage everything, if we have no damage
            damage = vec![output_geo];
        };

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
            slog::trace!(log, "nothing damaged, exiting early");
            return Ok(None);
        }

        slog::trace!(log, "damage to be rendered: {:#?}", &damage);

        let res = renderer.render(output_size, output_transform, |renderer, frame| {
            let clear_damage = opaque_regions.iter().flat_map(|(_, regions)| regions).fold(
                damage.clone(),
                |damage, region| {
                    damage
                        .into_iter()
                        .flat_map(|geo| geo.subtract_rect(*region))
                        .collect::<Vec<_>>()
                },
            );

            frame.clear(clear_color, &*clear_damage)?;

            for (mut z_index, element) in render_elements.iter().rev().enumerate() {
                // This is necessary because we reversed the render elements to draw
                // them back to front, but z-index including opaque regions is defined
                // front to back
                z_index = render_elements.len() - 1 - z_index;

                let element_geometry = element.geometry(output_scale);

                let element_damage = opaque_regions
                    .iter()
                    .filter(|(index, _)| *index < z_index)
                    .flat_map(|(_, regions)| regions)
                    .fold(
                        damage
                            .clone()
                            .into_iter()
                            .filter_map(|d| d.intersection(element_geometry))
                            .collect::<Vec<_>>(),
                        |damage, region| {
                            damage
                                .into_iter()
                                .flat_map(|geo| geo.subtract_rect(*region))
                                .collect::<Vec<_>>()
                        },
                    )
                    .into_iter()
                    .map(|mut d| {
                        d.loc -= element_geometry.loc;
                        d
                    })
                    .collect::<Vec<_>>();

                if element_damage.is_empty() {
                    continue;
                }

                element.draw(
                    renderer,
                    frame,
                    element.location(output_scale),
                    output_scale,
                    &*element_damage,
                    log,
                )?;
            }

            Result::<(), R::Error>::Ok(())
        });

        if let Err(err) = res {
            // if the rendering errors on us, we need to be prepared, that this whole buffer was partially updated and thus now unusable.
            // thus clean our old states before returning
            self.last_state = Default::default();
            return Err(DamageTrackedRendererError::Rendering(err));
        }

        let new_elements_state = render_elements
            .iter()
            .enumerate()
            .map(|(z_index, elem)| {
                let id = elem.id().clone();
                let current_commit = elem.current_commit();
                let elem_geometry = elem.geometry(output_scale);
                let state = ElementState {
                    last_commit: current_commit,
                    last_geometry: elem_geometry,
                    last_z_index: z_index,
                };
                (id, state)
            })
            .collect();
        self.last_state.size = Some(output_size);
        self.last_state.elements = new_elements_state;
        self.last_state.old_damage.push_front(new_damage.clone());

        Ok(Some(new_damage))
    }
}
