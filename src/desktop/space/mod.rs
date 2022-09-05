//! This module contains the [`Space`] helper class as well has related
//! rendering helpers to add custom elements or different clients to a space.

use crate::{
    backend::renderer::{Frame, ImportAll, Renderer},
    desktop::{
        layer::{layer_map_for_output, LayerSurface},
        popup::PopupManager,
        utils::{output_leave, output_update},
        window::Window,
    },
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Transform},
    wayland::compositor::{get_parent, is_sync_subsurface, with_surface_tree_downward, TraversalAction},
};
use indexmap::{IndexMap, IndexSet};
use std::{collections::VecDeque, fmt};
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle, Resource};

mod element;
mod layer;
mod output;
mod popup;
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
            let loc = window_loc(w, &self.id) - w.geometry().loc;
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
            // resets last_seen and old_damage, if remapped
            ..Default::default()
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

    /// Render a given [`Output`] using a given [`Renderer`].
    ///
    /// [`Space`] will render all mapped [`Window`]s, mapped [`LayerSurface`](super::LayerSurface)s
    /// of the given [`Output`] and their popups (if tracked by a [`PopupManager`](super::PopupManager)).
    /// `clear_color` will be used to fill all unoccupied regions.
    ///
    /// Rendering using this function will automatically apply damage-tracking.
    /// To facilitate this you need to provide age values of the buffers bound to
    /// the given `renderer`. If you stop using `Space` temporarily for rendering
    /// or apply additional rendering operations, you need to reset the age values
    /// accordingly as `Space` will be unable to track your custom rendering operations
    /// to avoid rendering artifacts.
    ///
    /// To add aditional elements without breaking damage-tracking implement the `RenderElement`
    /// trait and use `custom_elements` to provide them to this function. `custom_elements are rendered
    /// after every other element.
    ///
    /// Returns a list of updated regions relative to the rendered output
    /// (or `None` if that list would be empty) in case of success.
    pub fn render_output<R, E>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        age: usize,
        clear_color: [f32; 4],
        custom_elements: &[E],
    ) -> Result<Option<Vec<Rectangle<i32, Physical>>>, RenderError<R>>
    where
        R: Renderer + ImportAll,
        R::TextureId: 'static,
        E: RenderElement<R>,
    {
        if !self.outputs.contains(output) {
            return Err(RenderError::UnmappedOutput);
        }

        let mut state = output_state(self.id, output);
        let output_size = output.current_mode().ok_or(RenderError::OutputNoMode)?.size;
        let output_scale = output.current_scale().fractional_scale();
        // We explicitly use ceil for the output geometry size to make sure the damage
        // spans at least the output size. Round and floor would result in parts not drawn as the
        // frame size could be bigger than the maximum the output_geo would define.
        let output_geo = Rectangle::from_loc_and_size(
            state.location.to_physical_precise_round(output_scale),
            output_size,
        );
        let layer_map = layer_map_for_output(output);

        let window_popups = self
            .windows
            .iter()
            .flat_map(|w| w.popup_elements(self.id))
            .collect::<Vec<_>>();
        let layer_popups = layer_map
            .layers()
            .flat_map(|l| l.popup_elements(self.id))
            .collect::<Vec<_>>();

        let mut render_elements: Vec<SpaceElement<'_, R, E>> = Vec::with_capacity(
            custom_elements.len()
                + layer_map.len()
                + self.windows.len()
                + window_popups.len()
                + layer_popups.len(),
        );

        render_elements.extend(
            custom_elements
                .iter()
                .map(|e| SpaceElement::Custom(e, std::marker::PhantomData)),
        );
        render_elements.extend(self.windows.iter().map(SpaceElement::Window));
        render_elements.extend(window_popups.iter().map(SpaceElement::Popup));
        render_elements.extend(layer_map.layers().map(SpaceElement::Layer));
        render_elements.extend(layer_popups.iter().map(SpaceElement::Popup));

        render_elements.sort_by_key(|e| e.z_index(self.id));

        let opaque_regions = render_elements
            .iter()
            .enumerate()
            .filter_map(|(zindex, element)| {
                element
                    .opaque_regions(self.id, output_scale)
                    .map(|regions| (zindex, regions))
            })
            .collect::<Vec<_>>();

        // This will hold all the damage we need for this rendering step
        let mut damage = Vec::<Rectangle<i32, Physical>>::new();

        // First add damage for windows gone
        for old_toplevel in state
            .last_toplevel_state
            .iter()
            .filter_map(|(id, state)| {
                if !render_elements.iter().any(|e| ToplevelId::from(e) == *id) {
                    Some(state.1)
                } else {
                    None
                }
            })
            .collect::<Vec<Rectangle<i32, Physical>>>()
        {
            slog::trace!(self.logger, "Removing toplevel at: {:?}", old_toplevel);
            damage.push(old_toplevel);
        }

        // lets iterate front to back and figure out, what new windows or unmoved windows we have
        for (zindex, element) in render_elements.iter().enumerate() {
            let geo = element.geometry(self.id, output_scale);
            let old_state = state.last_toplevel_state.get(&ToplevelId::from(element)).cloned();

            let mut element_damage = element.accumulated_damage(self.id, output_scale, Some((self, output)));

            // window was moved, resized or just appeared
            if old_state
                .map(|(old_zindex, old_geo)| old_geo != geo || zindex != old_zindex)
                .unwrap_or(true)
            {
                slog::trace!(self.logger, "Toplevel geometry changed, damaging previous and current geometry. previous geometry: {:?}, current geometry: {:?}", old_state, geo);
                // Add damage for the old position of the window
                if let Some((_, old_geo)) = old_state {
                    element_damage.push(old_geo);
                }
                element_damage.push(geo);
            }

            let element_damage = opaque_regions
                .iter()
                .filter(|(index, _)| *index > zindex)
                .flat_map(|(_, regions)| regions)
                .fold(element_damage, |damage, region| {
                    damage
                        .into_iter()
                        .flat_map(|geo| geo.subtract_rect(*region))
                        .collect::<Vec<_>>()
                })
                .into_iter()
                .collect::<Vec<_>>();

            // add the damage as reported by the element
            damage.extend(element_damage);
        }

        if state.last_output_geo.map(|geo| geo != output_geo).unwrap_or(true) {
            // The output geometry changed, so just damage everything
            slog::trace!(self.logger, "Output geometry changed, damaging whole output geometry. previous geometry: {:?}, current geometry: {:?}", state.last_output_geo, output_geo);
            damage = vec![output_geo];
        }

        // That is all completely new damage, which we need to store for subsequent renders
        let new_damage = damage.clone();
        // We now add old damage states, if we have an age value
        if age > 0 && state.old_damage.len() >= age {
            // We do not need even older states anymore
            state.old_damage.truncate(age);
            damage.extend(state.old_damage.iter().flatten().copied());
        } else {
            // just damage everything, if we have no damage
            damage = vec![output_geo];
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

        let output_transform: Transform = output.current_transform();
        let res = renderer.render(
            output_transform.transform_size(output_size),
            output_transform,
            |renderer, frame| {
                let clear_damage = opaque_regions
                    .iter()
                    .flat_map(|(_, regions)| regions)
                    .fold(damage.clone(), |damage, region| {
                        damage
                            .into_iter()
                            .flat_map(|geo| geo.subtract_rect(*region))
                            .collect::<Vec<_>>()
                    })
                    .into_iter()
                    .map(|geo| Rectangle::from_loc_and_size(geo.loc - output_geo.loc, geo.size))
                    .collect::<Vec<_>>();

                // First clear all damaged regions
                slog::trace!(self.logger, "Clearing at {:#?}", clear_damage);
                frame.clear(clear_color, &clear_damage)?;
                // Then re-draw all windows & layers overlapping with a damage rect.
                for (zindex, element) in render_elements.iter().enumerate() {
                    let geo = element.geometry(self.id, output_scale);

                    let element_damage = opaque_regions
                        .iter()
                        .filter(|(index, _)| *index > zindex)
                        .flat_map(|(_, regions)| regions)
                        .fold(damage.clone(), |damage, region| {
                            damage
                                .into_iter()
                                .flat_map(|geo| geo.subtract_rect(*region))
                                .collect::<Vec<_>>()
                        })
                        .into_iter()
                        .map(|geo| Rectangle::from_loc_and_size(geo.loc - output_geo.loc, geo.size))
                        .collect::<Vec<_>>();

                    let element_geo = Rectangle::from_loc_and_size(geo.loc - output_geo.loc, geo.size);
                    if element_damage.iter().any(|d| d.overlaps(element_geo)) {
                        let loc = element.location(self.id, output_scale);
                        slog::trace!(
                            self.logger,
                            "Rendering toplevel with index {} at {:?} with damage {:#?}",
                            zindex,
                            element_geo,
                            element_damage
                        );
                        element.draw(
                            self.id,
                            renderer,
                            frame,
                            output_scale,
                            loc - output_geo.loc.to_f64(),
                            &element_damage,
                            &self.logger,
                        )?;
                    }
                }

                Result::<(), R::Error>::Ok(())
            },
        );

        if let Err(err) = res {
            // if the rendering errors on us, we need to be prepared, that this whole buffer was partially updated and thus now unusable.
            // thus clean our old states before returning
            state.old_damage = VecDeque::new();
            state.last_toplevel_state = IndexMap::new();
            return Err(RenderError::Rendering(err));
        }

        // If rendering was successful capture the state and add the damage
        state.last_toplevel_state = render_elements
            .iter()
            .enumerate()
            .map(|(zindex, elem)| {
                let geo = elem.geometry(self.id, output_scale);
                (ToplevelId::from(elem), (zindex, geo))
            })
            .collect();
        state.old_damage.push_front(new_damage.clone());
        state.last_output_geo = Some(output_geo);

        Ok(Some(
            new_damage
                .into_iter()
                .map(|mut geo| {
                    geo.loc -= output_geo.loc;
                    geo
                })
                .collect(),
        ))
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

/// Errors thrown by [`Space::render_output`]
#[derive(thiserror::Error)]
pub enum RenderError<R: Renderer> {
    /// The provided [`Renderer`] did return an error during an operation
    #[error(transparent)]
    Rendering(R::Error),
    /// The given [`Output`] has no set mode
    #[error("Output has no active mode")]
    OutputNoMode,
    /// The given [`Output`] is not mapped to this [`Space`].
    #[error("Output was not mapped to this space")]
    UnmappedOutput,
}

impl<R: Renderer> fmt::Debug for RenderError<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Rendering(err) => fmt::Debug::fmt(err, f),
            RenderError::OutputNoMode => f.write_str("Output has no active move"),
            RenderError::UnmappedOutput => f.write_str("Output was not mapped to this space"),
        }
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! custom_elements_internal {
    (@enum $vis:vis $name:ident; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $vis enum $name {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher(std::convert::Infallible),
        }
    };
    (@enum $vis:vis $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $vis enum $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
        {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher((std::marker::PhantomData<$renderer>, std::convert::Infallible)),
        }
    };
    (@call $renderer:ty; $name:ident; $($x:ident),*) => {
        $crate::desktop::space::RenderElement::<$renderer>::$name($($x),*)
    };
    (@call $renderer:ty as $other:ty; draw; $x:ident, $renderer_ref:ident, $frame:ident, $($tail:ident),*) => {
        $crate::desktop::space::RenderElement::<$other>::draw($x, $renderer_ref.as_mut(), $frame.as_mut(), $($tail),*).map_err(Into::into)
    };
    (@call $renderer:ty as $other:ty; $name:ident; $($x:ident),*) => {
        $crate::desktop::space::RenderElement::<$other>::$name($($x),*)
    };
    (@body $renderer:ty; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn id(&self) -> usize {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; id; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn type_of(&self) -> std::any::TypeId {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; type_of; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn location(&self, scale: impl Into<$crate::utils::Scale<f64>>) -> $crate::utils::Point<f64, $crate::utils::Physical> {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; location; x, scale)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn geometry(&self, scale: impl Into<$crate::utils::Scale<f64>>) -> $crate::utils::Rectangle<i32, $crate::utils::Physical> {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; geometry; x, scale)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn accumulated_damage(&self, scale: impl Into<$crate::utils::Scale<f64>>, for_values: Option<$crate::desktop::space::SpaceOutputTuple<'_, '_>>) -> Vec<$crate::utils::Rectangle<i32, $crate::utils::Physical>> {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; accumulated_damage; x, scale, for_values)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn opaque_regions(&self, scale: impl Into<$crate::utils::Scale<f64>>) -> Option<Vec<$crate::utils::Rectangle<i32, $crate::utils::Physical>>> {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; opaque_regions; x, scale)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn z_index(&self) -> u8 {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; z_index; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@draw <$renderer:ty>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn draw(
            &self,
            renderer: &mut $renderer,
            frame: &mut <$renderer as $crate::backend::renderer::Renderer>::Frame,
            scale: impl Into<$crate::utils::Scale<f64>>,
            location: $crate::utils::Point<f64, $crate::utils::Physical>,
            damage: &[$crate::utils::Rectangle<i32, $crate::utils::Physical>],
            log: &slog::Logger,
        ) -> Result<(), <$renderer as $crate::backend::renderer::Renderer>::Error>
        where
        $(
            $(
                $renderer: std::convert::AsMut<$other_renderer>,
                <$renderer as $crate::backend::renderer::Renderer>::Frame: std::convert::AsMut<<$other_renderer as $crate::backend::renderer::Renderer>::Frame>,
                <$other_renderer as $crate::backend::renderer::Renderer>::Error: Into<<$renderer as $crate::backend::renderer::Renderer>::Error>,
            )*
        )*
        {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, renderer, frame, scale, location, damage, log)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@draw $renderer:ty; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn draw(
            &self,
            renderer: &mut $renderer,
            frame: &mut <$renderer as $crate::backend::renderer::Renderer>::Frame,
            scale: impl Into<$crate::utils::Scale<f64>>,
            location: $crate::utils::Point<f64, $crate::utils::Physical>,
            damage: &[$crate::utils::Rectangle<i32, $crate::utils::Physical>],
            log: &slog::Logger,
        ) -> Result<(), <$renderer as $crate::backend::renderer::Renderer>::Error>
        {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, renderer, frame, scale, location, damage, log)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@impl $name:ident<$renderer:ident>; $($tail:tt)*) => {
        impl<$renderer> $crate::desktop::space::RenderElement<$renderer> for $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll + 'static,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::custom_elements_internal!(@body $renderer; $($tail)*);
            $crate::custom_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident; $renderer:ident; $($tail:tt)*) => {
        impl<$renderer> $crate::desktop::space::RenderElement<$renderer> for $name
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll + 'static,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::custom_elements_internal!(@body $renderer; $($tail)*);
            $crate::custom_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        impl $crate::desktop::space::RenderElement<$renderer> for $name
        {
            $crate::custom_elements_internal!(@body $renderer; $($tail)*);
            $crate::custom_elements_internal!(@draw $renderer; $($tail)*);
        }
    };
    (@from $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl<$renderer> From<$field> for $name<$renderer>
            where
                $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
                $(
                    $($renderer: std::convert::AsMut<$other_renderer>,)?
                )*
            {
                fn from(field: $field) -> $name<$renderer> {
                    $name::$body(field)
                }
            }
        )*
    };
    (@from $name:ident; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl From<$field> for $name {
                fn from(field: $field) -> $name {
                    $name::$body(field)
                }
            }
        )*
    };
}

/// Macro to collate multiple [`crate::desktop::space::RenderElement`]-implementations
/// into one type to be used with [`Space::render_output`].
/// ## Example
///
/// ```no_run
/// # use wayland_server::{Display, DisplayHandle};
/// use smithay::{
///     backend::renderer::{Texture, Renderer, ImportAll},
///     desktop::space::{SurfaceTree, Space, SpaceOutputTuple, RenderElement},
///     utils::{Point, Size, Rectangle, Transform, Logical, Scale},
/// };
/// use slog::Logger;
///
/// # use smithay::{
/// #   backend::SwapBuffersError,
/// #   backend::renderer::{TextureFilter, Frame},
/// #   reexports::wayland_server::protocol::wl_buffer,
/// #   wayland::compositor::SurfaceData,
/// #   utils::{Buffer, Physical},
/// # };
/// # struct DummyRenderer;
/// # struct DummyFrame;
/// # struct DummyError;
/// # struct DummyTexture;
/// # impl Renderer for DummyRenderer {
/// #    type Error = smithay::backend::SwapBuffersError;
/// #    type TextureId = DummyTexture;
/// #    type Frame = DummyFrame;
/// #    fn id(&self) -> usize { 0 }
/// #    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> { Ok(()) }
/// #    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> { Ok(()) }
/// #    fn render<F, R>(
/// #        &mut self,
/// #        size: Size<i32, Physical>,
/// #        dst_transform: Transform,
/// #        rendering: F,
/// #    ) -> Result<R, Self::Error>
/// #    where
/// #        F: FnOnce(&mut Self, &mut Self::Frame) -> R
/// #    {
/// #       Ok(rendering(self, &mut DummyFrame))
/// #    }
/// # }
/// # impl ImportAll for DummyRenderer {
/// #    fn import_buffer(
/// #        &mut self,
/// #        buffer: &wl_buffer::WlBuffer,
/// #        surface: Option<&SurfaceData>,
/// #        damage: &[Rectangle<i32, Buffer>],
/// #    ) -> Option<Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>> { None }
/// # }
/// # impl Texture for DummyTexture {
/// #    fn width(&self) -> u32 { 0 }
/// #    fn height(&self) -> u32 { 0 }
/// # }
/// # impl Frame for DummyFrame {
/// #   type Error = SwapBuffersError;
/// #   type TextureId = DummyTexture;
/// #   fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> { Ok(()) }
/// #   #[allow(clippy::too_many_arguments)]
/// #   fn render_texture_at(
/// #       &mut self,
/// #       texture: &Self::TextureId,
/// #       pos: Point<i32, Physical>,
/// #       texture_scale: i32,
/// #       output_scale: impl Into<Scale<f64>>,
/// #       src_transform: Transform,
/// #       damage: &[Rectangle<i32, Physical>],
/// #       alpha: f32,
/// #   ) -> Result<(), Self::Error> {
/// #       Ok(())
/// #   }
/// #   fn render_texture_from_to(
/// #       &mut self,
/// #       texture: &Self::TextureId,
/// #       src: Rectangle<f64, Buffer>,
/// #       dst: Rectangle<i32, Physical>,
/// #       damage: &[Rectangle<i32, Physical>],
/// #       src_transform: Transform,
/// #       alpha: f32,
/// #   ) -> Result<(), Self::Error> {
/// #       Ok(())   
/// #   }
/// #   fn transformation(&self) -> Transform { Transform::Normal }
/// # }
///
/// smithay::custom_elements! {
///     CustomElem; // name of the new type
///     SurfaceTree=SurfaceTree, // <variant name> = <type to collate>
/// };
///
/// smithay::custom_elements! {
///     CustomElemGeneric<R>; // You can make it generic over renderers!
///     SurfaceTree=SurfaceTree,
///     PointerElement=PointerElement<<R as Renderer>::TextureId>, // and then use R in your types
/// };
///
/// smithay::custom_elements! {
///     CustomElemExplicit<=DummyRenderer>; // You can make it only usable with one renderer
///     // that is particulary useful, if your renderer has lifetimes (e.g. MultiRenderer)
///     SurfaceTree=SurfaceTree,
///     PointerElement=PointerElement<DummyTexture>, // and then you use a concrete and matching texture type
/// };
/// // in case your renderer wraps another renderer and implements `std::convert::AsMut`
/// // you can also use `RenderElement`-implementations requiring the wrapped renderer
/// // by writing something like: `EguiFrame=EguiFrame as <Gles2Renderer>` where `as <renderer>`
/// // denotes the type of the wrapped renderer.
///
/// pub struct PointerElement<T: Texture> {
///    texture: T,
///    position: Point<i32, Logical>,
///    size: Size<i32, Logical>,
/// }
///
/// impl<T: Texture> PointerElement<T> {
///    pub fn new(texture: T, pointer_pos: Point<i32, Logical>) -> PointerElement<T> {
///        let size = texture.size().to_logical(1, Transform::Normal);
///        PointerElement {
///            texture,
///            position: pointer_pos,
///            size,
///        }
///    }
/// }
///
///# impl<R> RenderElement<R> for PointerElement<<R as Renderer>::TextureId>
///# where
///#    R: Renderer + ImportAll,
///#    <R as Renderer>::TextureId: 'static,
///# {
///#    fn id(&self) -> usize {
///#        0
///#    }
///#
///#    fn location(&self, scale: impl Into<Scale<f64>>) -> Point<f64, Physical> {
///#        self.position.to_f64().to_physical(scale)
///#    }
///#
///#    fn geometry(&self, scale: impl Into<Scale<f64>>) -> Rectangle<i32, Physical> {
///#        Rectangle::from_loc_and_size(self.position, self.size).to_f64().to_physical(scale).to_i32_round()
///#    }
///#
///#    fn accumulated_damage(&self, _: impl Into<Scale<f64>>, _: Option<SpaceOutputTuple<'_, '_>>) -> Vec<Rectangle<i32, Physical>> {
///#        vec![]
///#    }
///#
///#    fn opaque_regions(&self, scale: impl Into<Scale<f64>>) -> Option<Vec<Rectangle<i32, Physical>>> {
///#        None
///#    }
///#
///#    fn draw(
///#        &self,
///#        _renderer: &mut R,
///#        frame: &mut <R as Renderer>::Frame,
///#        scale: impl Into<Scale<f64>>,
///#        location: Point<f64, Physical>,
///#        damage: &[Rectangle<i32, Physical>],
///#        _log: &Logger,
///#    ) -> Result<(), <R as Renderer>::Error> {
///#        Ok(())
///#    }
///# }
///# // just placeholders
///# let mut renderer = DummyRenderer;
///# let texture = DummyTexture;
///# let output = unsafe { std::mem::zeroed() };
///# let surface_tree: SurfaceTree = unsafe { std::mem::zeroed() };
///# let mut space = Space::new(None);
///# let age = 0;
///# let display = Display::<()>::new().unwrap();
///# let dh = display.handle();
///
/// let elements = [CustomElem::from(surface_tree)];
/// space.render_output(&mut renderer, &output, age, [0.0, 0.0, 0.0, 1.0], &elements);
/// ```
#[macro_export]
macro_rules! custom_elements {
    ($vis:vis $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        $crate::custom_elements_internal!(@enum $vis $name; $($tail)*);
        $crate::custom_elements_internal!(@impl $name<=$renderer>; $($tail)*);
        $crate::custom_elements_internal!(@from $name; $($tail)*);

    };
    ($vis:vis $name:ident<$renderer:ident>; $($tail:tt)*) => {
        $crate::custom_elements_internal!(@enum $vis $name<$renderer>; $($tail)*);
        $crate::custom_elements_internal!(@impl $name<$renderer>; $($tail)*);
        $crate::custom_elements_internal!(@from $name<$renderer>; $($tail)*);
    };
    ($vis:vis $name:ident; $($tail:tt)*) => {
        $crate::custom_elements_internal!(@enum $vis $name; $($tail)*);
        $crate::custom_elements_internal!(@impl $name; R; $($tail)*);
        $crate::custom_elements_internal!(@from $name; $($tail)*);
    };
}
