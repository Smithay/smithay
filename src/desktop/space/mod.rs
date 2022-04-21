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
    utils::{Logical, Point, Rectangle, Transform},
    wayland::{
        compositor::{get_parent, is_sync_subsurface},
        output::Output,
    },
};
use indexmap::{IndexMap, IndexSet};
use std::{collections::VecDeque, fmt};
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};

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
            let loc = window.elem_location(self.id);
            let mut geo = window.bbox();
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
            let loc = w.elem_location(self.id);
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

    /// Returns the window matching a given surface, if any
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<&Window> {
        self.windows.iter().find(|w| w.toplevel().wl_surface() == surface)
    }

    /// Returns the layer matching a given surface, if any
    pub fn layer_for_surface(&self, surface: &WlSurface) -> Option<LayerSurface> {
        self.outputs.iter().find_map(|o| {
            let map = layer_map_for_output(o);
            map.layer_for_surface(surface).cloned()
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
    pub fn refresh(&mut self, dh: &mut DisplayHandle<'_>) {
        self.windows.retain(|w| w.toplevel().alive());

        // TODO(desktop-0.30)
        // for output in &mut self.outputs {
        //     output_state(self.id, output)
        //         .surfaces
        //         .retain(|s| s.as_ref().is_alive());
        // }

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
        dh: &mut DisplayHandle<'_>,
        renderer: &mut R,
        output: &Output,
        age: usize,
        clear_color: [f32; 4],
        custom_elements: &[E],
    ) -> Result<Option<Vec<Rectangle<i32, Logical>>>, RenderError<R>>
    where
        R: Renderer + ImportAll,
        R::TextureId: 'static,
        E: RenderElement<R>,
    {
        if !self.outputs.contains(output) {
            return Err(RenderError::UnmappedOutput);
        }

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

        render_elements.sort_by_key(|e| e.z_index());

        // This will hold all the damage we need for this rendering step
        let mut damage = Vec::<Rectangle<i32, Logical>>::new();
        // First add damage for windows gone

        for old_toplevel in state
            .last_state
            .iter()
            .filter_map(|(id, geo)| {
                if !render_elements.iter().any(|e| ToplevelId::from(e) == *id) {
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
        for element in &render_elements {
            let geo = element.geometry(self.id);
            let old_geo = state.last_state.get(&ToplevelId::from(element)).cloned();

            // window was moved, resized or just appeared
            if old_geo.map(|old_geo| old_geo != geo).unwrap_or(true) {
                // Add damage for the old position of the window
                if let Some(old_geo) = old_geo {
                    damage.push(old_geo);
                }
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
                        // Map from global space to output space
                        .map(|geo| Rectangle::from_loc_and_size(geo.loc - output_geo.loc, geo.size))
                        // Map from logical to physical
                        .map(|geo| geo.to_f64().to_physical(state.render_scale))
                        .collect::<Vec<_>>(),
                )?;
                // Then re-draw all windows & layers overlapping with a damage rect.

                for element in &render_elements {
                    let geo = element.geometry(self.id);
                    if damage.iter().any(|d| d.overlaps(geo)) {
                        let loc = element.location(self.id);
                        let damage = damage
                            .iter()
                            .flat_map(|d| d.intersection(geo))
                            // Map from output space to surface-relative coordinates
                            .map(|geo| Rectangle::from_loc_and_size(geo.loc - loc, geo.size))
                            .collect::<Vec<_>>();
                        slog::trace!(
                            self.logger,
                            "Rendering toplevel at {:?} with damage {:#?}",
                            Rectangle::from_loc_and_size(geo.loc - output_geo.loc, geo.size),
                            damage
                        );
                        element.draw(
                            dh,
                            self.id,
                            renderer,
                            frame,
                            state.render_scale,
                            loc - output_geo.loc,
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
        state.last_state = render_elements
            .iter()
            .map(|elem| {
                let geo = elem.geometry(self.id);
                (ToplevelId::from(elem), geo)
            })
            .collect();
        state.old_damage.push_front(new_damage.clone());

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
    pub fn send_frames(&self, dh: &mut DisplayHandle<'_>, time: u32) {
        for window in self.windows.iter() {
            window.send_frame(dh, time);
        }

        for output in self.outputs.iter() {
            let map = layer_map_for_output(output);
            for layer in map.layers() {
                layer.send_frame(dh, time);
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

        fn geometry(&self) -> $crate::utils::Rectangle<i32, $crate::utils::Logical> {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; geometry; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn accumulated_damage(&self, for_values: std::option::Option<$crate::desktop::space::SpaceOutputTuple<'_, '_>>) -> Vec<$crate::utils::Rectangle<i32, $crate::utils::Logical>> {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; accumulated_damage; x, for_values)
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
            dh: &mut $crate::reexports::wayland_server::DisplayHandle<'_>,
            renderer: &mut $renderer,
            frame: &mut <$renderer as $crate::backend::renderer::Renderer>::Frame,
            scale: f64,
            location: $crate::utils::Point<i32, $crate::utils::Logical>,
            damage: &[$crate::utils::Rectangle<i32, $crate::utils::Logical>],
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
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, dh, renderer, frame, scale, location, damage, log)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@draw $renderer:ty; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn draw(
            &self,
            dh: &mut $crate::reexports::wayland_server::DisplayHandle<'_>,
            renderer: &mut $renderer,
            frame: &mut <$renderer as $crate::backend::renderer::Renderer>::Frame,
            scale: f64,
            location: $crate::utils::Point<i32, $crate::utils::Logical>,
            damage: &[$crate::utils::Rectangle<i32, $crate::utils::Logical>],
            log: &slog::Logger,
        ) -> Result<(), <$renderer as $crate::backend::renderer::Renderer>::Error>
        {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::custom_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, dh, renderer, frame, scale, location, damage, log)
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

/// Macro to collate multiple [`smithay::desktop::RenderElement`]-implementations
/// into one type to be used with [`Space::render_output`].
/// ## Example
///
/// ```no_run
/// use smithay::{
///     backend::renderer::{Texture, Renderer, ImportAll},
///     desktop::space::{SurfaceTree, Space, SpaceOutputTuple, RenderElement},
///     utils::{Point, Size, Rectangle, Transform, Logical},
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
/// #   fn clear(&mut self, color: [f32; 4], at: &[Rectangle<f64, Physical>]) -> Result<(), Self::Error> { Ok(()) }
/// #   #[allow(clippy::too_many_arguments)]
/// #   fn render_texture_at(
/// #       &mut self,
/// #       texture: &Self::TextureId,
/// #       pos: Point<f64, Physical>,
/// #       texture_scale: i32,
/// #       output_scale: f64,
/// #       src_transform: Transform,
/// #       damage: &[Rectangle<f64, Physical>],
/// #       alpha: f32,
/// #   ) -> Result<(), Self::Error> {
/// #       Ok(())
/// #   }
/// #   fn render_texture_from_to(
/// #       &mut self,
/// #       texture: &Self::TextureId,
/// #       src: Rectangle<i32, Buffer>,
/// #       dst: Rectangle<f64, Physical>,
/// #       damage: &[Rectangle<f64, Physical>],
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
///#    fn geometry(&self) -> Rectangle<i32, Logical> {
///#        Rectangle::from_loc_and_size(self.position, self.size)
///#    }
///#
///#    fn accumulated_damage(&self, _: Option<SpaceOutputTuple<'_, '_>>) -> Vec<Rectangle<i32, Logical>> {
///#        vec![]
///#    }
///#
///#    fn draw(
///#        &self,
///#        _renderer: &mut R,
///#        frame: &mut <R as Renderer>::Frame,
///#        scale: f64,
///#        location: Point<i32, Logical>,
///#        damage: &[Rectangle<i32, Logical>],
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
