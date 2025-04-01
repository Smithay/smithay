//! Helper for effective output damage tracking
//!
//! # Why use this implementation
//!
//! The [`OutputDamageTracker`] in combination with the [`RenderElement`] trait
//! can help you to reduce resource consumption by tracking what elements have
//! been damaged and only redraw the damaged parts on an output.
//!
//! It does so by keeping track of the last used [`CommitCounter`] for all provided
//! [`RenderElement`]s and queries the element for new damage on each call to [`render_output`](OutputDamageTracker::render_output) or [`damage_output`](OutputDamageTracker::damage_output).
//!
//! Additionally the damage tracker will automatically generate damage in the following situations:
//! - Current geometry for elements entering the output
//! - Current and last known geometry for moved elements (includes z-index changes)
//! - Last known geometry for elements no longer present
//!
//! Elements fully occluded by opaque regions as defined by elements higher in the stack are skipped.
//! The actual action taken by the damage tracker can be inspected from the returned [`RenderElementStates`].
//!
//! You can initialize it with a static output by using [`OutputDamageTracker::new`] or
//! allow it to track a specific [`Output`] with [`OutputDamageTracker::from_output`].
//!
//! See the [`renderer::element`](crate::backend::renderer::element) module for more information
//! about how to use [`RenderElement`].
//!
//! # How to use it
//!
//! ```no_run
//! # use smithay::{
//! #     backend::renderer::{Color32F, DebugFlags, Frame, ImportMem, Renderer, Texture, TextureFilter, sync::SyncPoint, test::{DummyRenderer, DummyFramebuffer}},
//! #     utils::{Buffer, Physical, Rectangle, Size},
//! # };
//! use smithay::{
//!     backend::{
//!         allocator::Fourcc,
//!         renderer::{
//!             damage::OutputDamageTracker,
//!             element::{
//!                 Kind,
//!                 memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
//!             }
//!         },
//!     },
//!     utils::{Point, Transform},
//! };
//! use std::time::{Duration, Instant};
//!
//! const WIDTH: i32 = 10;
//! const HEIGHT: i32 = 10;
//! # let mut renderer = DummyRenderer::default();
//! # let mut framebuffer = DummyFramebuffer;
//! # let buffer_age = 0;
//!
//! // Initialize a new damage tracker for a static output
//! let mut damage_tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
//!
//! // Initialize a buffer to render
//! let mut memory_buffer = MemoryRenderBuffer::new(Fourcc::Argb8888, (WIDTH, HEIGHT), 1, Transform::Normal, None);
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
//!             Result::<_, ()>::Ok(vec![Rectangle::from_size((WIDTH, HEIGHT).into())])
//!         });
//!
//!         last_update = now;
//!     }
//!
//!     // Create a render element from the buffer
//!     let location = Point::from((100.0, 100.0));
//!     let render_element =
//!         MemoryRenderBufferRenderElement::from_buffer(&mut renderer, location, &memory_buffer, None, None, None, Kind::Unspecified)
//!         .expect("Failed to upload memory to gpu");
//!
//!     // Render the output
//!     damage_tracker
//!         .render_output(
//!             &mut renderer,
//!             &mut framebuffer,
//!             buffer_age,
//!             &[render_element],
//!             [0.8, 0.8, 0.9, 1.0],
//!         )
//!         .expect("failed to render the output");
//! }
//! ```

use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
};

use indexmap::IndexMap;
use smallvec::{smallvec, SmallVec};
use tracing::{info_span, instrument, trace};

use crate::{
    backend::renderer::{element::RenderElementPresentationState, Frame},
    output::{Output, OutputModeSource, OutputNoMode},
    utils::{Buffer as BufferCoords, Physical, Rectangle, Scale, Size, Transform},
};

use super::{
    element::{Element, Id, RenderElement, RenderElementState, RenderElementStates},
    sync::SyncPoint,
    utils::CommitCounter,
    Color32F,
};

use super::{Renderer, Texture};

mod shaper;

use shaper::DamageShaper;

const MAX_AGE: usize = 4;

#[derive(Debug, Clone, Copy)]
struct ElementInstanceState {
    last_src: Rectangle<f64, BufferCoords>,
    last_geometry: Rectangle<i32, Physical>,
    last_transform: Transform,
    last_alpha: f32,
    last_z_index: usize,
}

impl ElementInstanceState {
    #[inline]
    fn matches(
        &self,
        src: Rectangle<f64, BufferCoords>,
        geometry: Rectangle<i32, Physical>,
        transform: Transform,
        alpha: f32,
        z_index: usize,
    ) -> bool {
        self.last_src == src
            && self.last_geometry == geometry
            && self.last_transform == transform
            && self.last_alpha == alpha
            && self.last_z_index == z_index
    }
}

#[derive(Debug, Clone)]
struct ElementState {
    last_commit: CommitCounter,
    last_instances: SmallVec<[ElementInstanceState; 1]>,
}

impl ElementState {
    #[inline]
    fn instance_matches(
        &self,
        src: Rectangle<f64, BufferCoords>,
        geometry: Rectangle<i32, Physical>,
        transform: Transform,
        alpha: f32,
        z_index: usize,
    ) -> bool {
        self.last_instances
            .iter()
            .any(|instance| instance.matches(src, geometry, transform, alpha, z_index))
    }
}

#[derive(Debug, Default)]
struct RendererState {
    transform: Option<Transform>,
    size: Option<Size<i32, Physical>>,
    elements: IndexMap<Id, ElementState>,
    old_damage: VecDeque<Vec<Rectangle<i32, Physical>>>,
    opaque_regions: Vec<Rectangle<i32, Physical>>,
    clear_color: Option<Color32F>,
}

/// Damage tracker for a single output
#[derive(Debug)]
pub struct OutputDamageTracker {
    mode: OutputModeSource,
    last_state: RendererState,
    damage_shaper: DamageShaper,
    damage: Vec<Rectangle<i32, Physical>>,
    element_damage: Vec<Rectangle<i32, Physical>>,
    opaque_regions: Vec<Rectangle<i32, Physical>>,
    opaque_regions_index: Vec<Range<usize>>,
    element_opaque_regions: Vec<Rectangle<i32, Physical>>,
    element_visible_area_workhouse: Vec<Rectangle<i32, Physical>>,
    span: tracing::Span,
}

/// Errors thrown by [`OutputDamageTracker::render_output`]
#[derive(thiserror::Error)]
pub enum Error<E: std::error::Error> {
    /// The provided [`Renderer`] returned an error
    #[error(transparent)]
    Rendering(E),
    /// The given [`Output`] has no mode set
    #[error(transparent)]
    OutputNoMode(#[from] OutputNoMode),
}

/// Represents the result from rendering the output
#[derive(Debug)]
pub struct RenderOutputResult<'a> {
    /// Holds the sync point of the rendering operation
    pub sync: SyncPoint,
    /// Holds the damage from the rendering operation
    pub damage: Option<&'a Vec<Rectangle<i32, Physical>>>,
    /// Holds the render element states
    pub states: RenderElementStates,
}

impl RenderOutputResult<'_> {
    fn skipped(states: RenderElementStates) -> Self {
        Self {
            sync: SyncPoint::signaled(),
            damage: None,
            states,
        }
    }
}

impl<E: std::error::Error> std::fmt::Debug for Error<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Rendering(err) => std::fmt::Debug::fmt(err, f),
            Error::OutputNoMode(err) => std::fmt::Debug::fmt(err, f),
        }
    }
}

impl OutputDamageTracker {
    /// Initialize a static [`OutputDamageTracker`]
    pub fn new(
        size: impl Into<Size<i32, Physical>>,
        scale: impl Into<Scale<f64>>,
        transform: Transform,
    ) -> Self {
        Self {
            mode: OutputModeSource::Static {
                size: size.into(),
                scale: scale.into(),
                transform,
            },
            last_state: Default::default(),
            damage_shaper: Default::default(),
            damage: Default::default(),
            element_damage: Default::default(),
            opaque_regions: Default::default(),
            opaque_regions_index: Default::default(),
            element_opaque_regions: Default::default(),
            element_visible_area_workhouse: Default::default(),
            span: info_span!("renderer_damage"),
        }
    }

    /// Initialize a new [`OutputDamageTracker`] from an [`Output`]
    ///
    /// The renderer will keep track of changes to the [`Output`]
    /// and handle size and scaling changes automatically on the
    /// next call to [`render_output`](OutputDamageTracker::render_output)
    pub fn from_output(output: &Output) -> Self {
        Self {
            mode: OutputModeSource::Auto(output.clone()),
            damage_shaper: Default::default(),
            damage: Default::default(),
            element_damage: Default::default(),
            opaque_regions: Default::default(),
            opaque_regions_index: Default::default(),
            element_opaque_regions: Default::default(),
            element_visible_area_workhouse: Default::default(),
            last_state: Default::default(),
            span: info_span!("renderer_damage", output = output.name()),
        }
    }

    /// Initialize a new [`OutputDamageTracker`] from an [`OutputModeSource`].
    ///
    /// This should only be used when trying to support both static and automatic output mode
    /// sources. For known modes use [`OutputDamageTracker::new`] or
    /// [`OutputDamageTracker::from_output`] instead.
    pub fn from_mode_source(output_mode_source: impl Into<OutputModeSource>) -> Self {
        Self {
            mode: output_mode_source.into(),
            span: info_span!("render_damage"),
            damage_shaper: Default::default(),
            damage: Default::default(),
            element_damage: Default::default(),
            element_opaque_regions: Default::default(),
            opaque_regions: Default::default(),
            opaque_regions_index: Default::default(),
            element_visible_area_workhouse: Default::default(),
            last_state: Default::default(),
        }
    }

    /// Get the [`OutputModeSource`] of the [`OutputDamageTracker`]
    pub fn mode(&self) -> &OutputModeSource {
        &self.mode
    }

    /// Render this output with the provided [`Renderer`]
    ///
    /// - `elements` for this output in front-to-back order
    #[instrument(level = "trace", parent = &self.span, skip(renderer, framebuffer, elements, clear_color))]
    #[profiling::function]
    pub fn render_output<E, R>(
        &mut self,
        renderer: &mut R,
        framebuffer: &mut R::Framebuffer<'_>,
        age: usize,
        elements: &[E],
        clear_color: impl Into<Color32F>,
    ) -> Result<RenderOutputResult<'_>, Error<R::Error>>
    where
        E: RenderElement<R>,
        R: Renderer,
        R::TextureId: Texture,
    {
        let clear_color = clear_color.into();
        let (output_size, output_scale, output_transform) =
            std::convert::TryInto::<(Size<i32, Physical>, Scale<f64>, Transform)>::try_into(&self.mode)?;

        // Output transform is specified in surface-rotation, so inversion gives us the
        // render transform for the output itself.
        let output_transform = output_transform.invert();

        // We have to apply to output transform to the output size so that the intersection
        // tests in damage_output_internal produces the correct results and do not crop
        // damage with the wrong size
        let output_geo = Rectangle::from_size(output_transform.transform_size(output_size));

        // This will hold all the damage we need for this rendering step
        let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
        let states = self.damage_output_internal(
            age,
            elements,
            output_scale,
            output_transform,
            output_geo,
            Some(clear_color),
            &mut render_elements,
        );

        if self.damage.is_empty() {
            trace!("no damage, skipping rendering");
            return Ok(RenderOutputResult::skipped(states));
        }

        trace!(
            "rendering with damage {:?} and opaque regions {:?}",
            self.damage,
            self.opaque_regions
        );

        let render_res = (|| {
            // we have to take the element damage to be able to move it around
            let mut element_damage = std::mem::take(&mut self.element_damage);
            let mut element_opaque_regions = std::mem::take(&mut self.element_opaque_regions);
            let mut frame = renderer.render(framebuffer, output_size, output_transform)?;

            element_damage.clear();
            element_damage.extend_from_slice(&self.damage);
            element_damage =
                Rectangle::subtract_rects_many_in_place(element_damage, self.opaque_regions.iter().copied());

            trace!("clearing damage {:?}", element_damage);
            frame.clear(clear_color, &element_damage)?;

            for (z_index, element) in render_elements.iter().rev().enumerate() {
                let element_id = element.id();
                let element_geometry = element.geometry(output_scale);

                element_damage.clear();
                element_damage.extend(
                    self.damage
                        .iter()
                        .filter_map(|d| d.intersection(element_geometry)),
                );

                let element_opaque_regions_range =
                    self.opaque_regions_index.iter().rev().nth(z_index).unwrap();
                element_damage = Rectangle::subtract_rects_many_in_place(
                    element_damage,
                    self.opaque_regions[..element_opaque_regions_range.start]
                        .iter()
                        .copied(),
                );
                element_damage.iter_mut().for_each(|d| {
                    d.loc -= element_geometry.loc;
                });

                if element_damage.is_empty() {
                    trace!(
                        "skipping rendering element {:?} with geometry {:?}, no damage",
                        element_id,
                        element_geometry
                    );
                    continue;
                }

                element_opaque_regions.clear();
                element_opaque_regions.extend(
                    self.opaque_regions[element_opaque_regions_range.start..element_opaque_regions_range.end]
                        .iter()
                        .copied()
                        .map(|mut rect| {
                            rect.loc -= element_geometry.loc;
                            rect
                        }),
                );

                trace!(
                    "rendering element {:?} with geometry {:?} and damage {:?}",
                    element_id,
                    element_geometry,
                    element_damage,
                );

                element.draw(
                    &mut frame,
                    element.src(),
                    element_geometry,
                    &element_damage,
                    &element_opaque_regions,
                )?;
            }

            // return the element damage so that we can re-use the allocation
            std::mem::swap(&mut self.element_damage, &mut element_damage);
            std::mem::swap(&mut self.element_opaque_regions, &mut element_opaque_regions);
            frame.finish()
        })();

        match render_res {
            Ok(sync) => Ok(RenderOutputResult {
                sync,
                damage: Some(&self.damage),
                states,
            }),
            Err(err) => {
                // if the rendering errors on us, we need to be prepared, that this whole buffer was partially updated and thus now unusable.
                // thus clean our old states before returning
                self.last_state = Default::default();
                Err(Error::Rendering(err))
            }
        }
    }

    /// Damage this output and return the damage without actually rendering the difference
    ///
    /// - `elements` for this output in front-to-back order
    #[instrument(level = "trace", parent = &self.span, skip(elements))]
    #[profiling::function]
    pub fn damage_output<'a, 'e, E>(
        &'a mut self,
        age: usize,
        elements: &'e [E],
    ) -> Result<(Option<&'a Vec<Rectangle<i32, Physical>>>, RenderElementStates), OutputNoMode>
    where
        E: Element,
    {
        let (output_size, output_scale, output_transform) = self.mode.clone().try_into()?;

        // Output transform is specified in surface-rotation, so inversion gives us the
        // render transform for the output itself.
        let output_transform = output_transform.invert();

        // We have to apply to output transform to the output size so that the intersection
        // tests in damage_output_internal produces the correct results and do not crop
        // damage with the wrong size
        let output_geo = Rectangle::from_size(output_transform.transform_size(output_size));

        let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
        let states = self.damage_output_internal(
            age,
            elements,
            output_scale,
            output_transform,
            output_geo,
            self.last_state.clear_color,
            &mut render_elements,
        );

        if self.damage.is_empty() {
            Ok((None, states))
        } else {
            Ok((Some(&self.damage), states))
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[profiling::function]
    fn damage_output_internal<'a, E>(
        &mut self,
        age: usize,
        elements: &'a [E],
        output_scale: Scale<f64>,
        output_transform: Transform,
        output_geo: Rectangle<i32, Physical>,
        clear_color: Option<Color32F>,
        render_elements: &mut Vec<&'a E>,
    ) -> RenderElementStates
    where
        E: Element,
    {
        self.damage.clear();
        self.opaque_regions.clear();
        self.opaque_regions_index.clear();

        let mut element_render_states = RenderElementStates {
            states: HashMap::with_capacity(elements.len()),
        };

        // we have to take the element damage to be able to move it around
        let mut element_damage = std::mem::take(&mut self.element_damage);

        let mut element_visible_area_workhouse = std::mem::take(&mut self.element_visible_area_workhouse);
        for element in elements.iter() {
            let element_id = element.id();
            let element_loc = element.geometry(output_scale).loc;

            // First test if the element overlaps with the output
            // if not we can skip it
            let element_output_geometry = match element.geometry(output_scale).intersection(output_geo) {
                Some(geo) => geo,
                None => continue,
            };

            // Then test if the element is completely hidden behind opaque regions
            element_visible_area_workhouse.clear();
            element_visible_area_workhouse.push(element_output_geometry);
            element_visible_area_workhouse = Rectangle::subtract_rects_many_in_place(
                element_visible_area_workhouse,
                self.opaque_regions.iter().copied(),
            );
            let element_visible_area = element_visible_area_workhouse
                .iter()
                .fold(0usize, |acc, item| acc + (item.size.w * item.size.h) as usize);

            // No need to draw a completely hidden element
            if element_visible_area == 0 {
                // We allow multiple instance of a single element, so do not
                // override the state if we already have one
                if !element_render_states.states.contains_key(element_id) {
                    element_render_states
                        .states
                        .insert(element_id.clone(), RenderElementState::skipped());
                }
                continue;
            }

            let element_output_damage = element
                .damage_since(
                    output_scale,
                    self.last_state.elements.get(element_id).map(|s| s.last_commit),
                )
                .into_iter()
                .map(|mut d| {
                    d.loc += element_loc;
                    d
                })
                .filter_map(|geo| geo.intersection(output_geo));
            self.damage.extend(element_output_damage);

            let element_opaque_regions_start_index = self.opaque_regions.len();
            let element_opaque_regions = element
                .opaque_regions(output_scale)
                .into_iter()
                .map(|mut region| {
                    region.loc += element_loc;
                    region
                })
                .filter_map(|geo| geo.intersection(output_geo));
            self.opaque_regions.extend(element_opaque_regions);
            let element_opaque_regions_end_index = self.opaque_regions.len();
            self.opaque_regions_index
                .push(element_opaque_regions_start_index..element_opaque_regions_end_index);
            render_elements.push(element);

            if let Some(state) = element_render_states.states.get_mut(element_id) {
                if matches!(state.presentation_state, RenderElementPresentationState::Skipped) {
                    *state = RenderElementState::rendered(element_visible_area);
                } else {
                    state.visible_area += element_visible_area;
                }
            } else {
                element_render_states.states.insert(
                    element_id.clone(),
                    RenderElementState::rendered(element_visible_area),
                );
            }
        }
        std::mem::swap(
            &mut self.element_visible_area_workhouse,
            &mut element_visible_area_workhouse,
        );

        // add the damage for elements gone that are not covered an opaque region
        let elements_gone = self.last_state.elements.iter().filter(|(id, _)| {
            element_render_states
                .states
                .get(id)
                .map(|state| state.presentation_state == RenderElementPresentationState::Skipped)
                .unwrap_or(true)
        });

        for (_, state) in elements_gone {
            self.damage.extend(
                state
                    .last_instances
                    .iter()
                    .filter_map(|i| i.last_geometry.intersection(output_geo)),
            );
        }

        // if the element has been moved or it's alpha or z index changed, damage it
        for (z_index, element) in render_elements.iter().enumerate() {
            let element_src = element.src();
            let element_geometry = element.geometry(output_scale);
            let element_transform = element.transform();
            let element_alpha = element.alpha();
            let element_last_state = self.last_state.elements.get(element.id());

            if element_last_state
                .map(|s| {
                    !s.instance_matches(
                        element_src,
                        element_geometry,
                        element_transform,
                        element_alpha,
                        z_index,
                    )
                })
                .unwrap_or(true)
            {
                if let Some(intersection) = element_geometry.intersection(output_geo) {
                    self.damage.push(intersection);
                }
                if let Some(state) = element_last_state {
                    self.damage.extend(
                        state
                            .last_instances
                            .iter()
                            .filter_map(|i| i.last_geometry.intersection(output_geo)),
                    );
                }
            }
        }

        // damage regions no longer covered by opaque regions
        element_damage.clear();
        element_damage.extend_from_slice(&self.last_state.opaque_regions);
        element_damage =
            Rectangle::subtract_rects_many_in_place(element_damage, self.opaque_regions.iter().copied());
        self.damage.extend_from_slice(&element_damage);

        // we no longer need the element damage, return it so that we can
        // re-use its allocation next time
        std::mem::swap(&mut self.element_damage, &mut element_damage);

        if self.last_state.size != Some(output_geo.size)
            || self.last_state.transform != Some(output_transform)
            || self.last_state.clear_color != clear_color
        {
            // The output geometry or transform changed, so just damage everything
            trace!(
                previous_geometry = ?self.last_state.size,
                current_geometry = ?output_geo.size,
                previous_transform = ?self.last_state.transform,
                current_transform = ?output_transform,
                previous_clear_color = ?self.last_state.clear_color,
                current_clear_color = ?clear_color,
                "Output geometry, transform or clear color changed, damaging whole output geometry");
            self.damage.clear();
            self.damage.push(output_geo);
        }

        // That is all completely new damage, which we need to store for subsequent renders
        let mut new_damage = self.damage.clone();
        new_damage.shrink_to_fit();

        // We now add old damage states, if we have an age value
        if age > 0 && self.last_state.old_damage.len() >= age {
            trace!("age of {} recent enough, using old damage", age);
            // We do not need even older states anymore
            self.last_state.old_damage.truncate(age);
            self.damage
                .extend(self.last_state.old_damage.iter().take(age - 1).flatten().copied());
        } else {
            trace!(
                "no old damage available, re-render everything. age: {} old_damage len: {}",
                age,
                self.last_state.old_damage.len(),
            );
            // we still truncate the old damage to prevent growing
            // indefinitely in case we are continuously called with
            // an age of 0
            self.last_state.old_damage.truncate(MAX_AGE);
            // just damage everything, if we have no damage
            self.damage.clear();
            self.damage.push(output_geo);
        };

        // Optimize the damage for rendering

        // Clamp all rectangles to the bounds removing the ones without intersection.
        self.damage.retain_mut(|rect| {
            if let Some(intersected) = rect.intersection(output_geo) {
                *rect = intersected;
                true
            } else {
                false
            }
        });

        self.damage_shaper.shape_damage(&mut self.damage);

        if self.damage.is_empty() {
            trace!("nothing damaged, exiting early");
            return element_render_states;
        }

        let mut new_elements_state = std::mem::take(&mut self.last_state.elements);
        new_elements_state.clear();
        new_elements_state.reserve(render_elements.len());
        let new_elements_state =
            render_elements
                .iter()
                .enumerate()
                .fold(new_elements_state, |mut map, (z_index, elem)| {
                    let id = elem.id();
                    let elem_src = elem.src();
                    let elem_alpha = elem.alpha();
                    let elem_geometry = elem.geometry(output_scale);
                    let elem_transform = elem.transform();

                    if let Some(state) = map.get_mut(id) {
                        state.last_instances.push(ElementInstanceState {
                            last_src: elem_src,
                            last_geometry: elem_geometry,
                            last_transform: elem_transform,
                            last_alpha: elem_alpha,
                            last_z_index: z_index,
                        });
                    } else {
                        let current_commit = elem.current_commit();
                        map.insert(
                            id.clone(),
                            ElementState {
                                last_commit: current_commit,
                                last_instances: smallvec![ElementInstanceState {
                                    last_src: elem_src,
                                    last_geometry: elem_geometry,
                                    last_transform: elem_transform,
                                    last_alpha: elem_alpha,
                                    last_z_index: z_index,
                                }],
                            },
                        );
                    }

                    map
                });

        self.last_state.size = Some(output_geo.size);
        self.last_state.transform = Some(output_transform);
        self.last_state.elements = new_elements_state;
        self.last_state.old_damage.push_front(new_damage);
        self.last_state.opaque_regions.clear();
        self.last_state
            .opaque_regions
            .extend(self.opaque_regions.iter().copied());
        self.last_state.opaque_regions.shrink_to_fit();
        self.last_state.clear_color = clear_color;

        element_render_states
    }
}
