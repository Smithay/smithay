//! This module contains the [`Space`] helper class as well has related
//! rendering helpers to add custom elements or different clients to a space.

use crate::{
    backend::renderer::{
        damage::{
            DamageTrackedRenderer, DamageTrackedRendererError, DamageTrackedRendererMode, OutputNoMode,
        },
        element::{surface::WaylandSurfaceRenderElement, AsRenderElements, RenderElement},
        ImportAll, Renderer, Texture,
    },
    desktop::layer::{layer_map_for_output, LayerSurface},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Transform},
};
use std::{collections::HashSet, fmt};
use wayland_server::protocol::wl_surface::WlSurface;

mod element;
mod layer;
mod output;
mod window;

pub use self::element::*;
use self::output::*;

use super::WindowSurfaceType;

crate::utils::ids::id_gen!(next_space_id, SPACE_ID, SPACE_IDS);

#[derive(Debug)]
struct InnerElement<E> {
    element: E,
    location: Point<i32, Logical>,
    outputs: HashSet<Output>,
}

/// Represents two dimensional plane to map windows and outputs upon.
#[derive(Debug)]
pub struct Space<E: SpaceElement> {
    pub(super) id: usize,
    // in z-order, back to front
    elements: Vec<InnerElement<E>>,
    outputs: Vec<Output>,
    _logger: ::slog::Logger,
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

impl<E: SpaceElement + PartialEq> Space<E> {
    /// Create a new [`Space`]
    pub fn new<L>(log: L) -> Self
    where
        L: Into<Option<slog::Logger>>,
    {
        Space {
            id: next_space_id(),
            elements: Vec::new(),
            outputs: Vec::new(),
            _logger: crate::slog_or_fallback(log),
        }
    }

    /// Gets the id of this space
    pub fn id(&self) -> usize {
        self.id
    }

    /// Map a [`Window`] and move it to top of the stack
    ///
    /// If a z_index is provided it will override the default
    /// z_index of [`RenderZindex::Shell`] for the mapped window.
    ///
    /// This can safely be called on an already mapped window
    /// to update its location or z_index inside the space.
    ///
    /// If activate is true it will set the new windows state
    /// to be activate and removes that state from every
    /// other mapped window.
    pub fn map_element<P>(&mut self, element: E, location: P, activate: bool)
    where
        P: Into<Point<i32, Logical>>,
    {
        if let Some(pos) = self.elements.iter().position(|inner| inner.element == element) {
            self.elements.swap_remove(pos);
        }

        let inner = InnerElement {
            element,
            location: location.into(),
            outputs: HashSet::new(),
        };
        self.insert_elem(inner, activate);
    }

    /// Moves an already mapped [`Window`] to top of the stack
    ///
    /// This function does nothing for unmapped windows.
    ///
    /// If activate is true it will set the new windows state
    /// to be activate and removes that state from every
    /// other mapped window.
    pub fn raise_element(&mut self, element: &E, activate: bool) {
        if let Some(pos) = self.elements.iter().position(|inner| &inner.element == element) {
            let inner = self.elements.swap_remove(pos);
            self.insert_elem(inner, activate);
        }
    }

    fn insert_elem(&mut self, elem: InnerElement<E>, activate: bool) {
        if activate {
            elem.element.set_activate(true);
            for e in self.elements.iter() {
                e.set_activate(false);
            }
        }

        self.elements.push(elem);
        self.elements
            .sort_by(|e1, e2| e1.z_index().cmp(&e2.element.z_index()));
    }

    /// Unmap a [`Window`] from this space.
    ///
    /// This function does nothing for already unmapped windows
    // TODO: Requirements for E? Also provide retain?
    pub fn unmap_elem(&mut self, element: &E) {
        if let Some(pos) = self.elements.iter().position(|inner| &inner.element == element) {
            self.elements.swap_remove(pos);
        }
    }

    /// Iterate window in z-order back to front
    pub fn elements(&self) -> impl DoubleEndedIterator<Item = &E> {
        self.elements.iter().map(|e| &e.element)
    }

    /// Finds the topmost surface under this point if any and returns it
    /// together with the location of this surface relative to this space
    /// and the window the surface belongs to.
    ///
    /// This is equivalent to iterating the windows in the space from
    /// top to bottom and calling [`Window::surface_under`] for each
    /// window and returning the first matching surface.
    /// As [`Window::surface_under`] internally uses the surface input regions
    /// the same applies to this method and it will only return a surface
    /// where the point is within the surface input regions.
    pub fn element_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> Option<(&E, Point<i32, Logical>)> {
        let point = point.into();
        self.elements.iter().rev().find_map(|e| {
            if e.input_region(&point) {
                Some((&e.element, e.location))
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

    /// Returns the layer matching a given surface, if any
    ///
    /// `surface_type` can be used to limit the types of surfaces queried for equality.
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

    /// Returns the location of a [`Window`] inside the Space.
    pub fn element_location(&self, elem: &E) -> Option<Point<i32, Logical>> {
        self.elements
            .iter()
            .find(|e| &e.element == elem)
            .map(|e| e.location)
    }

    /// Returns the bounding box of a [`Window`] including its relative position inside the Space.
    pub fn element_bbox(&self, elem: &E) -> Option<Rectangle<i32, Logical>> {
        self.elements
            .iter()
            .find(|e| &e.element == elem)
            .map(|e| e.bbox())
    }

    /// Maps an [`Output`] inside the space.
    ///
    /// Can be safely called on an already mapped
    /// [`Output`] to update its location.
    ///
    /// *Note:* Remapping an output does reset it's damage memory.
    pub fn map_output<P: Into<Point<i32, Logical>>>(&mut self, output: &Output, location: P) {
        let mut state = output_state(self.id, output);
        *state = OutputState {
            location: location.into(),
        };
        if !self.outputs.contains(output) {
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

    /// Returns all [`Output`]s a [`Window`] overlaps with.
    pub fn outputs_for_element(&self, elem: &E) -> Vec<Output> {
        if !self.elements.iter().any(|e| &e.element == elem) {
            return Vec::new();
        }

        self.elements
            .iter()
            .find(|e| &e.element == elem)
            .into_iter()
            .flat_map(|e| &e.outputs)
            .cloned()
            .collect()
    }

    /// Refresh some internal values and update client state,
    /// meaning this will handle output enter and leave events
    /// for mapped outputs and windows based on their position.
    ///
    /// Needs to be called periodically, at best before every
    /// wayland socket flush.
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
                // Check if the bounding box of the toplevel intersects with
                // the output
                if !output_geometry.overlaps(bbox) {
                    if e.outputs.remove(output) {
                        e.element.output_leave(output);
                    }
                } else if e.outputs.insert(output.clone()) {
                    e.element.output_enter(output);
                }
            }
        }

        self.elements.iter().for_each(SpaceElement::refresh);
    }

    /// Retrieve the render elements for an output
    pub fn elements_for_output<'a, R>(
        &'a self,
        output: &Output,
    ) -> Result<Vec<SpaceRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement>>, OutputError>
    where
        R: Renderer + ImportAll,
        <R as Renderer>::TextureId: Texture + 'static,
        E: AsRenderElements<R>,
        <E as AsRenderElements<R>>::RenderElement: 'a,
        SpaceRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement>:
            From<<E as AsRenderElements<R>>::RenderElement>,
    {
        if !self.outputs.contains(output) {
            return Err(OutputError::Unmapped);
        }

        let state = output_state(self.id, output);
        let output_size = output.current_mode().ok_or(OutputNoMode)?.size;
        let output_scale = output.current_scale().fractional_scale();
        let output_location = state.location;
        let output_geo = Rectangle::from_loc_and_size(
            state.location,
            output_size.to_f64().to_logical(output_scale).to_i32_ceil(),
        );

        let layer_map = layer_map_for_output(output);
        let mut space_elements: Vec<SpaceElements<'_, E>> = Vec::new();

        space_elements.extend(self.elements.iter().rev().map(SpaceElements::Element));

        space_elements.extend(layer_map.layers().rev().cloned().map(SpaceElements::Layer));

        space_elements.sort_by_key(|e| std::cmp::Reverse(e.z_index()));

        Ok(space_elements
            .into_iter()
            .filter(|e| {
                let geometry = e.bbox();
                output_geo.overlaps(geometry)
            })
            .flat_map(|e| {
                let location = e.bbox().loc - output_location;
                e.render_elements::<SpaceRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement>>(
                    location.to_physical_precise_round(output_scale),
                    Scale::from(output_scale),
                )
            })
            .collect::<Vec<_>>())
    }
}

/// Errors thrown by [`Space::render_elements_for_output`]
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

impl<E: SpaceElement> SpaceElement for InnerElement<E> {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = self.element.geometry();
        geo.loc += self.location;
        geo
    }

    fn bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = self.element.bbox();
        bbox.loc += self.location;
        bbox
    }

    fn input_region(&self, point: &Point<f64, Logical>) -> bool {
        self.element.input_region(&(*point - self.location.to_f64()))
    }
    fn z_index(&self) -> u8 {
        self.element.z_index()
    }

    fn set_activate(&self, activated: bool) {
        self.element.set_activate(activated)
    }
    fn output_enter(&self, output: &Output) {
        self.element.output_enter(output)
    }
    fn output_leave(&self, output: &Output) {
        self.element.output_leave(output)
    }

    fn refresh(&self) {
        self.element.refresh()
    }
}

crate::backend::renderer::element::render_elements! {
    /// Defines the render elements used internally by a [`Space`]
    ///
    /// Use them in place of `E` in `space_render_elements` or
    /// `render_output` if you do not need custom render elements
    pub SpaceRenderElements<'a, R, E>;
    /// A single wayland surface
    Surface=WaylandSurfaceRenderElement,
    /// A single texture
    Element=&'a E,
}

impl<'a, R, E> std::fmt::Debug for SpaceRenderElements<'a, R, E>
where
    R: Renderer + ImportAll,
    E: RenderElement<R> + std::fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            Self::Element(arg0) => f.debug_tuple("Element").field(arg0).finish(),
            Self::_GenericCatcher(_) => unreachable!(),
        }
    }
}

crate::backend::renderer::element::render_elements! {
    OutputRenderElements<'a, R, E, C>;
    Space=SpaceRenderElements<'a, R, E>,
    Custom=&'a C,
}

/// Get the render elements for a specific output
pub fn space_render_elements<'a, R, E>(
    spaces: &[&'a Space<E>],
    output: &Output,
) -> Result<Vec<SpaceRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement>>, OutputNoMode>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
    E: SpaceElement + PartialEq + AsRenderElements<R>,
    <E as AsRenderElements<R>>::RenderElement: 'a,
    SpaceRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement>:
        From<<E as AsRenderElements<R>>::RenderElement>,
{
    let mut render_elements = Vec::new();

    for space in spaces {
        match space.elements_for_output(output) {
            Ok(elements) => render_elements.extend(elements),
            Err(OutputError::Unmapped) => {}
            Err(OutputError::NoMode(_)) => return Err(OutputNoMode),
        }
    }

    Ok(render_elements)
}

/// Render a output
#[allow(clippy::too_many_arguments)]
pub fn render_output<'a, R, C, E>(
    output: &Output,
    renderer: &mut R,
    age: usize,
    spaces: &[&'a Space<E>],
    custom_elements: &'a [C],
    damage_tracked_renderer: &mut DamageTrackedRenderer,
    clear_color: [f32; 4],
    log: &slog::Logger,
) -> Result<Option<Vec<Rectangle<i32, Physical>>>, DamageTrackedRendererError<R>>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
    E: SpaceElement + PartialEq + AsRenderElements<R>,
    <E as AsRenderElements<R>>::RenderElement: 'a,
    SpaceRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement>:
        From<<E as AsRenderElements<R>>::RenderElement>,
    C: RenderElement<R>,
{
    if let DamageTrackedRendererMode::Auto(renderer_output) = damage_tracked_renderer.mode() {
        assert!(renderer_output == output);
    }

    let space_render_elements = space_render_elements(spaces, output)?;

    let mut render_elements: Vec<OutputRenderElements<'a, R, <E as AsRenderElements<R>>::RenderElement, C>> =
        Vec::with_capacity(custom_elements.len() + space_render_elements.len());

    render_elements.extend(custom_elements.iter().map(OutputRenderElements::Custom));
    render_elements.extend(space_render_elements.into_iter().map(OutputRenderElements::Space));

    damage_tracked_renderer.render_output(renderer, age, &*render_elements, clear_color, log)
}
