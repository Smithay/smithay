//! This module contains the [`Space`] helper class as well has related
//! rendering helpers to add custom elements or different clients to a space.

use crate::{
    backend::renderer::{
        damage::{Error as OutputDamageTrackerError, OutputDamageTracker, RenderOutputResult},
        element::{AsRenderElements, RenderElement, Wrap},
        Renderer, Texture,
    },
    output::{Output, OutputModeSource, OutputNoMode},
    utils::{IsAlive, Logical, Point, Rectangle, Scale, Transform},
};
#[cfg(feature = "wayland_frontend")]
use crate::{
    backend::renderer::{element::surface::WaylandSurfaceRenderElement, ImportAll},
    desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType},
    wayland::shell::wlr_layer::Layer,
};
use std::{collections::HashMap, fmt};
use tracing::{debug, debug_span, instrument};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_surface::WlSurface;

mod element;
mod output;
mod utils;

#[cfg(feature = "wayland_frontend")]
pub(crate) mod wayland;

pub use self::element::*;
use self::output::*;
pub use self::utils::*;

crate::utils::ids::id_gen!(next_space_id, SPACE_ID, SPACE_IDS);

#[derive(Debug)]
struct InnerElement<E> {
    element: E,
    location: Point<i32, Logical>,
    outputs: HashMap<Output, Rectangle<i32, Logical>>,
}

/// Represents two dimensional plane to map windows and outputs upon.
///
/// Space is generic over the types of elements mapped onto it.
/// The simplest usecase is a `Space<Window>`, but other types can be used
/// by implementing [`SpaceElement`]. Multiple types might be quickly aggregated into
/// an enum by using the [`space_elements!`]-macro.
#[derive(Debug)]
pub struct Space<E: SpaceElement> {
    pub(super) id: usize,
    // in z-order, back to front
    elements: Vec<InnerElement<E>>,
    outputs: Vec<Output>,
    span: tracing::Span,
}

impl<E: SpaceElement> PartialEq for Space<E> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<E: SpaceElement> Drop for Space<E> {
    fn drop(&mut self) {
        SPACE_IDS.lock().unwrap().remove(&self.id);
    }
}

impl<E: SpaceElement> Default for Space<E> {
    fn default() -> Self {
        let id = next_space_id();
        let span = debug_span!("desktop_space", id);

        Self {
            id,
            elements: Default::default(),
            outputs: Default::default(),
            span,
        }
    }
}

impl<E: SpaceElement + PartialEq> Space<E> {
    /// Gets the id of this space
    pub fn id(&self) -> usize {
        self.id
    }

    /// Map a [`SpaceElement`] and move it to top of the stack
    ///
    /// This can safely be called on an already mapped window
    /// to update its location inside the space.
    ///
    /// If activate is true it will set the new windows state
    /// to be activate and removes that state from every
    /// other mapped window.
    pub fn map_element<P>(&mut self, element: E, location: P, activate: bool)
    where
        P: Into<Point<i32, Logical>>,
    {
        let outputs = if let Some(pos) = self.elements.iter().position(|inner| inner.element == element) {
            self.elements.remove(pos).outputs
        } else {
            HashMap::new()
        };

        let inner = InnerElement {
            element,
            location: location.into(),
            outputs,
        };
        self.insert_elem(inner, activate);
    }

    /// Moves an already mapped [`SpaceElement`] to top of the stack
    ///
    /// This function does nothing for unmapped windows.
    ///
    /// If activate is true it will set the new windows state
    /// to be activate and removes that state from every
    /// other mapped window.
    pub fn raise_element(&mut self, element: &E, activate: bool) {
        if let Some(pos) = self.elements.iter().position(|inner| &inner.element == element) {
            let inner = self.elements.remove(pos);
            self.insert_elem(inner, activate);
        }
    }

    fn insert_elem(&mut self, elem: InnerElement<E>, activate: bool) {
        if activate {
            elem.element.set_activate(true);
            for e in self.elements.iter() {
                e.element.set_activate(false);
            }
        }

        self.elements.push(elem);
        self.elements
            .sort_by(|e1, e2| e1.element.z_index().cmp(&e2.element.z_index()));
    }

    /// Unmap a [`SpaceElement`] from this space.
    ///
    /// This function does nothing for already unmapped windows
    pub fn unmap_elem(&mut self, element: &E) {
        if let Some(pos) = self.elements.iter().position(|inner| &inner.element == element) {
            let elem = self.elements.remove(pos);
            for output in elem.outputs.keys() {
                elem.element.output_leave(output);
            }
        }
    }

    /// Iterate elements in z-order back to front
    pub fn elements(&self) -> impl DoubleEndedIterator<Item = &E> {
        self.elements.iter().map(|e| &e.element)
    }

    /// Iterate elements on a specific output in z-order back to front
    pub fn elements_for_output<'output>(
        &'output self,
        output: &'output Output,
    ) -> impl DoubleEndedIterator<Item = &'output E> {
        self.elements
            .iter()
            .filter(|e| e.outputs.contains_key(output))
            .map(|e| &e.element)
    }

    /// Finds the topmost element under this point if any and returns it
    /// together with the location of this element relative to this space.
    ///
    /// This is equivalent to iterating the elements in the space from
    /// top to bottom and testing if the point is within the elements
    /// input region and returning the first matching one.
    ///
    /// Note that [`SpaceElement::is_in_input_region`] expects the point
    /// to be relative to the elements origin.
    pub fn element_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> Option<(&E, Point<i32, Logical>)> {
        let point = point.into();
        self.elements
            .iter()
            .rev()
            .filter(|e| e.bbox().to_f64().contains(point))
            .find_map(|e| {
                // we need to offset the point to the location where the surface is actually drawn
                let render_location = e.render_location();
                if e.element.is_in_input_region(&(point - render_location.to_f64())) {
                    Some((&e.element, render_location))
                } else {
                    None
                }
            })
    }

    /// Get a reference to the outputs under a given point
    pub fn output_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> impl Iterator<Item = &Output> {
        let point = point.into();
        self.outputs.iter().rev().filter(move |o| {
            let bbox = self.output_geometry(o);
            bbox.map(|bbox| bbox.to_f64().contains(point)).unwrap_or(false)
        })
    }

    /// Returns the layer surface matching a given surface, if any
    ///
    /// `surface_type` can be used to limit the types of surfaces queried for equality.
    #[cfg(feature = "wayland_frontend")]
    pub fn layer_for_surface(
        &self,
        surface: &WlSurface,
        surface_type: WindowSurfaceType,
    ) -> Option<LayerSurface> {
        self.outputs.iter().find_map(|o| {
            let map = layer_map_for_output(o);
            map.layer_for_surface(surface, surface_type).cloned()
        })
    }

    /// Returns the location of a [`SpaceElement`] inside the Space.
    pub fn element_location(&self, elem: &E) -> Option<Point<i32, Logical>> {
        self.elements
            .iter()
            .find(|e| &e.element == elem)
            .map(|e| e.location)
    }

    /// Returns the bounding box of a [`SpaceElement`] including its relative position inside the Space.
    pub fn element_bbox(&self, elem: &E) -> Option<Rectangle<i32, Logical>> {
        self.elements
            .iter()
            .find(|e| &e.element == elem)
            .map(|e| e.bbox())
    }

    /// Returns the geometry of a [`SpaceElement`] including its relative position inside the Space.
    ///
    /// This area is usually defined as the contents of the window, excluding decorations.
    pub fn element_geometry(&self, elem: &E) -> Option<Rectangle<i32, Logical>> {
        self.elements
            .iter()
            .find(|e| &e.element == elem)
            .map(|e| e.geometry())
    }

    /// Maps an [`Output`] inside the space.
    ///
    /// Can be safely called on an already mapped
    /// [`Output`] to update its location.
    ///
    /// *Note:* Remapping an output does reset it's damage memory.
    pub fn map_output<P: Into<Point<i32, Logical>>>(&mut self, output: &Output, location: P) {
        let mut state = output_state(self.id, output);
        let location = location.into();
        *state = OutputState { location };
        if !self.outputs.contains(output) {
            debug!(parent: &self.span, output = output.name(), "Mapping output at {:?}", location);
            self.outputs.push(output.clone());
        }
    }

    /// Iterate over all mapped [`Output`]s of this space.
    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.outputs.iter()
    }

    /// Unmap an [`Output`] from this space.
    ///
    /// Does nothing if the output was not previously mapped.
    pub fn unmap_output(&mut self, output: &Output) {
        if !self.outputs.contains(output) {
            return;
        }
        debug!(parent: &self.span, output = output.name(), "Unmapping output");
        if let Some(map) = output.user_data().get::<OutputUserdata>() {
            map.borrow_mut().remove(&self.id);
        }
        self.outputs.retain(|o| o != output);
    }

    /// Returns the geometry of the output including it's relative position inside the space.
    ///
    /// The size is matching the amount of logical pixels of the space visible on the output
    /// given is current mode and scale.
    pub fn output_geometry(&self, o: &Output) -> Option<Rectangle<i32, Logical>> {
        if !self.outputs.contains(o) {
            return None;
        }

        let transform: Transform = o.current_transform();
        let state = output_state(self.id, o);
        o.current_mode().map(|mode| {
            Rectangle::from_loc_and_size(
                state.location,
                transform
                    .transform_size(mode.size)
                    .to_f64()
                    .to_logical(o.current_scale().fractional_scale())
                    .to_i32_ceil(),
            )
        })
    }

    /// Returns all [`Output`]s a [`SpaceElement`] overlaps with.
    pub fn outputs_for_element(&self, elem: &E) -> Vec<Output> {
        if !self.elements.iter().any(|e| &e.element == elem) {
            return Vec::new();
        }

        self.elements
            .iter()
            .find(|e| &e.element == elem)
            .into_iter()
            .flat_map(|e| &e.outputs)
            .map(|(o, _)| o)
            .cloned()
            .collect()
    }

    /// Refresh some internal values and update client state,
    /// meaning this will handle output enter and leave events
    /// for mapped outputs and windows based on their position.
    ///
    /// Needs to be called periodically, at best before every
    /// wayland socket flush.
    #[profiling::function]
    pub fn refresh(&mut self) {
        self.elements.retain(|e| e.alive());

        let outputs = self
            .outputs
            .iter()
            .cloned()
            .map(|o| {
                let geo = self
                    .output_geometry(&o)
                    .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (0, 0)));
                (o, geo)
            })
            .collect::<Vec<_>>();
        for e in &mut self.elements {
            let bbox = e.bbox();

            for (output, output_geometry) in &outputs {
                // Check if the bounding box of the toplevel intersects with the output
                if let Some(mut overlap) = output_geometry.intersection(bbox) {
                    // output_enter expects the overlap to be relative to the element
                    overlap.loc -= bbox.loc;
                    let old = e.outputs.insert(output.clone(), overlap);
                    if old.is_none() || matches!(old, Some(old_overlap) if old_overlap != overlap) {
                        e.element.output_enter(output, overlap);
                    }
                } else if e.outputs.remove(output).is_some() {
                    e.element.output_leave(output);
                }
            }
            e.outputs.retain(|output, _| {
                if !outputs.iter().any(|(o, _)| o == output) {
                    e.element.output_leave(output);
                    false
                } else {
                    true
                }
            });
        }

        self.elements.iter().for_each(|e| e.element.refresh());
    }

    /// Retrieve the render elements for a given region of the space.
    ///
    /// *Note:* Because this is not rendering a specific output,
    /// this will not contain layer surfaces.
    /// Use [`Space::render_elements_for_output`], if you care about this.
    #[instrument(level = "trace", skip(self, renderer, scale), parent = &self.span)]
    #[profiling::function]
    pub fn render_elements_for_region<'a, R: Renderer, S: Into<Scale<f64>>>(
        &'a self,
        renderer: &mut R,
        region: &Rectangle<i32, Logical>,
        scale: S,
        alpha: f32,
    ) -> Vec<<E as AsRenderElements<R>>::RenderElement>
    where
        <R as Renderer>::TextureId: Texture + 'static,
        E: AsRenderElements<R>,
        <E as AsRenderElements<R>>::RenderElement: 'a,
    {
        let scale = scale.into();

        self.elements
            .iter()
            .rev()
            .filter(|e| {
                let geometry = e.bbox();
                region.overlaps(geometry)
            })
            .flat_map(|e| {
                let location = e.render_location() - region.loc;
                e.element
                    .render_elements::<<E as AsRenderElements<R>>::RenderElement>(
                        renderer,
                        location.to_physical_precise_round(scale),
                        scale,
                        alpha,
                    )
            })
            .collect::<Vec<_>>()
    }

    /// Retrieve the render elements for an output
    #[instrument(level = "trace", skip(self, renderer), parent = &self.span)]
    #[profiling::function]
    pub fn render_elements_for_output<
        'a,
        #[cfg(feature = "wayland_frontend")] R: Renderer + ImportAll,
        #[cfg(not(feature = "wayland_frontend"))] R: Renderer,
    >(
        &'a self,
        renderer: &mut R,
        output: &Output,
        alpha: f32,
    ) -> Result<Vec<SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>>, OutputError>
    where
        <R as Renderer>::TextureId: Texture + 'static,
        E: AsRenderElements<R>,
        <E as AsRenderElements<R>>::RenderElement: 'a,
        SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>:
            From<Wrap<<E as AsRenderElements<R>>::RenderElement>>,
    {
        if !self.outputs.contains(output) {
            return Err(OutputError::Unmapped);
        }

        let output_scale = output.current_scale().fractional_scale();
        // The unwrap is safe or we would have returned OutputError::Unmapped already
        let output_geo = self.output_geometry(output).unwrap();

        let mut space_elements: Vec<SpaceElements<'a, E>> =
            self.elements.iter().rev().map(SpaceElements::Element).collect();

        #[cfg(feature = "wayland_frontend")]
        {
            let layer_map = layer_map_for_output(output);
            space_elements.extend(layer_map.layers().rev().cloned().map(|l| SpaceElements::Layer {
                surface: l,
                output_location: output_geo.loc,
            }));
        }

        space_elements.sort_by_key(|e| std::cmp::Reverse(e.z_index()));

        Ok(space_elements
            .into_iter()
            .filter(|e| {
                let geometry = e.bbox();
                output_geo.overlaps(geometry)
            })
            .flat_map(|e| {
                let location = e.render_location() - output_geo.loc;
                e.render_elements::<SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>>(
                    renderer,
                    location.to_physical_precise_round(output_scale),
                    Scale::from(output_scale),
                    alpha,
                )
            })
            .collect::<Vec<_>>())
    }
}

/// Errors thrown by [`Space::elements_for_output`]
#[derive(thiserror::Error, Debug)]
pub enum OutputError {
    /// The given [`Output`] has no set mode
    #[error(transparent)]
    NoMode(#[from] OutputNoMode),
    /// The given [`Output`] is not mapped to this [`Space`].
    #[error("Output was not mapped to this space")]
    Unmapped,
}

impl<E: IsAlive> IsAlive for InnerElement<E> {
    fn alive(&self) -> bool {
        self.element.alive()
    }
}

impl<E: SpaceElement> InnerElement<E> {
    // the inner geometry of the element in space coordinates
    fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = self.element.geometry();
        geo.loc = self.location;
        geo
    }

    // the bounding box of the element in space coordinates
    fn bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = self.element.bbox();
        bbox.loc += self.location - self.element.geometry().loc;
        bbox
    }

    fn render_location(&self) -> Point<i32, Logical> {
        self.location - self.element.geometry().loc
    }
}

#[cfg(feature = "wayland_frontend")]
crate::backend::renderer::element::render_elements! {
    /// Defines the render elements used internally by a [`Space`]
    ///
    /// Use them in place of `E` in `space_render_elements` or
    /// `render_output` if you do not need custom render elements
    pub SpaceRenderElements<R, E> where
        R: ImportAll;
    /// A single wayland surface
    Surface=WaylandSurfaceRenderElement<R>,
    /// A single texture
    Element=Wrap<E>,
}
#[cfg(not(feature = "wayland_frontend"))]
crate::backend::renderer::element::render_elements! {
    /// Defines the render elements used internally by a [`Space`]
    ///
    /// Use them in place of `E` in `space_render_elements` or
    /// `render_output` if you do not need custom render elements
    pub SpaceRenderElements<R, E>;
    /// A single texture
    Element=Wrap<E>,
}

impl<
        #[cfg(feature = "wayland_frontend")] R: Renderer + ImportAll,
        #[cfg(not(feature = "wayland_frontend"))] R: Renderer,
        E: RenderElement<R> + std::fmt::Debug,
    > std::fmt::Debug for SpaceRenderElements<R, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "wayland_frontend")]
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            Self::Element(arg0) => f.debug_tuple("Element").field(arg0).finish(),
            Self::_GenericCatcher(_) => unreachable!(),
        }
    }
}

#[cfg(feature = "wayland_frontend")]
crate::backend::renderer::element::render_elements! {
    OutputRenderElements<'a, R, E, C> where
        R: ImportAll;
    Space=SpaceRenderElements<R, E>,
    Custom=&'a C,
}
#[cfg(not(feature = "wayland_frontend"))]
crate::backend::renderer::element::render_elements! {
    OutputRenderElements<'a, R, E, C>;
    Space=SpaceRenderElements<R, E>,
    Custom=&'a C,
}

/// Get the render elements for a specific output
///
/// If multiple spaces are given their elements will be stacked
/// the same way.
///
/// *Note*: If the `wayland_frontend`-feature is enabled
/// this will include layer-shell surfaces added to this
/// outputs [`LayerMap`](crate::desktop::LayerMap).
#[instrument(level = "trace", skip(spaces, renderer))]
#[profiling::function]
pub fn space_render_elements<
    'a,
    #[cfg(feature = "wayland_frontend")] R: Renderer + ImportAll,
    #[cfg(not(feature = "wayland_frontend"))] R: Renderer,
    E: SpaceElement + PartialEq + AsRenderElements<R> + 'a,
    S: IntoIterator<Item = &'a Space<E>>,
>(
    renderer: &mut R,
    spaces: S,
    output: &Output,
    alpha: f32,
) -> Result<Vec<SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>>, OutputNoMode>
where
    <R as Renderer>::TextureId: Texture + 'static,
    <E as AsRenderElements<R>>::RenderElement: 'a,
    SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>:
        From<Wrap<<E as AsRenderElements<R>>::RenderElement>>,
{
    let mut render_elements = Vec::new();
    let output_scale = output.current_scale().fractional_scale();

    #[cfg(feature = "wayland_frontend")]
    let layer_map = layer_map_for_output(output);
    #[cfg(feature = "wayland_frontend")]
    let lower = {
        let (lower, upper): (Vec<&LayerSurface>, Vec<&LayerSurface>) = layer_map
            .layers()
            .rev()
            .partition(|s| matches!(s.layer(), Layer::Background | Layer::Bottom));

        render_elements.extend(
            upper
                .into_iter()
                .filter_map(|surface| layer_map.layer_geometry(surface).map(|geo| (geo.loc, surface)))
                .flat_map(|(loc, surface)| {
                    AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                        surface,
                        renderer,
                        loc.to_physical_precise_round(output_scale),
                        Scale::from(output_scale),
                        alpha,
                    )
                    .into_iter()
                    .map(SpaceRenderElements::Surface)
                }),
        );

        lower
    };

    for space in spaces {
        let _guard = space.span.enter();
        if let Some(output_geo) = space.output_geometry(output) {
            render_elements.extend(
                space
                    .render_elements_for_region(renderer, &output_geo, output_scale, alpha)
                    .into_iter()
                    .map(|e| SpaceRenderElements::Element(Wrap::from(e))),
            );
        }
    }

    #[cfg(feature = "wayland_frontend")]
    render_elements.extend(
        lower
            .into_iter()
            .filter_map(|surface| layer_map.layer_geometry(surface).map(|geo| (geo.loc, surface)))
            .flat_map(|(loc, surface)| {
                AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                    surface,
                    renderer,
                    loc.to_physical_precise_round(output_scale),
                    Scale::from(output_scale),
                    alpha,
                )
                .into_iter()
                .map(SpaceRenderElements::Surface)
            }),
    );

    Ok(render_elements)
}

/// Render a output
///
/// If multiple spaces are given their elements will be stacked
/// the same way.
#[allow(clippy::too_many_arguments)]
#[profiling::function]
pub fn render_output<
    'a,
    #[cfg(feature = "wayland_frontend")] R: Renderer + ImportAll,
    #[cfg(not(feature = "wayland_frontend"))] R: Renderer,
    C: RenderElement<R>,
    E: SpaceElement + PartialEq + AsRenderElements<R> + 'a,
    S: IntoIterator<Item = &'a Space<E>>,
>(
    output: &Output,
    renderer: &mut R,
    alpha: f32,
    age: usize,
    spaces: S,
    custom_elements: &'a [C],
    damage_tracker: &mut OutputDamageTracker,
    clear_color: [f32; 4],
) -> Result<RenderOutputResult, OutputDamageTrackerError<R>>
where
    <R as Renderer>::TextureId: Texture + 'static,
    <E as AsRenderElements<R>>::RenderElement: 'a,
    SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>:
        From<Wrap<<E as AsRenderElements<R>>::RenderElement>>,
{
    if let OutputModeSource::Auto(renderer_output) = damage_tracker.mode() {
        assert!(renderer_output == output);
    }

    let space_render_elements = space_render_elements(renderer, spaces, output, alpha)?;

    let mut render_elements: Vec<OutputRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement, C>> =
        Vec::with_capacity(custom_elements.len() + space_render_elements.len());

    render_elements.extend(custom_elements.iter().map(OutputRenderElements::Custom));
    render_elements.extend(space_render_elements.into_iter().map(OutputRenderElements::Space));

    damage_tracker.render_output(renderer, age, &render_elements, clear_color)
}
