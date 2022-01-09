//! Utility module for helpers around drawing [`WlSurface`]s with [`Renderer`]s.

#[cfg(feature = "desktop")]
use crate::utils::Coordinate;
use crate::{
    backend::renderer::{buffer_dimensions, Frame, ImportAll, Renderer},
    utils::{Buffer, Logical, Point, Rectangle, Scale, Size, Transform},
    wayland::compositor::{
        add_destruction_hook, is_sync_subsurface, with_surface_tree_upward, BufferAssignment, Damage,
        SubsurfaceCachedState, SurfaceAttributes, SurfaceData, TraversalAction,
    },
};
use std::collections::VecDeque;
use std::{
    any::TypeId,
    cell::RefCell,
    collections::{hash_map::Entry, HashMap},
};
use wayland_server::{
    protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
    DisplayHandle,
};

#[derive(Default)]
pub(crate) struct SurfaceState {
    pub(crate) commit_count: usize,
    pub(crate) buffer_dimensions: Option<Size<i32, Buffer>>,
    pub(crate) buffer_scale: i32,
    pub(crate) buffer_transform: Transform,
    pub(crate) buffer: Option<WlBuffer>,
    pub(crate) damage: VecDeque<Vec<Rectangle<i32, Buffer>>>,
    pub(crate) renderer_seen: HashMap<(TypeId, usize), usize>,
    pub(crate) textures: HashMap<(TypeId, usize), Box<dyn std::any::Any>>,
    pub(crate) surface_view: Option<SurfaceView>,
    // #[cfg(feature = "desktop")]
    // pub(crate) space_seen: HashMap<crate::desktop::space::SpaceOutputHash, usize>,
}

const MAX_DAMAGE: usize = 4;

impl SurfaceState {
    pub fn update_buffer(&mut self, dh: &mut DisplayHandle<'_>, attrs: &mut SurfaceAttributes) {
        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer { buffer, .. }) => {
                // new contents
                self.buffer_dimensions = buffer_dimensions(&buffer);

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
                        old_buffer.release(dh);
                    }
                }
                self.textures.clear();
                self.commit_count = self.commit_count.wrapping_add(1);

                let surface_size = self
                    .buffer_dimensions
                    .unwrap()
                    .to_logical(self.buffer_scale, self.buffer_transform);
                self.surface_view = Some(SurfaceView::from_states(states, surface_size));

                let mut buffer_damage = attrs
                    .damage
                    .drain(..)
                    .flat_map(|dmg| {
                        match dmg {
                            Damage::Buffer(rect) => rect,
                            Damage::Surface(rect) => {
                                rect.to_buffer(self.buffer_scale, self.buffer_transform, &surface_size)
                            }
                        }
                        .intersection(Rectangle::from_loc_and_size(
                            (0, 0),
                            self.buffer_dimensions.unwrap(),
                        ))
                    })
                    .collect::<Vec<Rectangle<i32, Buffer>>>();
                buffer_damage.dedup();
                self.damage.push_front(buffer_damage);
                self.damage.truncate(MAX_DAMAGE);
            }
            Some(BufferAssignment::Removed) => {
                // remove the contents
                self.buffer_dimensions = None;
                if let Some(buffer) = self.buffer.take() {
                    buffer.release(dh);
                };
                self.textures.clear();
                self.commit_count = self.commit_count.wrapping_add(1);
                self.damage.clear();
                self.surface_view = None;
            }
            None => {}
        }
    }

    pub(crate) fn damage_since(&self, commit: Option<usize>) -> Vec<Rectangle<i32, Buffer>> {
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
    pub fn reset_space_damage(&mut self) {
        self.space_seen.clear();
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
pub fn on_commit_buffer_handler(dh: &mut DisplayHandle<'_>, surface: &WlSurface) {
    if !is_sync_subsurface(surface) {
        with_surface_tree_upward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |surf, states, _| {
                if states
                    .data_map
                    .insert_if_missing(|| RefCell::new(SurfaceState::default()))
                {
                    add_destruction_hook(surf, |data| {
                        if let Some(buffer) = data
                            .data_map
                            .get::<RefCell<SurfaceState>>()
                            .and_then(|s| s.borrow_mut().buffer.take())
                        {
                            buffer.release();
                        }
                    });
                }
                let mut data = states
                    .data_map
                    .get::<RefCell<SurfaceState>>()
                    .unwrap()
                    .borrow_mut();
                data.update_buffer(dh, &mut *states.cached_state.current::<SurfaceAttributes>());
            },
            |_, _, _| true,
        );
    }
}

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub(crate) struct SurfaceView {
    pub src: Rectangle<f64, Logical>,
    pub dst: Size<i32, Logical>,
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

    #[cfg(feature = "desktop")]
    fn scale(&self) -> Scale<f64> {
        Scale::from((
            self.dst.w as f64 / self.src.size.w,
            self.dst.h as f64 / self.src.size.h,
        ))
    }
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
    with_surface_tree_upward(
        surface,
        location,
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
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

/// Draws a surface and its subsurfaces using a given [`Renderer`] and [`Frame`].
///
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `location` is the position the surface should be drawn at.
/// - `damage` is the set of regions that should be drawn relative to the same origin as the location.
///
/// Note: This element will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
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
    let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
    let mut result = Ok(());
    let scale = scale.into();
    let _ = import_surface_tree_and(
        renderer,
        surface,
        scale,
        log,
        location,
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                let mut data = data.borrow_mut();
                let surface_view = data.surface_view;
                let buffer_scale = data.buffer_scale;
                let buffer_transform = data.buffer_transform;
                let buffer_dimensions = data.buffer_dimensions;
                if let Some(texture) = data
                    .textures
                    .get_mut(&texture_id)
                    .and_then(|x| x.downcast_mut::<<R as Renderer>::TextureId>())
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

                    let damage = damage
                        .iter()
                        .cloned()
                        // first move the damage by the surface offset in logical space
                        .map(|geo| geo.to_f64())
                        // then clamp to surface size again in logical space
                        .flat_map(|geo| geo.intersection(dst))
                        // lastly transform it into physical space
                        .map(|mut geo| {
                            geo.loc -= dst.loc;
                            geo
                        })
                        .map(|geo| geo.to_f64())
                        .collect::<Vec<_>>();

                    if damage.is_empty() {
                        return;
                    }

                    let src = surface_view.src.to_buffer(
                        buffer_scale as f64,
                        buffer_transform,
                        &buffer_dimensions
                            .unwrap()
                            .to_logical(buffer_scale, buffer_transform)
                            .to_f64(),
                    );
                    if let Err(err) = frame.render_texture_from_to(
                        texture,
                        src,
                        dst.to_f64(),
                        &damage,
                        buffer_transform,
                        1.0,
                    ) {
                        result = Err(err);
                    }
                }
            }
        },
    );
    result
}
