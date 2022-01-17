//! This module contains the [`Space`] helper class as well has related
//! rendering helpers to add custom elements or different clients to a space.

use crate::{
    backend::renderer::{utils::SurfaceState, Frame, ImportAll, Renderer, Transform},
    desktop::{
        layer::{layer_map_for_output, LayerSurface},
        window::Window,
    },
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{
            get_parent, is_sync_subsurface, with_surface_tree_downward, SubsurfaceCachedState,
            TraversalAction,
        },
        output::Output,
        shell::wlr_layer::Layer as WlrLayer,
    },
};
use indexmap::{IndexMap, IndexSet};
use std::{cell::RefCell, collections::VecDeque, fmt};
use wayland_server::protocol::wl_surface::WlSurface;

mod element;
mod layer;
mod output;
mod popup;
mod window;

pub use self::element::*;
use self::layer::*;
use self::output::*;
use self::window::*;

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

/// Elements rendered by [`Space::render_output`] in addition to windows, layers and popups.
pub type DynamicRenderElements<R> =
    Box<dyn RenderElement<R, <R as Renderer>::Frame, <R as Renderer>::Error, <R as Renderer>::TextureId>>;

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
    /// This can safely be called on an already mapped window
    /// to update its location inside the space.
    ///
    /// If activate is true it will set the new windows state
    /// to be activate and removes that state from every
    /// other mapped window.
    pub fn map_window<P: Into<Point<i32, Logical>>>(&mut self, window: &Window, location: P, activate: bool) {
        self.insert_window(window, activate);
        window_state(self.id, window).location = location.into();
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

    /// Get a reference to the window under a given point, if any
    pub fn window_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> Option<&Window> {
        let point = point.into();
        self.windows.iter().rev().find(|w| {
            let bbox = window_rect(w, &self.id);
            bbox.to_f64().contains(point)
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

    /// Returns the window matching a given surface, if any
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<&Window> {
        if !surface.as_ref().is_alive() {
            return None;
        }

        self.windows
            .iter()
            .find(|w| w.toplevel().get_surface().map(|x| x == surface).unwrap_or(false))
    }

    /// Returns the layer matching a given surface, if any
    pub fn layer_for_surface(&self, surface: &WlSurface) -> Option<LayerSurface> {
        if !surface.as_ref().is_alive() {
            return None;
        }
        self.outputs.iter().find_map(|o| {
            let map = layer_map_for_output(o);
            map.layer_for_surface(surface).cloned()
        })
    }

    /// Returns the geometry of a [`Window`] including its relative position inside the Space.
    pub fn window_geometry(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.windows.contains(w) {
            return None;
        }

        Some(window_geo(w, &self.id))
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
    /// [`Output`] to update its scale or location.
    ///
    /// The scale is the what is rendered for the given output
    /// and may be fractional. It is independent from the integer scale
    /// reported to clients by the output.
    ///
    /// *Note:* Remapping an output does reset it's damage memory.
    pub fn map_output<P: Into<Point<i32, Logical>>>(&mut self, output: &Output, scale: f64, location: P) {
        let mut state = output_state(self.id, output);
        *state = OutputState {
            location: location.into(),
            render_scale: scale,
            // keep surfaces, we still need to inform them of leaving,
            // if they don't overlap anymore during refresh.
            surfaces: state.surfaces.drain(..).collect::<Vec<_>>(),
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
    /// given is current mode and render scale.
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
                    .to_logical(state.render_scale)
                    .to_i32_round(),
            )
        })
    }

    /// Returns the reder scale of a mapped output.
    ///
    /// If the output was not previously mapped to the `Space`
    /// this function returns `None`.
    pub fn output_scale(&self, o: &Output) -> Option<f64> {
        if !self.outputs.contains(o) {
            return None;
        }

        let state = output_state(self.id, o);
        Some(state.render_scale)
    }

    /// Returns all [`Output`]s a [`Window`] overlaps with.
    pub fn outputs_for_window(&self, w: &Window) -> Vec<Output> {
        if !self.windows.contains(w) {
            return Vec::new();
        }

        let w_geo = window_rect(w, &self.id);
        let mut outputs = self
            .outputs
            .iter()
            .cloned()
            .filter(|o| {
                let o_geo = self.output_geometry(o).unwrap();
                w_geo.overlaps(o_geo)
            })
            .collect::<Vec<Output>>();
        outputs.sort_by(|o1, o2| {
            let overlap = |rect1: Rectangle<i32, Logical>, rect2: Rectangle<i32, Logical>| -> i32 {
                // x overlap
                std::cmp::max(0, std::cmp::min(rect1.loc.x + rect1.size.w, rect2.loc.x + rect2.size.w) - std::cmp::max(rect1.loc.x, rect2.loc.x))
                // y overlap
                * std::cmp::max(0, std::cmp::min(rect1.loc.y + rect1.size.h, rect2.loc.y + rect2.size.h) - std::cmp::max(rect1.loc.y, rect2.loc.y))
            };
            let o1_area = overlap(self.output_geometry(o1).unwrap(), w_geo);
            let o2_area = overlap(self.output_geometry(o2).unwrap(), w_geo);
            o1_area.cmp(&o2_area)
        });
        outputs
    }

    /// Refresh some internal values and update client state,
    /// meaning this will handle output enter and leave events
    /// for mapped outputs and windows based on their position.
    ///
    /// Needs to be called periodically, at best before every
    /// wayland socket flush.
    pub fn refresh(&mut self) {
        self.windows.retain(|w| w.toplevel().alive());

        for output in &mut self.outputs {
            output_state(self.id, output)
                .surfaces
                .retain(|s| s.as_ref().is_alive());
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
                    if let Some(surface) = kind.get_surface() {
                        with_surface_tree_downward(
                            surface,
                            (),
                            |_, _, _| TraversalAction::DoChildren(()),
                            |wl_surface, _, _| {
                                if output_state.surfaces.contains(wl_surface) {
                                    slog::trace!(
                                        self.logger,
                                        "surface ({:?}) leaving output {:?}",
                                        wl_surface,
                                        output.name()
                                    );
                                    output.leave(wl_surface);
                                    output_state.surfaces.retain(|s| s != wl_surface);
                                }
                            },
                            |_, _, _| true,
                        )
                    }
                    continue;
                }

                if let Some(surface) = kind.get_surface() {
                    with_surface_tree_downward(
                        surface,
                        window_loc(window, &self.id),
                        |_, states, location| {
                            let mut location = *location;
                            let data = states.data_map.get::<RefCell<SurfaceState>>();

                            if data.is_some() {
                                if states.role == Some("subsurface") {
                                    let current = states.cached_state.current::<SubsurfaceCachedState>();
                                    location += current.location;
                                }

                                TraversalAction::DoChildren(location)
                            } else {
                                // If the parent surface is unmapped, then the child surfaces are hidden as
                                // well, no need to consider them here.
                                TraversalAction::SkipChildren
                            }
                        },
                        |wl_surface, states, &loc| {
                            let data = states.data_map.get::<RefCell<SurfaceState>>();

                            if let Some(size) = data.and_then(|d| d.borrow().size()) {
                                let surface_rectangle = Rectangle { loc, size };

                                if output_geometry.overlaps(surface_rectangle) {
                                    // We found a matching output, check if we already sent enter
                                    if !output_state.surfaces.contains(wl_surface) {
                                        slog::trace!(
                                            self.logger,
                                            "surface ({:?}) entering output {:?}",
                                            wl_surface,
                                            output.name()
                                        );
                                        output.enter(wl_surface);
                                        output_state.surfaces.push(wl_surface.clone());
                                    }
                                } else {
                                    // Surface does not match output, if we sent enter earlier
                                    // we should now send leave
                                    if output_state.surfaces.contains(wl_surface) {
                                        slog::trace!(
                                            self.logger,
                                            "surface ({:?}) leaving output {:?}",
                                            wl_surface,
                                            output.name()
                                        );
                                        output.leave(wl_surface);
                                        output_state.surfaces.retain(|s| s != wl_surface);
                                    }
                                }
                            } else {
                                // Maybe the the surface got unmapped, send leave on output
                                if output_state.surfaces.contains(wl_surface) {
                                    slog::trace!(
                                        self.logger,
                                        "surface ({:?}) leaving output {:?}",
                                        wl_surface,
                                        output.name()
                                    );
                                    output.leave(wl_surface);
                                    output_state.surfaces.retain(|s| s != wl_surface);
                                }
                            }
                        },
                        |_, _, _| true,
                    )
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
        if let Some(window) = self.windows().find(|w| w.toplevel().get_surface() == Some(&root)) {
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
    /// Returns a list of updated regions (or `None` if that list would be empty) in case of success.
    pub fn render_output<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        age: usize,
        clear_color: [f32; 4],
        custom_elements: &[DynamicRenderElements<R>],
    ) -> Result<Option<Vec<Rectangle<i32, Logical>>>, RenderError<R>>
    where
        R: Renderer + ImportAll + 'static,
        R::TextureId: 'static,
        R::Error: 'static,
        R::Frame: 'static,
    {
        if !self.outputs.contains(output) {
            return Err(RenderError::UnmappedOutput);
        }

        type SpaceElem<R> =
            dyn SpaceElement<R, <R as Renderer>::Frame, <R as Renderer>::Error, <R as Renderer>::TextureId>;

        let mut state = output_state(self.id, output);
        let output_size = output
            .current_mode()
            .ok_or(RenderError::OutputNoMode)?
            .size
            .to_f64()
            .to_logical(state.render_scale)
            .to_i32_round();
        let output_geo = Rectangle::from_loc_and_size(state.location, output_size);
        let layer_map = layer_map_for_output(output);

        let window_popups = self
            .windows
            .iter()
            .flat_map(|w| w.popup_elements::<R>(self.id))
            .collect::<Vec<_>>();
        let layer_popups = layer_map
            .layers()
            .flat_map(|l| l.popup_elements::<R>(self.id))
            .collect::<Vec<_>>();

        // This will hold all the damage we need for this rendering step
        let mut damage = Vec::<Rectangle<i32, Logical>>::new();
        // First add damage for windows gone
        for old_toplevel in state
            .last_state
            .iter()
            .filter_map(|(id, geo)| {
                if !self
                    .windows
                    .iter()
                    .map(|w| w as &SpaceElem<R>)
                    .chain(window_popups.iter().map(|p| p as &SpaceElem<R>))
                    .chain(layer_map.layers().map(|l| l as &SpaceElem<R>))
                    .chain(layer_popups.iter().map(|p| p as &SpaceElem<R>))
                    .chain(custom_elements.iter().map(|c| c as &SpaceElem<R>))
                    .any(|e| ToplevelId::from(e) == *id)
                {
                    Some(*geo)
                } else {
                    None
                }
            })
            .collect::<Vec<Rectangle<i32, Logical>>>()
        {
            slog::trace!(self.logger, "Removing toplevel at: {:?}", old_toplevel);
            damage.push(old_toplevel);
        }

        // lets iterate front to back and figure out, what new windows or unmoved windows we have
        for element in self
            .windows
            .iter()
            .map(|w| w as &SpaceElem<R>)
            .chain(window_popups.iter().map(|p| p as &SpaceElem<R>))
            .chain(layer_map.layers().map(|l| l as &SpaceElem<R>))
            .chain(layer_popups.iter().map(|p| p as &SpaceElem<R>))
            .chain(custom_elements.iter().map(|c| c as &SpaceElem<R>))
        {
            let geo = element.geometry(self.id);
            let old_geo = state.last_state.get(&ToplevelId::from(element)).cloned();

            // window was moved or resized
            if old_geo.map(|old_geo| old_geo != geo).unwrap_or(false) {
                // Add damage for the old position of the window
                damage.push(old_geo.unwrap());
                damage.push(geo);
            } else {
                // window stayed at its place
                let loc = element.location(self.id);
                damage.extend(element.accumulated_damage(Some((self, output))).into_iter().map(
                    |mut rect| {
                        rect.loc += loc;
                        rect
                    },
                ));
            }
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
        damage.retain(|rect| rect.size.h > 0 && rect.size.w > 0);
        // merge overlapping rectangles
        damage = damage.into_iter().fold(Vec::new(), |new_damage, mut rect| {
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

        let output_transform: Transform = output.current_transform().into();
        let res = renderer.render(
            output_transform
                .transform_size(output_size)
                .to_f64()
                .to_physical(state.render_scale)
                .to_i32_round(),
            output_transform,
            |renderer, frame| {
                // First clear all damaged regions
                slog::trace!(self.logger, "Clearing at {:#?}", damage);
                frame.clear(
                    clear_color,
                    &damage
                        .iter()
                        .map(|geo| geo.to_f64().to_physical(state.render_scale).to_i32_round())
                        .collect::<Vec<_>>(),
                )?;

                // Then re-draw all windows & layers overlapping with a damage rect.

                for element in layer_map
                    .layers_on(WlrLayer::Background)
                    .chain(layer_map.layers_on(WlrLayer::Bottom))
                    .map(|l| l as &SpaceElem<R>)
                    .chain(self.windows.iter().map(|w| w as &SpaceElem<R>))
                    .chain(
                        layer_map
                            .layers_on(WlrLayer::Top)
                            .chain(layer_map.layers_on(WlrLayer::Overlay))
                            .map(|l| l as &SpaceElem<R>),
                    )
                    .chain(custom_elements.iter().map(|c| c as &SpaceElem<R>))
                {
                    let geo = element.geometry(self.id);
                    if damage.iter().any(|d| d.overlaps(geo)) {
                        let loc = element.location(self.id) - output_geo.loc;
                        let damage = damage
                            .iter()
                            .flat_map(|d| d.intersection(geo))
                            .map(|geo| Rectangle::from_loc_and_size(geo.loc - loc, geo.size))
                            .collect::<Vec<_>>();
                        slog::trace!(
                            self.logger,
                            "Rendering toplevel at {:?} with damage {:#?}",
                            geo,
                            damage
                        );
                        element.draw(
                            self.id,
                            renderer,
                            frame,
                            state.render_scale,
                            loc,
                            &damage,
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
            state.last_state = IndexMap::new();
            return Err(RenderError::Rendering(err));
        }

        // If rendering was successful capture the state and add the damage
        state.last_state = self
            .windows
            .iter()
            .map(|w| w as &SpaceElem<R>)
            .chain(window_popups.iter().map(|p| p as &SpaceElem<R>))
            .chain(layer_map.layers().map(|l| l as &SpaceElem<R>))
            .chain(layer_popups.iter().map(|p| p as &SpaceElem<R>))
            .chain(custom_elements.iter().map(|c| c as &SpaceElem<R>))
            .map(|elem| {
                let geo = elem.geometry(self.id);
                (ToplevelId::from(elem), geo)
            })
            .collect();
        state.old_damage.push_front(new_damage.clone());

        Ok(Some(new_damage))
    }

    /// Sends the frame callback to mapped [`Window`]s and [`LayerSurface`]s.
    ///
    /// If `all` is set this will be send to `all` mapped surfaces.
    /// Otherwise only windows and layers previously drawn during the
    /// previous frame will be send frame events.
    pub fn send_frames(&self, all: bool, time: u32) {
        for window in self.windows.iter().filter(|w| {
            all || {
                let mut state = window_state(self.id, w);
                std::mem::replace(&mut state.drawn, false)
            }
        }) {
            window.send_frame(time);
        }

        for output in self.outputs.iter() {
            let map = layer_map_for_output(output);
            for layer in map.layers().filter(|l| {
                all || {
                    let mut state = layer_state(self.id, l);
                    std::mem::replace(&mut state.drawn, false)
                }
            }) {
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
