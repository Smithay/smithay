//! Utility module for helpers around drawing [`WlSurface`]s with [`Renderer`]s.

use crate::{
    backend::renderer::{buffer_dimensions, buffer_has_alpha, Frame, ImportAll, Renderer},
    utils::{Buffer as BufferCoord, Coordinate, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        compositor::{
            self, add_destruction_hook, is_sync_subsurface, with_surface_tree_downward,
            with_surface_tree_upward, BufferAssignment, Damage, RectangleKind, SubsurfaceCachedState,
            SurfaceAttributes, SurfaceData, TraversalAction,
        },
        viewporter,
    },
};
use slog::trace;
use std::collections::VecDeque;
use std::{
    any::TypeId,
    cell::RefCell,
    collections::{hash_map::Entry, HashMap},
};
use wayland_server::protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface};

/// Type stored in WlSurface states data_map
///
/// ```rs
/// compositor::with_states(surface, |states| {
///     let data = states.data_map.get::<RendererSurfaceStateUserData>();
/// });
/// ```
pub type RendererSurfaceStateUserData = RefCell<RendererSurfaceState>;

/// Surface state for rendering related data
#[derive(Default, Debug)]
pub struct RendererSurfaceState {
    pub(crate) commit_count: usize,
    pub(crate) buffer_dimensions: Option<Size<i32, BufferCoord>>,
    pub(crate) buffer_scale: i32,
    pub(crate) buffer_transform: Transform,
    pub(crate) buffer_delta: Option<Point<i32, Logical>>,
    pub(crate) buffer_has_alpha: Option<bool>,
    pub(crate) buffer: Option<WlBuffer>,
    pub(crate) damage: VecDeque<Vec<Rectangle<i32, BufferCoord>>>,
    pub(crate) renderer_seen: HashMap<(TypeId, usize), usize>,
    pub(crate) textures: HashMap<(TypeId, usize), Box<dyn std::any::Any>>,
    pub(crate) surface_view: Option<SurfaceView>,
    pub(crate) opaque_regions: Vec<Rectangle<i32, Logical>>,
    #[cfg(feature = "desktop")]
    pub(crate) space_seen: HashMap<crate::desktop::space::SpaceOutputHash, usize>,

    accumulated_buffer_delta: Point<i32, Logical>,
}

const MAX_DAMAGE: usize = 4;

impl RendererSurfaceState {
    pub(crate) fn update_buffer(&mut self, states: &SurfaceData) {
        let mut attrs = states.cached_state.current::<SurfaceAttributes>();
        self.buffer_delta = attrs.buffer_delta.take();

        if let Some(delta) = self.buffer_delta {
            self.accumulated_buffer_delta += delta;
        }

        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer(buffer)) => {
                // new contents
                self.buffer_dimensions = buffer_dimensions(&buffer);
                if self.buffer_dimensions.is_none() {
                    // This results in us rendering nothing (can happen e.g. for failed egl-buffer-calls),
                    // but it is better than crashing the compositor for a bad buffer
                    return;
                }
                self.buffer_has_alpha = buffer_has_alpha(&buffer);

                #[cfg(feature = "desktop")]
                if self.buffer_scale != attrs.buffer_scale
                    || self.buffer_transform != attrs.buffer_transform.into()
                {
                    self.reset_space_damage();
                }
                self.buffer_scale = attrs.buffer_scale;
                self.buffer_transform = attrs.buffer_transform.into();

                if let Some(old_buffer) = std::mem::replace(&mut self.buffer, Some(buffer)) {
                    if &old_buffer != self.buffer.as_ref().unwrap() {
                        old_buffer.release();
                    }
                }
                self.textures.clear();
                self.commit_count = self.commit_count.wrapping_add(1);

                let surface_size = self
                    .buffer_dimensions
                    .unwrap()
                    .to_logical(self.buffer_scale, self.buffer_transform);
                let surface_view = SurfaceView::from_states(states, surface_size);
                self.surface_view = Some(surface_view);

                let mut buffer_damage = attrs
                    .damage
                    .drain(..)
                    .flat_map(|dmg| {
                        match dmg {
                            Damage::Buffer(rect) => rect,
                            Damage::Surface(rect) => surface_view.rect_to_local(rect).to_i32_up().to_buffer(
                                self.buffer_scale,
                                self.buffer_transform,
                                &surface_size,
                            ),
                        }
                        .intersection(Rectangle::from_loc_and_size(
                            (0, 0),
                            self.buffer_dimensions.unwrap(),
                        ))
                    })
                    .collect::<Vec<Rectangle<i32, BufferCoord>>>();
                buffer_damage.dedup();
                self.damage.push_front(buffer_damage);
                self.damage.truncate(MAX_DAMAGE);

                self.opaque_regions.clear();
                if !self.buffer_has_alpha.unwrap_or(true) {
                    self.opaque_regions.push(Rectangle::from_loc_and_size(
                        (0, 0),
                        self.surface_view.unwrap().dst,
                    ))
                } else if let Some(region_attributes) = &attrs.opaque_region {
                    let opaque_regions = region_attributes
                        .rects
                        .iter()
                        .map(|(kind, rect)| {
                            let dest_size = self.surface_view.unwrap().dst;

                            let rect_constrained_loc = rect
                                .loc
                                .constrain(Rectangle::from_extemities((0, 0), dest_size.to_point()));
                            let rect_clamped_size = rect
                                .size
                                .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                            let rect = Rectangle::from_loc_and_size(rect_constrained_loc, rect_clamped_size);

                            (kind, rect)
                        })
                        .fold(
                            std::mem::take(&mut self.opaque_regions),
                            |mut new_regions, (kind, rect)| {
                                match kind {
                                    RectangleKind::Add => {
                                        let added_regions = new_regions
                                            .iter()
                                            .filter(|region| region.overlaps(rect))
                                            .fold(vec![rect], |new_regions, existing_region| {
                                                new_regions
                                                    .into_iter()
                                                    .flat_map(|region| region.subtract_rect(*existing_region))
                                                    .collect::<Vec<_>>()
                                            });
                                        new_regions.extend(added_regions);
                                    }
                                    RectangleKind::Subtract => {
                                        new_regions = new_regions
                                            .into_iter()
                                            .flat_map(|r| r.subtract_rect(rect))
                                            .collect::<Vec<_>>();
                                    }
                                }

                                new_regions
                            },
                        );

                    self.opaque_regions = opaque_regions;
                }
            }
            Some(BufferAssignment::Removed) => {
                // remove the contents
                self.buffer_dimensions = None;
                if let Some(buffer) = self.buffer.take() {
                    buffer.release();
                };
                self.textures.clear();
                self.commit_count = self.commit_count.wrapping_add(1);
                self.damage.clear();
                self.surface_view = None;
                self.buffer_has_alpha = None;
                self.opaque_regions.clear();
            }
            None => {}
        }
    }

    /// Get the current commit position of this surface
    ///
    /// The position should be saved after calling [`damage_since`] and
    /// provided as the commit in the next call.
    pub fn current_commit(&self) -> usize {
        self.commit_count
    }

    /// Gets the damage since the last commit
    ///
    /// If either the commit is `None` or the commit is too old
    /// the whole buffer will be returned as damage.
    pub fn damage_since(&self, commit: Option<usize>) -> Vec<Rectangle<i32, BufferCoord>> {
        // on overflow the wrapping_sub should end up
        let recent_enough = commit
            // if commit > commit_count we have overflown, in that case the following map might result
            // in a false-positive, if commit is still very large. So we force false in those cases.
            // That will result in a potentially sub-optimal full damage every usize::MAX frames,
            // which is acceptable.
            .filter(|commit| *commit <= self.commit_count)
            .map(|commit| self.commit_count.wrapping_sub(self.damage.len()) <= commit)
            .unwrap_or(false);
        if recent_enough {
            self.damage
                .iter()
                .take(self.commit_count.wrapping_sub(commit.unwrap()))
                .fold(Vec::new(), |mut acc, elem| {
                    acc.extend(elem);
                    acc
                })
        } else {
            self.buffer_dimensions
                .as_ref()
                .map(|size| vec![Rectangle::from_loc_and_size((0, 0), *size)])
                .unwrap_or_else(Vec::new)
        }
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn reset_space_damage(&mut self) {
        self.space_seen.clear();
    }

    /// Returns the logical size of the current attached buffer
    pub fn buffer_size(&self) -> Option<Size<i32, Logical>> {
        self.buffer_dimensions
            .as_ref()
            .map(|dim| dim.to_logical(self.buffer_scale, self.buffer_transform))
    }

    /// Returns the logical size of the surface.
    ///
    /// Note: The surface size may not be equal to the buffer size in case
    /// a viewport has been attached to the surface.
    pub fn surface_size(&self) -> Option<Size<i32, Logical>> {
        self.surface_view.map(|view| view.dst)
    }

    /// Get the attached buffer.
    /// Can be used to check if surface is mapped
    pub fn wl_buffer(&self) -> Option<&WlBuffer> {
        self.buffer.as_ref()
    }

    /// Gets a reference to the texture for the specified renderer
    pub fn texture<R>(&self, renderer: &R) -> Option<&R::TextureId>
    where
        R: Renderer,
        <R as Renderer>::TextureId: 'static,
    {
        let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
        self.textures.get(&texture_id).and_then(|e| e.downcast_ref())
    }

    /// Location of the buffer relative to the previous call of take_accumulated_buffer_delta
    ///
    /// In other words, the x and y, combined with the new surface size define in which directions
    /// the surface's size changed since last call to this method.
    ///
    /// Once delta is taken this method returns `None` to avoid processing it several times.
    pub fn take_accumulated_buffer_delta(&mut self) -> Point<i32, Logical> {
        std::mem::take(&mut self.accumulated_buffer_delta)
    }

    /// Gets the opaque regions of this surface
    pub fn opaque_regions(&self) -> Option<&[Rectangle<i32, Logical>]> {
        // If the surface is unmapped there can be no opaque regions
        self.surface_size()?;

        // To make it easier for upstream, but to no re-allocate a
        // new vec if the opaque regions change for return None
        // on empty regions
        if self.opaque_regions.is_empty() {
            return None;
        }

        Some(&self.opaque_regions[..])
    }

    /// Gets the [`SurfaceView`] of this surface
    pub fn view(&self) -> Option<SurfaceView> {
        self.surface_view
    }
}

/// Handler to let smithay take over buffer management.
///
/// Needs to be called first on the commit-callback of
/// [`crate::wayland::compositor::CompositorHandler::commit`].
///
/// Consumes the buffer of [`SurfaceAttributes`], the buffer will
/// not be accessible anymore, but [`draw_surface_tree`] and other
/// `draw_*` helpers of the [desktop module](`crate::desktop`) will
/// become usable for surfaces handled this way.
pub fn on_commit_buffer_handler(surface: &WlSurface) {
    if !is_sync_subsurface(surface) {
        let mut new_surfaces = Vec::new();
        with_surface_tree_upward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |surf, states, _| {
                if states
                    .data_map
                    .insert_if_missing(|| RefCell::new(RendererSurfaceState::default()))
                {
                    new_surfaces.push(surf.clone());
                }
                let mut data = states
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .unwrap()
                    .borrow_mut();
                data.update_buffer(states);
            },
            |_, _, _| true,
        );
        for surf in &new_surfaces {
            add_destruction_hook(surf, |data| {
                if let Some(buffer) = data
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .and_then(|s| s.borrow_mut().buffer.take())
                {
                    buffer.release();
                }
            });
        }
    }
}

/// Defines a view into the surface
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub struct SurfaceView {
    /// The logical source used for cropping
    pub src: Rectangle<f64, Logical>,
    /// The logical destination size used for scaling
    pub dst: Size<i32, Logical>,
    /// The logical offset for a sub-surface
    pub offset: Point<i32, Logical>,
}

impl SurfaceView {
    fn from_states(states: &SurfaceData, surface_size: Size<i32, Logical>) -> SurfaceView {
        viewporter::ensure_viewport_valid(states, surface_size);
        let viewport = states.cached_state.current::<viewporter::ViewportCachedState>();
        let src = viewport
            .src
            .unwrap_or_else(|| Rectangle::from_loc_and_size((0.0, 0.0), surface_size.to_f64()));
        let dst = viewport.size().unwrap_or(surface_size);
        let offset = if states.role == Some("subsurface") {
            states.cached_state.current::<SubsurfaceCachedState>().location
        } else {
            Default::default()
        };
        SurfaceView { src, dst, offset }
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn rect_to_global<N>(&self, rect: Rectangle<N, Logical>) -> Rectangle<f64, Logical>
    where
        N: Coordinate,
    {
        let scale = self.scale();
        let mut rect = rect.to_f64();
        rect.loc -= self.src.loc;
        rect.upscale(scale)
    }

    pub(crate) fn rect_to_local<N>(&self, rect: Rectangle<N, Logical>) -> Rectangle<f64, Logical>
    where
        N: Coordinate,
    {
        let scale = self.scale();
        let mut rect = rect.to_f64().downscale(scale);
        rect.loc += self.src.loc;
        rect
    }

    fn scale(&self) -> Scale<f64> {
        Scale::from((
            self.dst.w as f64 / self.src.size.w,
            self.dst.h as f64 / self.src.size.h,
        ))
    }
}

/// Access the buffer related states associated to this surface
///
/// Calls [`compositor::with_states`] internally
pub fn with_renderer_surface_state<F, T>(surface: &WlSurface, cb: F) -> T
where
    F: FnOnce(&mut RendererSurfaceState) -> T,
{
    compositor::with_states(surface, |states| {
        let mut data = states
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .unwrap()
            .borrow_mut();
        cb(&mut data)
    })
}

/// Imports buffers of a surface and its subsurfaces using a given [`Renderer`].
///
/// This can be called early as an optimization, if `draw_surface_tree` is used later.
/// `draw_surface_tree` will also import buffers as necessary, but calling `import_surface_tree`
/// already may allow buffer imports to happen before compositing takes place, depending
/// on your event loop.
///
/// Note: This will do nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
pub fn import_surface_tree<R>(
    renderer: &mut R,
    surface: &WlSurface,
    log: &slog::Logger,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    import_surface_tree_and(renderer, surface, 1.0, log, (0.0, 0.0).into(), |_, _, _| {})
}

fn import_surface_tree_and<F, R, S>(
    renderer: &mut R,
    surface: &WlSurface,
    scale: S,
    log: &slog::Logger,
    location: Point<f64, Physical>,
    processor: F,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    S: Into<Scale<f64>>,
    F: FnMut(&WlSurface, &SurfaceData, &Point<f64, Physical>),
{
    let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
    let mut result = Ok(());
    let scale = scale.into();
    with_surface_tree_downward(
        surface,
        location,
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
                let mut data_ref = data.borrow_mut();
                let data = &mut *data_ref;
                // Import a new buffer if necessary
                let last_commit = data.renderer_seen.get(&texture_id);
                let buffer_damage = data.damage_since(last_commit.copied());
                if let Entry::Vacant(e) = data.textures.entry(texture_id) {
                    if let Some(buffer) = data.buffer.as_ref() {
                        match renderer.import_buffer(buffer, Some(states), &buffer_damage) {
                            Some(Ok(m)) => {
                                e.insert(Box::new(m));
                                data.renderer_seen.insert(texture_id, data.commit_count);
                            }
                            Some(Err(err)) => {
                                slog::warn!(log, "Error loading buffer: {}", err);
                                result = Err(err);
                            }
                            None => {
                                slog::error!(log, "Unknown buffer format for: {:?}", buffer);
                            }
                        }
                    }
                }
                // Now, should we be drawn ?
                if data.textures.contains_key(&texture_id) {
                    // if yes, also process the children
                    let surface_view = data.surface_view.unwrap();
                    location += surface_view.offset.to_f64().to_physical(scale);
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
        processor,
        |_, _, _| true,
    );
    result
}

#[derive(Debug, Default)]
struct RenderOp {
    src: Rectangle<f64, BufferCoord>,
    dst: Rectangle<i32, Physical>,
    damage: Vec<Rectangle<i32, Physical>>,
}

/// Draws a surface and its subsurfaces using a given [`Renderer`] and [`Frame`].
///
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `location` is the position the surface should be drawn at.
/// - `damage` is the set of regions that should be drawn relative to the same origin as the location.
///
/// Note: This element will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[allow(clippy::too_many_arguments)]
pub fn draw_surface_tree<R, S>(
    renderer: &mut R,
    frame: &mut <R as Renderer>::Frame,
    surface: &WlSurface,
    scale: S,
    location: Point<f64, Physical>,
    damage: &[Rectangle<i32, Physical>],
    log: &slog::Logger,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    S: Into<Scale<f64>>,
{
    trace!(
        log,
        "Rendering surface tree at {:?} with damage {:#?}",
        location,
        damage
    );

    // First pass is from near to far and will set-up everything we need for rendering
    // We use two passes so that we can reduce the damage from near to far by the opaque
    // region
    let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
    let mut result = Ok(());
    let scale = scale.into();
    let mut damage = damage.to_vec();
    let _ = import_surface_tree_and(
        renderer,
        surface,
        scale,
        log,
        location,
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
                let mut data = data.borrow_mut();
                let surface_view = data.surface_view;
                let buffer_scale = data.buffer_scale;
                let buffer_transform = data.buffer_transform;
                let buffer_dimensions = data.buffer_dimensions;
                let opaque_regions = data.opaque_regions().map(|regions| regions.to_vec());
                if data
                    .textures
                    .get_mut(&texture_id)
                    .and_then(|x| x.downcast_mut::<<R as Renderer>::TextureId>())
                    .is_some()
                {
                    let surface_view = surface_view.unwrap();
                    // Add the surface offset again to the location as
                    // with_surface_tree_upward only passes the updated
                    // location to its children
                    location += surface_view.offset.to_f64().to_physical(scale);

                    let dst = Rectangle::from_loc_and_size(
                        location.to_i32_round(),
                        ((surface_view.dst.to_f64().to_physical(scale).to_point() + location).to_i32_round()
                            - location.to_i32_round())
                        .to_size(),
                    );

                    states
                        .data_map
                        .insert_if_missing(|| RefCell::new(RenderOp::default()));
                    let render_op = states.data_map.get::<RefCell<RenderOp>>().unwrap();
                    let mut render_op = render_op.borrow_mut();
                    render_op.damage.clear();
                    render_op.damage.extend(
                        damage
                            .iter()
                            .cloned()
                            // clamp to surface size
                            .flat_map(|geo| geo.intersection(dst))
                            // move relative to surface
                            .map(|mut geo| {
                                geo.loc -= dst.loc;
                                geo
                            }),
                    );

                    let src = surface_view.src.to_buffer(
                        buffer_scale as f64,
                        buffer_transform,
                        &buffer_dimensions
                            .unwrap()
                            .to_logical(buffer_scale, buffer_transform)
                            .to_f64(),
                    );

                    render_op.src = src;
                    render_op.dst = dst;

                    // Now that we know the damage of the current surface we can
                    // remove all opaque regions from it so that any surface below
                    // us will only receive damage outside of the opaque regions
                    if let Some(opaque_regions) = opaque_regions {
                        damage = opaque_regions
                            .iter()
                            .map(|r| {
                                let loc = (r.loc.to_f64().to_physical(scale) + location).to_i32_round();
                                let size = ((r.size.to_f64().to_physical(scale).to_point() + location)
                                    .to_i32_round()
                                    - location.to_i32_round())
                                .to_size();
                                Rectangle::<i32, Physical>::from_loc_and_size(loc, size)
                            })
                            .fold(damage.clone(), |damage, region| {
                                damage
                                    .into_iter()
                                    .flat_map(|geo| geo.subtract_rect(region))
                                    .collect::<Vec<_>>()
                            })
                            .into_iter()
                            .collect::<Vec<_>>();
                    }
                }
            }
        },
    );

    // Second pass actually renders the surfaces with the reduced damage
    with_surface_tree_upward(
        surface,
        (),
        |_, states, _| {
            if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
                let mut data_ref = data.borrow_mut();
                let data = &mut *data_ref;

                // Now, should we be drawn ?
                if data.textures.contains_key(&texture_id) {
                    // if yes, also process the children
                    TraversalAction::DoChildren(())
                } else {
                    // we are not displayed, so our children are neither
                    TraversalAction::SkipChildren
                }
            } else {
                // we are not displayed, so our children are neither
                TraversalAction::SkipChildren
            }
        },
        |_, states, _| {
            if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
                let mut data = data.borrow_mut();
                let buffer_transform = data.buffer_transform;
                if let Some(texture) = data
                    .textures
                    .get_mut(&texture_id)
                    .and_then(|x| x.downcast_mut::<<R as Renderer>::TextureId>())
                {
                    let render_op = states.data_map.get::<RefCell<RenderOp>>().unwrap();
                    let render_op = render_op.borrow();

                    if render_op.damage.is_empty() {
                        return;
                    }

                    trace!(log, "Rendering surface {:#?}", render_op);

                    if let Err(err) = frame.render_texture_from_to(
                        texture,
                        render_op.src,
                        render_op.dst,
                        &render_op.damage,
                        buffer_transform,
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
