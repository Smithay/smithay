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
//! #     backend::renderer::{DebugFlags, Frame, ImportMem, Renderer, Texture, TextureFilter, sync::SyncPoint},
//! #     utils::{Buffer, Physical, Rectangle, Size},
//! # };
//! #
//! # #[derive(Clone, Debug)]
//! # struct FakeTexture;
//! #
//! # impl Texture for FakeTexture {
//! #     fn width(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn height(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn format(&self) -> Option<Fourcc> {
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
//! #     fn id(&self) -> usize { unimplemented!() }
//! #     fn clear(&mut self, _: [f32; 4], _: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn draw_solid(
//! #         &mut self,
//! #         _dst: Rectangle<i32, Physical>,
//! #         _damage: &[Rectangle<i32, Physical>],
//! #         _color: [f32; 4],
//! #     ) -> Result<(), Self::Error> {
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
//! #     fn finish(self) -> Result<SyncPoint, Self::Error> { unimplemented!() }
//! # }
//! #
//! # #[derive(Debug)]
//! # struct FakeRenderer;
//! #
//! # impl Renderer for FakeRenderer {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #     type Frame<'a> = FakeFrame;
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
//! #     fn set_debug_flags(&mut self, _: DebugFlags) {
//! #         unimplemented!()
//! #     }
//! #     fn debug_flags(&self) -> DebugFlags {
//! #         unimplemented!()
//! #     }
//! #     fn render(&mut self, _: Size<i32, Physical>, _: Transform) -> Result<Self::Frame<'_>, Self::Error>
//! #     {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # impl ImportMem for FakeRenderer {
//! #     fn import_memory(
//! #         &mut self,
//! #         _: &[u8],
//! #         _: Fourcc,
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
//! #     fn mem_formats(&self) -> Box<dyn Iterator<Item=Fourcc>> {
//! #         unimplemented!()
//! #     }
//! # }
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
//! # let mut renderer = FakeRenderer;
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
//!             Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(Point::default(), (WIDTH, HEIGHT))])
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
//!             buffer_age,
//!             &[render_element],
//!             [0.8, 0.8, 0.9, 1.0],
//!         )
//!         .expect("failed to render the output");
//! }
//! ```

use std::collections::{HashMap, VecDeque};

use indexmap::IndexMap;
use smallvec::{smallvec, SmallVec};
use tracing::{info_span, instrument, trace};

use crate::{
    backend::renderer::{element::RenderElementPresentationState, Frame},
    output::{Output, OutputModeSource, OutputNoMode},
    utils::{Physical, Rectangle, Scale, Size, Transform},
};

use super::{
    element::{Element, Id, RenderElement, RenderElementState, RenderElementStates},
    sync::SyncPoint,
    utils::CommitCounter,
    Bind,
};

use super::{Renderer, Texture};

mod shaper;

use shaper::DamageShaper;

const MAX_AGE: usize = 4;

#[derive(Debug, Clone, Copy)]
struct ElementInstanceState {
    last_geometry: Rectangle<i32, Physical>,
    last_alpha: f32,
    last_z_index: usize,
}

impl ElementInstanceState {
    fn matches(&self, geometry: Rectangle<i32, Physical>, alpha: f32, z_index: usize) -> bool {
        self.last_geometry == geometry && self.last_alpha == alpha && self.last_z_index == z_index
    }
}

#[derive(Debug, Clone)]
struct ElementState {
    last_commit: CommitCounter,
    last_instances: SmallVec<[ElementInstanceState; 1]>,
}

impl ElementState {
    fn instance_matches(&self, geometry: Rectangle<i32, Physical>, alpha: f32, z_index: usize) -> bool {
        self.last_instances
            .iter()
            .any(|instance| instance.matches(geometry, alpha, z_index))
    }
}

#[derive(Debug, Default)]
struct RendererState {
    transform: Option<Transform>,
    size: Option<Size<i32, Physical>>,
    elements: IndexMap<Id, ElementState>,
    old_damage: VecDeque<Vec<Rectangle<i32, Physical>>>,
    opaque_regions: Vec<Rectangle<i32, Physical>>,
}

/// Damage tracker for a single output
#[derive(Debug)]
pub struct OutputDamageTracker {
    mode: OutputModeSource,
    last_state: RendererState,
    damage_shaper: DamageShaper,
    span: tracing::Span,
}

/// Errors thrown by [`OutputDamageTracker::render_output`]
#[derive(thiserror::Error)]
pub enum Error<R: Renderer> {
    /// The provided [`Renderer`] returned an error
    #[error(transparent)]
    Rendering(R::Error),
    /// The given [`Output`] has no mode set
    #[error(transparent)]
    OutputNoMode(#[from] OutputNoMode),
}

/// Represents the result from rendering the output
#[derive(Debug)]
pub struct RenderOutputResult {
    /// Holds the sync point of the rendering operation
    pub sync: SyncPoint,
    /// Holds the damage from the rendering operation
    pub damage: Option<Vec<Rectangle<i32, Physical>>>,
    /// Holds the render element states
    pub states: RenderElementStates,
}

impl RenderOutputResult {
    fn skipped(states: RenderElementStates) -> Self {
        Self {
            sync: SyncPoint::signaled(),
            damage: None,
            states,
        }
    }
}

impl<R: Renderer> std::fmt::Debug for Error<R> {
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
            last_state: Default::default(),
        }
    }

    /// Get the [`OutputModeSource`] of the [`OutputDamageTracker`]
    pub fn mode(&self) -> &OutputModeSource {
        &self.mode
    }

    /// Render this output with the provided [`Renderer`] in the provided buffer
    ///
    /// - `elements` for this output in front-to-back order
    #[instrument(level = "trace", parent = &self.span, skip(renderer, elements, buffer))]
    #[profiling::function]
    pub fn render_output_with<E, R, B>(
        &mut self,
        renderer: &mut R,
        buffer: B,
        age: usize,
        elements: &[E],
        clear_color: [f32; 4],
    ) -> Result<RenderOutputResult, Error<R>>
    where
        E: RenderElement<R>,
        R: Renderer + Bind<B>,
        <R as Renderer>::TextureId: Texture,
    {
        self.render_output_internal(renderer, age, elements, clear_color, |r| r.bind(buffer))
    }

    /// Render this output with the provided [`Renderer`]
    ///
    /// - `elements` for this output in front-to-back order
    #[instrument(level = "trace", parent = &self.span, skip(renderer, elements))]
    #[profiling::function]
    pub fn render_output<E, R>(
        &mut self,
        renderer: &mut R,
        age: usize,
        elements: &[E],
        clear_color: [f32; 4],
    ) -> Result<RenderOutputResult, Error<R>>
    where
        E: RenderElement<R>,
        R: Renderer,
        <R as Renderer>::TextureId: Texture,
    {
        self.render_output_internal(renderer, age, elements, clear_color, |_| Ok(()))
    }

    /// Damage this output and return the damage without actually rendering the difference
    ///
    /// - `elements` for this output in front-to-back order
    #[instrument(level = "trace", parent = &self.span, skip(elements))]
    #[profiling::function]
    pub fn damage_output<E>(
        &mut self,
        age: usize,
        elements: &[E],
    ) -> Result<(Option<Vec<Rectangle<i32, Physical>>>, RenderElementStates), OutputNoMode>
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
        let output_geo = Rectangle::from_loc_and_size((0, 0), output_transform.transform_size(output_size));

        // This will hold all the damage we need for this rendering step
        let mut damage: Vec<Rectangle<i32, Physical>> = Vec::new();
        let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
        let mut opaque_regions: Vec<(usize, Vec<Rectangle<i32, Physical>>)> = Vec::new();
        let states = self.damage_output_internal(
            age,
            elements,
            output_scale,
            output_transform,
            output_geo,
            &mut damage,
            &mut render_elements,
            &mut opaque_regions,
        );

        if damage.is_empty() {
            Ok((None, states))
        } else {
            Ok((Some(damage), states))
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
        damage: &mut Vec<Rectangle<i32, Physical>>,
        render_elements: &mut Vec<&'a E>,
        opaque_regions: &mut Vec<(usize, Vec<Rectangle<i32, Physical>>)>,
    ) -> RenderElementStates
    where
        E: Element,
    {
        let mut element_render_states = RenderElementStates {
            states: HashMap::with_capacity(elements.len()),
        };

        // We use an explicit z-index because the following loop can skip
        // elements that are completely hidden and we want the z-index to
        // match when enumerating the render elements later
        let mut z_index = 0;
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
            let element_visible_area = element_output_geometry
                .subtract_rects(opaque_regions.iter().flat_map(|(_, r)| r).copied())
                .into_iter()
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
                    self.last_state.elements.get(element.id()).map(|s| s.last_commit),
                )
                .into_iter()
                .map(|mut d| {
                    d.loc += element_loc;
                    d
                })
                .filter_map(|geo| geo.intersection(output_geo))
                .collect::<Vec<_>>();
            damage.extend(element_output_damage);

            let element_opaque_regions = element
                .opaque_regions(output_scale)
                .into_iter()
                .map(|mut region| {
                    region.loc += element_loc;
                    region
                })
                .filter_map(|geo| geo.intersection(output_geo))
                .collect::<Vec<_>>();
            opaque_regions.push((z_index, element_opaque_regions));
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
            z_index += 1;
        }

        // add the damage for elements gone that are not covered an opaque region
        let elements_gone = self
            .last_state
            .elements
            .iter()
            .filter(|(id, _)| !render_elements.iter().any(|e| e.id() == *id))
            .flat_map(|(_, state)| {
                Rectangle::subtract_rects_many(
                    state
                        .last_instances
                        .iter()
                        .filter_map(|i| i.last_geometry.intersection(output_geo)),
                    opaque_regions
                        .iter()
                        .filter(|(z_index, _)| state.last_instances.iter().any(|i| *z_index < i.last_z_index))
                        .flat_map(|(_, opaque_regions)| opaque_regions)
                        .copied(),
                )
            })
            .collect::<Vec<_>>();
        damage.extend(elements_gone);

        // if the element has been moved or it's alpha or z index changed, damage it
        for (z_index, element) in render_elements.iter().enumerate() {
            let element_geometry = element.geometry(output_scale);
            let element_alpha = element.alpha();
            let element_last_state = self.last_state.elements.get(element.id());

            if element_last_state
                .map(|s| !s.instance_matches(element_geometry, element_alpha, z_index))
                .unwrap_or(true)
            {
                let mut element_damage = if let Some(damage) = element_geometry.intersection(output_geo) {
                    vec![damage]
                } else {
                    vec![]
                };
                if let Some(state) = element_last_state {
                    element_damage.extend(
                        state
                            .last_instances
                            .iter()
                            .filter_map(|i| i.last_geometry.intersection(output_geo)),
                    );
                }

                damage.extend(Rectangle::subtract_rects_many_in_place(
                    element_damage,
                    opaque_regions
                        .iter()
                        .filter(|(index, _)| *index < z_index)
                        .flat_map(|(_, opaque_regions)| opaque_regions)
                        .copied(),
                ));
            }
        }

        // damage regions no longer covered by opaque regions
        damage.extend(Rectangle::subtract_rects_many_in_place(
            self.last_state.opaque_regions.clone(),
            opaque_regions.iter().flat_map(|(_, r)| r).copied(),
        ));

        if self.last_state.size != Some(output_geo.size)
            || self.last_state.transform != Some(output_transform)
        {
            // The output geometry or transform changed, so just damage everything
            trace!(
                previous_geometry = ?self.last_state.size,
                current_geometry = ?output_geo.size,
                previous_transform = ?self.last_state.transform,
                current_transform = ?output_transform,
                "Output geometry or transform changed, damaging whole output geometry");
            *damage = vec![output_geo];
        }

        // That is all completely new damage, which we need to store for subsequent renders
        let new_damage = damage.clone();

        // We now add old damage states, if we have an age value
        if age > 0 && self.last_state.old_damage.len() >= age {
            trace!("age of {} recent enough, using old damage", age);
            // We do not need even older states anymore
            self.last_state.old_damage.truncate(age);
            damage.extend(self.last_state.old_damage.iter().take(age - 1).flatten().copied());
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
            *damage = vec![output_geo];
        };

        // Optimize the damage for rendering

        // Clamp all rectangles to the bounds removing the ones without intersection.
        damage.retain_mut(|rect| {
            if let Some(intersected) = rect.intersection(output_geo) {
                *rect = intersected;
                true
            } else {
                false
            }
        });

        self.damage_shaper.shape_damage(damage);

        if damage.is_empty() {
            trace!("nothing damaged, exiting early");
            return element_render_states;
        }

        trace!("damage to be rendered: {:#?}", &damage);

        let new_elements_state = render_elements.iter().enumerate().fold(
            IndexMap::<Id, ElementState>::with_capacity(render_elements.len()),
            |mut map, (z_index, elem)| {
                let id = elem.id();
                let elem_alpha = elem.alpha();
                let elem_geometry = elem.geometry(output_scale);

                if let Some(state) = map.get_mut(id) {
                    state.last_instances.push(ElementInstanceState {
                        last_geometry: elem_geometry,
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
                                last_geometry: elem_geometry,
                                last_alpha: elem_alpha,
                                last_z_index: z_index,
                            }],
                        },
                    );
                }

                map
            },
        );

        self.last_state.size = Some(output_geo.size);
        self.last_state.transform = Some(output_transform);
        self.last_state.elements = new_elements_state;
        self.last_state.old_damage.push_front(new_damage);
        self.last_state.opaque_regions.clear();
        self.last_state
            .opaque_regions
            .extend(opaque_regions.iter().flat_map(|(_, r)| r.iter().copied()));
        self.last_state.opaque_regions.shrink_to_fit();

        element_render_states
    }

    fn render_output_internal<E, R, F>(
        &mut self,
        renderer: &mut R,
        age: usize,
        elements: &[E],
        clear_color: [f32; 4],
        pre_render: F,
    ) -> Result<RenderOutputResult, Error<R>>
    where
        E: RenderElement<R>,
        R: Renderer,
        <R as Renderer>::TextureId: Texture,
        F: FnOnce(&mut R) -> Result<(), <R as Renderer>::Error>,
    {
        let (output_size, output_scale, output_transform) = self.mode.clone().try_into()?;

        // Output transform is specified in surface-rotation, so inversion gives us the
        // render transform for the output itself.
        let output_transform = output_transform.invert();

        // We have to apply to output transform to the output size so that the intersection
        // tests in damage_output_internal produces the correct results and do not crop
        // damage with the wrong size
        let output_geo = Rectangle::from_loc_and_size((0, 0), output_transform.transform_size(output_size));

        // This will hold all the damage we need for this rendering step
        let mut damage: Vec<Rectangle<i32, Physical>> = Vec::new();
        let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
        let mut opaque_regions: Vec<(usize, Vec<Rectangle<i32, Physical>>)> = Vec::new();
        let states = self.damage_output_internal(
            age,
            elements,
            output_scale,
            output_transform,
            output_geo,
            &mut damage,
            &mut render_elements,
            &mut opaque_regions,
        );

        if damage.is_empty() {
            trace!("no damage, skipping rendering");
            return Ok(RenderOutputResult::skipped(states));
        }

        trace!(
            "rendering with damage {:?} and opaque regions {:?}",
            damage,
            opaque_regions
        );

        pre_render(renderer).map_err(Error::Rendering)?;

        let render_res = (|| {
            let mut frame = renderer.render(output_size, output_transform)?;

            let clear_damage = Rectangle::subtract_rects_many_in_place(
                damage.clone(),
                opaque_regions.iter().flat_map(|(_, regions)| regions).copied(),
            );

            trace!("clearing damage {:?}", clear_damage);
            frame.clear(clear_color, &clear_damage)?;

            for (mut z_index, element) in render_elements.iter().rev().enumerate() {
                // This is necessary because we reversed the render elements to draw
                // them back to front, but z-index including opaque regions is defined
                // front to back
                z_index = render_elements.len() - 1 - z_index;

                let element_id = element.id();
                let element_geometry = element.geometry(output_scale);

                let element_damage = Rectangle::subtract_rects_many(
                    damage.iter().filter_map(|d| d.intersection(element_geometry)),
                    opaque_regions
                        .iter()
                        .filter(|(index, _)| *index < z_index)
                        .flat_map(|(_, regions)| regions)
                        .copied(),
                )
                .into_iter()
                .map(|mut d| {
                    d.loc -= element_geometry.loc;
                    d
                })
                .collect::<Vec<_>>();

                if element_damage.is_empty() {
                    trace!(
                        "skipping rendering element {:?} with geometry {:?}, no damage",
                        element_id,
                        element_geometry
                    );
                    continue;
                }

                trace!(
                    "rendering element {:?} with geometry {:?} and damage {:?}",
                    element_id,
                    element_geometry,
                    element_damage,
                );

                element.draw(&mut frame, element.src(), element_geometry, &element_damage)?;
            }

            frame.finish()
        })();

        match render_res {
            Ok(sync) => Ok(RenderOutputResult {
                sync,
                damage: Some(damage),
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
}
