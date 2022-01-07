//! Utility module for helpers around drawing [`WlSurface`]s with [`Renderer`]s.

use crate::{
    backend::renderer::{buffer_dimensions, Frame, ImportAll, Renderer, Texture},
    utils::{Logical, Physical, Point, Rectangle, Size},
    wayland::compositor::{
        is_sync_subsurface, with_surface_tree_upward, BufferAssignment, Damage, SubsurfaceCachedState,
        SurfaceAttributes, TraversalAction,
    },
};
use std::cell::RefCell;
// #[cfg(feature = "desktop")]
// use std::collections::HashSet;
use wayland_server::{
    protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
    DisplayHandle,
};

#[derive(Default)]
pub(crate) struct SurfaceState {
    pub(crate) buffer_dimensions: Option<Size<i32, Physical>>,
    pub(crate) buffer_scale: i32,
    pub(crate) buffer: Option<WlBuffer>,
    pub(crate) texture: Option<Box<dyn std::any::Any + 'static>>,
    // #[cfg(feature = "desktop")]
    // pub(crate) damage_seen: HashSet<crate::desktop::space::SpaceOutputHash>,
}

impl SurfaceState {
    pub fn update_buffer<D>(&mut self, cx: &mut DisplayHandle<'_, D>, attrs: &mut SurfaceAttributes) {
        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer { buffer, .. }) => {
                // new contents
                self.buffer_dimensions = buffer_dimensions(&buffer);
                self.buffer_scale = attrs.buffer_scale;
                if let Some(old_buffer) = std::mem::replace(&mut self.buffer, Some(buffer)) {
                    if &old_buffer != self.buffer.as_ref().unwrap() {
                        old_buffer.release(cx);
                    }
                }
                self.texture = None;
                // TODO: After dekstop is back
                // #[cfg(feature = "desktop")]
                // self.damage_seen.clear();
            }
            Some(BufferAssignment::Removed) => {
                // remove the contents
                self.buffer_dimensions = None;
                if let Some(buffer) = self.buffer.take() {
                    buffer.release(cx);
                };
                self.texture = None;
                // #[cfg(feature = "desktop")]
                // self.damage_seen.clear();
            }
            None => {}
        }
    }
}

/// Handler to let smithay take over buffer management.
///
/// Needs to be called first on the commit-callback of
/// [`crate::wayland::compositor::compositor_init`].
///
/// Consumes the buffer of [`SurfaceAttributes`], the buffer will
/// not be accessible anymore, but [`draw_surface_tree`] and other
/// `draw_*` helpers of the [desktop module](`crate::desktop`) will
/// become usable for surfaces handled this way.
pub fn on_commit_buffer_handler<D: 'static>(cx: &mut DisplayHandle<'_, D>, surface: &WlSurface) {
    if !is_sync_subsurface(cx, surface) {
        with_surface_tree_upward::<D, _, _, _, _>(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_surf, states, _| {
                states
                    .data_map
                    .insert_if_missing(|| RefCell::new(SurfaceState::default()));
                let mut data = states
                    .data_map
                    .get::<RefCell<SurfaceState>>()
                    .unwrap()
                    .borrow_mut();
                data.update_buffer(cx, &mut *states.cached_state.current::<SurfaceAttributes>());
            },
            |_, _, _| true,
        );
    }
}

/// Draws a surface and its subsurfaces using a given [`Renderer`] and [`Frame`].
///
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `location` is the position the surface should be drawn at.
/// - `damage` is the set of regions of the surface that should be drawn.
///
/// Note: This element will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
pub fn draw_surface_tree<D, R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    surface: &WlSurface,
    scale: f64,
    location: Point<i32, Logical>,
    damage: &[Rectangle<i32, Logical>],
    log: &slog::Logger,
) -> Result<(), R::Error>
where
    D: 'static,
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
{
    let mut result = Ok(());
    let damage = damage
        .iter()
        .map(|geo| geo.to_f64().to_physical(scale).to_i32_up())
        .collect::<Vec<_>>();
    with_surface_tree_upward::<D, _, _, _, _>(
        surface,
        location,
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                let mut data = data.borrow_mut();
                let attributes = states.cached_state.current::<SurfaceAttributes>();
                // Import a new buffer if necessary
                if data.texture.is_none() {
                    if let Some(buffer) = data.buffer.as_ref() {
                        let damage = attributes
                            .damage
                            .iter()
                            .map(|dmg| match dmg {
                                Damage::Buffer(rect) => *rect,
                                // TODO also apply transformations
                                Damage::Surface(rect) => rect.to_buffer(attributes.buffer_scale),
                            })
                            .collect::<Vec<_>>();

                        match renderer.import_buffer(buffer, Some(states), &damage) {
                            Some(Ok(m)) => {
                                data.texture = Some(Box::new(m));
                            }
                            Some(Err(err)) => {
                                slog::warn!(log, "Error loading buffer: {}", err);
                            }
                            None => {
                                slog::error!(log, "Unknown buffer format for: {:?}", buffer);
                            }
                        }
                    }
                }
                // Now, should we be drawn ?
                if data.texture.is_some() {
                    // if yes, also process the children
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        location += current.location;
                    }
                    TraversalAction::DoChildren(location)
                } else {
                    // we are not displayed, so our children are neither
                    TraversalAction::SkipChildren
                }
            } else {
                // we are not displayed, so our children are neither
                TraversalAction::SkipChildren
            }
        },
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                let mut data = data.borrow_mut();
                let buffer_scale = data.buffer_scale;
                let buffer_dimensions = data.buffer_dimensions;
                let attributes = states.cached_state.current::<SurfaceAttributes>();
                if let Some(texture) = data.texture.as_mut().and_then(|x| x.downcast_mut::<T>()) {
                    // we need to re-extract the subsurface offset, as the previous closure
                    // only passes it to our children
                    let mut surface_offset = (0, 0).into();
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        surface_offset = current.location;
                        location += current.location;
                    }

                    let rect = Rectangle::<i32, Physical>::from_loc_and_size(
                        surface_offset.to_f64().to_physical(scale).to_i32_round(),
                        buffer_dimensions
                            .unwrap_or_default()
                            .to_logical(buffer_scale)
                            .to_f64()
                            .to_physical(scale)
                            .to_i32_round(),
                    );
                    let new_damage = damage
                        .iter()
                        .cloned()
                        .flat_map(|geo| geo.intersection(rect))
                        .map(|mut geo| {
                            geo.loc -= rect.loc;
                            geo
                        })
                        .collect::<Vec<_>>();

                    // TODO: Take wp_viewporter into account
                    if let Err(err) = frame.render_texture_at(
                        texture,
                        location.to_f64().to_physical(scale).to_i32_round(),
                        buffer_scale,
                        scale,
                        attributes.buffer_transform.into(),
                        &new_damage,
                        1.0,
                    ) {
                        result = Err(err);
                    }
                }
            }
        },
        |_, _, _| true,
    );

    result
}
