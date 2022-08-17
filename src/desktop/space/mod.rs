//! This module contains the [`Space`] helper class as well has related
//! rendering helpers to add custom elements or different clients to a space.

use crate::{
    backend::renderer::{
        output::{
            element::{surface::WaylandSurfaceRenderElement, texture::TextureRenderElement, RenderElement},
            OutputRender, OutputRenderError,
        },
        ImportAll, Renderer, Texture, gles2::Gles2Renderer,
    },
    desktop::{
        layer::{layer_map_for_output, LayerSurface},
        popup::PopupManager,
        utils::{output_leave, output_update},
        window::Window,
    },
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Transform},
    wayland::{
        compositor::{get_parent, is_sync_subsurface, with_surface_tree_downward, TraversalAction},
        output::Output,
    },
};
use indexmap::IndexSet;
use std::{fmt, marker::PhantomData};
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle, Resource};

mod element;
mod layer;
mod output;
mod window;

pub use self::element::*;
use self::output::*;
use self::window::*;

use super::WindowSurfaceType;

crate::utils::ids::id_gen!(next_space_id, SPACE_ID, SPACE_IDS);

/// Represents two dimensional plane to map windows and outputs upon.
#[derive(Debug)]
pub struct Space {
    pub(super) id: usize,
    // in z-order, back to front
    windows: IndexSet<Window>,
    outputs: Vec<Output>,
    logger: ::slog::Logger,
}

impl PartialEq for Space {
    fn eq(&self, other: &Space) -> bool {
        self.id == other.id
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        SPACE_IDS.lock().unwrap().remove(&self.id);
    }
}

impl Space {
    /// Create a new [`Space`]
    pub fn new<L>(log: L) -> Space
    where
        L: Into<Option<slog::Logger>>,
    {
        Space {
            id: next_space_id(),
            windows: IndexSet::new(),
            outputs: Vec::new(),
            logger: crate::slog_or_fallback(log),
        }
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
    pub fn map_window<P, Z>(&mut self, window: &Window, location: P, z_index: Z, activate: bool)
    where
        P: Into<Point<i32, Logical>>,
        Z: Into<Option<u8>>,
    {
        let z_index = z_index.into().unwrap_or(RenderZindex::Shell as u8);
        {
            let mut state = window_state(self.id, window);
            state.location = location.into();
            state.z_index = z_index;
        }
        self.insert_window(window, activate);
    }

    /// Moves an already mapped [`Window`] to top of the stack
    ///
    /// This function does nothing for unmapped windows.
    ///
    /// If activate is true it will set the new windows state
    /// to be activate and removes that state from every
    /// other mapped window.
    pub fn raise_window(&mut self, window: &Window, activate: bool) {
        if self.windows.shift_remove(window) {
            self.insert_window(window, activate);
        }
    }

    fn insert_window(&mut self, window: &Window, activate: bool) {
        self.windows.insert(window.clone());
        self.windows.sort_by(|w1, w2| {
            window_state(self.id, w1)
                .z_index
                .cmp(&window_state(self.id, w2).z_index)
        });

        if activate {
            window.set_activated(true);
            for w in self.windows.iter() {
                if w != window {
                    w.set_activated(false);
                }
            }
        }
    }

    /// Unmap a [`Window`] from this space.
    ///
    /// This function does nothing for already unmapped windows
    pub fn unmap_window(&mut self, window: &Window) {
        if let Some(map) = window.user_data().get::<WindowUserdata>() {
            map.borrow_mut().remove(&self.id);
        }
        self.windows.shift_remove(window);
    }

    /// Iterate window in z-order back to front
    pub fn windows(&self) -> impl DoubleEndedIterator<Item = &Window> {
        self.windows.iter()
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
    pub fn surface_under<P: Into<Point<f64, Logical>>>(
        &self,
        point: P,
        surface_type: WindowSurfaceType,
    ) -> Option<(Window, WlSurface, Point<i32, Logical>)> {
        let point = point.into();
        for window in self.windows.iter().rev() {
            let loc = window_loc(window, &self.id) - window.geometry().loc;
            let mut geo = window.bbox_with_popups();
            geo.loc += loc;

            if !geo.to_f64().contains(point) {
                continue;
            }

            if let Some((surface, location)) = window.surface_under(point - loc.to_f64(), surface_type) {
                return Some((window.clone(), surface, location + loc));
            }
        }

        None
    }

    /// Get a reference to the window under a given point, if any
    pub fn window_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> Option<&Window> {
        let point = point.into();
        self.windows.iter().rev().find(|w| {
            let loc = window_loc(w, &self.id) - Window::geometry(w).loc;
            let mut geo = w.bbox();
            geo.loc += loc;
            geo.to_f64().contains(point)
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

    /// Returns the window matching a given surface, if any.
    ///
    /// `surface_type` can be used to limit the types of surfaces queried for equality.
    pub fn window_for_surface(
        &self,
        surface: &WlSurface,
        surface_type: WindowSurfaceType,
    ) -> Option<&Window> {
        if !surface.alive() {
            return None;
        }

        if surface_type.contains(WindowSurfaceType::TOPLEVEL) {
            if let Some(window) = self.windows.iter().find(|w| w.toplevel().wl_surface() == surface) {
                return Some(window);
            }
        }

        if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
            use std::sync::atomic::{AtomicBool, Ordering};

            if let Some(window) = self.windows.iter().find(|w| {
                let toplevel = w.toplevel().wl_surface();
                let found = AtomicBool::new(false);
                with_surface_tree_downward(
                    toplevel,
                    surface,
                    |_, _, search| TraversalAction::DoChildren(search),
                    |s, _, search| {
                        found.fetch_or(s == *search, Ordering::SeqCst);
                    },
                    |_, _, _| !found.load(Ordering::SeqCst),
                );
                found.load(Ordering::SeqCst)
            }) {
                return Some(window);
            }
        }

        if surface_type.contains(WindowSurfaceType::POPUP) {
            if let Some(window) = self.windows.iter().find(|w| {
                PopupManager::popups_for_surface(w.toplevel().wl_surface())
                    .any(|(p, _)| p.wl_surface() == surface)
            }) {
                return Some(window);
            }
        }

        None
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
    pub fn window_location(&self, w: &Window) -> Option<Point<i32, Logical>> {
        if !self.windows.contains(w) {
            return None;
        }

        Some(window_loc(w, &self.id))
    }

    /// Returns the bounding box of a [`Window`] including its relative position inside the Space.
    pub fn window_bbox(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.windows.contains(w) {
            return None;
        }

        Some(window_rect(w, &self.id))
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
            // keep surfaces, we still need to inform them of leaving,
            // if they don't overlap anymore during refresh.
            surfaces: std::mem::take(&mut state.surfaces),
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

        let transform: Transform = o.current_transform().into();
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
    pub fn outputs_for_window(&self, w: &Window) -> Vec<Output> {
        if !self.windows.contains(w) {
            return Vec::new();
        }

        self.outputs
            .iter()
            .filter(|o| {
                let output_state = output_state(self.id, o);
                output_state.surfaces.contains(&w.toplevel().wl_surface().id())
            })
            .cloned()
            .collect()
    }

    /// Refresh some internal values and update client state,
    /// meaning this will handle output enter and leave events
    /// for mapped outputs and windows based on their position.
    ///
    /// Needs to be called periodically, at best before every
    /// wayland socket flush.
    pub fn refresh(&mut self, dh: &DisplayHandle) {
        self.windows.retain(|w| w.alive());

        for output in &mut self.outputs {
            output_state(self.id, output)
                .surfaces
                .retain(|i| dh.backend_handle().object_info(i.clone()).is_ok());
        }

        for window in &self.windows {
            let bbox = window_rect(window, &self.id);
            let kind = window.toplevel();

            for output in &self.outputs {
                let output_geometry = self
                    .output_geometry(output)
                    .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (0, 0)));
                let mut output_state = output_state(self.id, output);

                // Check if the bounding box of the toplevel intersects with
                // the output, if not no surface in the tree can intersect with
                // the output.
                if !output_geometry.overlaps(bbox) {
                    let surface = kind.wl_surface();
                    output_leave(dh, output, &mut output_state.surfaces, surface, &self.logger);
                    continue;
                }

                let surface = kind.wl_surface();
                output_update(
                    dh,
                    output,
                    output_geometry,
                    &mut output_state.surfaces,
                    surface,
                    window_loc(window, &self.id),
                    &self.logger,
                );

                for (popup, location) in PopupManager::popups_for_surface(surface) {
                    let surface = popup.wl_surface();
                    let location = window_loc(window, &self.id) + window.geometry().loc + location
                        - popup.geometry().loc;
                    output_update(
                        dh,
                        output,
                        output_geometry,
                        &mut output_state.surfaces,
                        surface,
                        location,
                        &self.logger,
                    );
                }
            }
        }
    }

    /// Should be called on commit to let the space automatically call [`Window::refresh`]
    /// for the window that belongs to the given surface, if managed by this space.
    pub fn commit(&self, surface: &WlSurface) {
        if is_sync_subsurface(surface) {
            return;
        }
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if let Some(window) = self.windows().find(|w| w.toplevel().wl_surface() == &root) {
            window.refresh();
        }
    }

    /// Retrieve the render elements for an output
    pub fn elements_for_output<R, C, E>(
        &self,
        output: &Output,
        custom_elements: &[C],
    ) -> Result<Vec<E>, OutputError>
    where
        R: Renderer + ImportAll + 'static,
        <R as Renderer>::TextureId: Texture + 'static,
        C: SpaceElement<R, E>,
        E: RenderElement<R>
            + From<WaylandSurfaceRenderElement>
            + From<TextureRenderElement<<R as Renderer>::TextureId>>,
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

        let mut space_elements: Vec<SpaceElements<'_, C>> = Vec::new();

        space_elements.extend(
            custom_elements
                .iter()
                .map(SpaceElements::Custom)
                .collect::<Vec<_>>(),
        );

        space_elements.extend(
            self.windows()
                .rev()
                .map(SpaceElements::Window)
                .collect::<Vec<_>>(),
        );

        space_elements.extend(
            layer_map
                .layers()
                .rev()
                .map(SpaceElements::Layer)
                .collect::<Vec<_>>(),
        );

        space_elements.sort_by_key(|e| std::cmp::Reverse(e.z_index(self.id)));

        Ok(space_elements
            .into_iter()
            .filter(|e| {
                let geometry = e.geometry(self.id);
                output_geo.overlaps(geometry)
            })
            .flat_map(|e| {
                let location = e.location(self.id) - output_location;
                e.render_elements(
                    location.to_physical_precise_round(output_scale),
                    Scale::from(output_scale),
                )
            })
            .collect::<Vec<_>>())
    }

    /// Sends the frame callback to mapped [`Window`]s and [`LayerSurface`]s.
    pub fn send_frames(&self, time: u32) {
        for window in self.windows.iter() {
            window.send_frame(time);
        }

        for output in self.outputs.iter() {
            let map = layer_map_for_output(output);
            for layer in map.layers() {
                layer.send_frame(time);
            }
        }
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

/// The given [`Output`] has no set mode
#[derive(thiserror::Error, Debug)]
#[error("Output has no active mode")]
pub struct OutputNoMode;

crate::backend::renderer::output::element::render_elements! {
    /// Defines the render elements used internally by a [`Space`]
    ///
    /// Use them in place of `E` in `space_render_elements` or
    /// `render_output` if you do not need custom render elements
    pub SpaceRenderElements<R>;
    /// A single wayland surface
    Surface=WaylandSurfaceRenderElement,
    /// A single texture
    Texture=TextureRenderElement<<R as Renderer>::TextureId>
}

impl<R> std::fmt::Debug for SpaceRenderElements<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: std::fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            Self::Texture(arg0) => f.debug_tuple("Texture").field(arg0).finish(),
            Self::_GenericCatcher(_) => unreachable!(),
        }
    }
}

crate::backend::renderer::output::element::render_elements! {
    OutputRenderElements<'a, R, E>;
    Space=crate::backend::renderer::output::element::Wrap<E>,
    Custom=&'a E,
}

/// Get the render elements for a specific output
pub fn space_render_elements<R, C, E>(
    spaces: &[(&Space, &[C])],
    output: &Output,
) -> Result<Vec<E>, OutputNoMode>
where
    R: Renderer + ImportAll + 'static,
    C: SpaceElement<R, E>,
    E: RenderElement<R>
        + From<WaylandSurfaceRenderElement>
        + From<TextureRenderElement<<R as Renderer>::TextureId>>,
{
    let mut render_elements = Vec::new();

    for (space, custom_elements) in spaces {
        match space.elements_for_output(output, custom_elements) {
            Ok(elements) => render_elements.extend(elements),
            Err(OutputError::Unmapped) => {}
            Err(OutputError::NoMode(_)) => return Err(OutputNoMode),
        }
    }

    Ok(render_elements)
}

/// Render a output
pub fn render_output<R, C, E>(
    renderer: &mut R,
    age: usize,
    spaces: &[(&Space, &[C])],
    custom_elements: &[E],
    output_render: &mut OutputRender,
    log: &slog::Logger,
) -> Result<Option<Vec<Rectangle<i32, Physical>>>, OutputRenderError<R>>
where
    R: Renderer + ImportAll + 'static,
    <R as Renderer>::TextureId: Texture + 'static,
    C: SpaceElement<R, E>,
    E: RenderElement<R>
        + From<WaylandSurfaceRenderElement>
        + From<TextureRenderElement<<R as Renderer>::TextureId>>,
{
    let mut render_elements: Vec<OutputRenderElements<'_, R, E>> = Vec::new();

    let space_render_elements =
        space_render_elements(spaces, output_render.output()).map_err(|_| OutputRenderError::OutputNoMode)?;

    render_elements.extend(custom_elements.iter().map(OutputRenderElements::from));
    render_elements.extend(
        space_render_elements
            .into_iter()
            .map(crate::backend::renderer::output::element::Wrap::from)
            .map(OutputRenderElements::Space),
    );

    output_render.render_output(renderer, age, &*render_elements, log)
}
