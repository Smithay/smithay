use crate::{
    backend::renderer::{buffer_dimensions, buffer_has_alpha, ImportAll, Renderer},
    utils::{Buffer as BufferCoord, Coordinate, Logical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        compositor::{
            self, is_sync_subsurface, with_surface_tree_downward, with_surface_tree_upward, BufferAssignment,
            Damage, RectangleKind, SubsurfaceCachedState, SurfaceAttributes, SurfaceData, TraversalAction,
        },
        viewporter,
    },
};
use std::sync::Arc;
use std::{
    any::TypeId,
    cell::RefCell,
    collections::{hash_map::Entry, HashMap},
};
use tracing::{error, instrument, warn};

use wayland_server::protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface};

use super::{CommitCounter, DamageBag, DamageSnapshot, SurfaceView};

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
    pub(crate) buffer_dimensions: Option<Size<i32, BufferCoord>>,
    pub(crate) buffer_scale: i32,
    pub(crate) buffer_transform: Transform,
    pub(crate) buffer_delta: Option<Point<i32, Logical>>,
    pub(crate) buffer_has_alpha: Option<bool>,
    pub(crate) buffer: Option<Buffer>,
    pub(crate) damage: DamageBag<i32, BufferCoord>,
    pub(crate) renderer_seen: HashMap<(TypeId, usize), CommitCounter>,
    pub(crate) textures: HashMap<(TypeId, usize), Box<dyn std::any::Any>>,
    pub(crate) surface_view: Option<SurfaceView>,
    pub(crate) opaque_regions: Vec<Rectangle<i32, Logical>>,

    accumulated_buffer_delta: Point<i32, Logical>,
}

#[derive(Debug)]
struct InnerBuffer(WlBuffer);

impl Drop for InnerBuffer {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// A wayland buffer
#[derive(Debug, Clone)]
pub struct Buffer {
    inner: Arc<InnerBuffer>,
}

impl From<WlBuffer> for Buffer {
    fn from(buffer: WlBuffer) -> Self {
        Buffer {
            inner: Arc::new(InnerBuffer(buffer)),
        }
    }
}

impl std::ops::Deref for Buffer {
    type Target = WlBuffer;

    fn deref(&self) -> &Self::Target {
        &self.inner.0
    }
}

impl PartialEq<WlBuffer> for Buffer {
    fn eq(&self, other: &WlBuffer) -> bool {
        self.inner.0 == *other
    }
}

impl PartialEq<WlBuffer> for &Buffer {
    fn eq(&self, other: &WlBuffer) -> bool {
        self.inner.0 == *other
    }
}

impl RendererSurfaceState {
    pub(crate) fn update_buffer<F>(&mut self, states: &SurfaceData, buffer_info: &F)
    where
        F: Fn(&WlBuffer) -> Option<(Size<i32, BufferCoord>, bool)>,
    {
        let mut attrs = states.cached_state.current::<SurfaceAttributes>();
        self.buffer_delta = attrs.buffer_delta.take();

        if let Some(delta) = self.buffer_delta {
            self.accumulated_buffer_delta += delta;
        }

        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer(buffer)) => {
                let (buffer_dimensions, buffer_has_alpha) = match buffer_info(&buffer) {
                    Some(info) => info,
                    None => {
                        warn!("failed to retrieve buffer info");
                        self.reset();
                        return;
                    }
                };
                // new contents
                self.buffer_dimensions = Some(buffer_dimensions);
                self.buffer_has_alpha = Some(buffer_has_alpha);
                self.buffer_scale = attrs.buffer_scale;
                self.buffer_transform = attrs.buffer_transform.into();

                if !self.buffer.as_ref().map_or(false, |b| b == buffer) {
                    self.buffer = Some(Buffer::from(buffer));
                }

                self.textures.clear();

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
                self.damage.add(buffer_damage);

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
                                            .filter(|region| region.overlaps_or_touches(rect))
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
                self.reset();
            }
            None => {}
        }
    }

    /// Get the current commit position of this surface
    ///
    /// The position should be saved after calling [`damage_since`](RendererSurfaceState::damage_since) and
    /// provided as the commit in the next call.
    pub fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    /// Gets the damage since the last commit
    ///
    /// If either the commit is `None` or the commit is too old
    /// the whole buffer will be returned as damage.
    pub fn damage_since(&self, commit: Option<CommitCounter>) -> Vec<Rectangle<i32, BufferCoord>> {
        self.damage.damage_since(commit).unwrap_or_else(|| {
            self.buffer_dimensions
                .as_ref()
                .map(|size| vec![Rectangle::from_loc_and_size((0, 0), *size)])
                .unwrap_or_else(Vec::new)
        })
    }

    /// Gets the damage of this surface
    pub fn damage(&self) -> DamageSnapshot<i32, BufferCoord> {
        self.damage.snapshot()
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
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    /// Returns the current buffer scale
    pub fn buffer_scale(&self) -> i32 {
        self.buffer_scale
    }

    /// Returns the current buffer transform
    pub fn buffer_transform(&self) -> Transform {
        self.buffer_transform
    }

    /// Gets a reference to the texture for the specified renderer
    pub fn texture<R>(&self, id: usize) -> Option<&R::TextureId>
    where
        R: Renderer,
        <R as Renderer>::TextureId: 'static,
    {
        let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), id);
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

    fn reset(&mut self) {
        self.buffer_dimensions = None;
        self.buffer = None;
        self.textures.clear();
        self.damage.reset();
        self.surface_view = None;
        self.buffer_has_alpha = None;
        self.opaque_regions.clear();
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
pub fn on_commit_custom_buffer_handler<F>(surface: &WlSurface, buffer_info: F)
where
    F: Fn(&WlBuffer) -> Option<(Size<i32, BufferCoord>, bool)>,
{
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
                data.update_buffer(states, &buffer_info);
            },
            |_, _, _| true,
        );
    }
}

/// Retrieves the buffer info for a smithay handled buffer type
pub fn buffer_info(buffer: &WlBuffer) -> Option<(Size<i32, BufferCoord>, bool)> {
    buffer_dimensions(buffer).map(|d| (d, buffer_has_alpha(buffer).unwrap_or(true)))
}

/// Handler to let smithay take over buffer management for smithay handled buffer types.
///
/// Needs to be called first on the commit-callback of
/// [`crate::wayland::compositor::CompositorHandler::commit`].
///
/// Consumes the buffer of [`SurfaceAttributes`], the buffer will
/// not be accessible anymore, but [`draw_surface_tree`] and other
/// `draw_*` helpers of the [desktop module](`crate::desktop`) will
/// become usable for surfaces handled this way.
pub fn on_commit_buffer_handler(surface: &WlSurface) {
    on_commit_custom_buffer_handler(surface, buffer_info)
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

/// Imports buffers of a surface using a given [`Renderer`] and import function.
///
/// Same as [`import_custom_surface`], but without implicitely borrowing the
/// [`RendererSurfaceState`]. This can be usefull in case the [`RendererSurfaceState`]
/// is already borrowed.
pub fn import_custom_renderer_surface<R, F>(
    renderer: &mut R,
    states: &SurfaceData,
    data: &mut RendererSurfaceState,
    import: F,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer,
    <R as Renderer>::TextureId: 'static,
    F: Fn(
        &mut R,
        &WlBuffer,
        &SurfaceData,
        &[Rectangle<i32, BufferCoord>],
    ) -> Option<Result<<R as Renderer>::TextureId, <R as Renderer>::Error>>,
{
    let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
    let last_commit = data.renderer_seen.get(&texture_id);
    let buffer_damage = data.damage_since(last_commit.copied());
    if let Entry::Vacant(e) = data.textures.entry(texture_id) {
        if let Some(buffer) = data.buffer.as_ref() {
            // There is no point in importing a single pixel buffer
            if matches!(
                crate::backend::renderer::buffer_type(buffer),
                Some(crate::backend::renderer::BufferType::SinglePixel)
            ) {
                return Ok(());
            }

            match import(renderer, buffer, states, &buffer_damage) {
                Some(Ok(m)) => {
                    e.insert(Box::new(m));
                    data.renderer_seen.insert(texture_id, data.current_commit());
                }
                Some(Err(err)) => {
                    warn!("Error loading buffer: {}", err);
                    return Err(err);
                }
                None => {
                    error!("Unknown buffer format for: {:?}", buffer);
                }
            }
        }
    }

    Ok(())
}

/// Imports buffers handled by smithay of a surface using a given [`Renderer`]
///
/// Same as [`import_surface`], but without implicitely borrowing the
/// [`RendererSurfaceState`]. This can be usefull in case the [`RendererSurfaceState`]
/// is already borrowed.
pub fn import_renderer_surface<R>(
    renderer: &mut R,
    states: &SurfaceData,
    data: &mut RendererSurfaceState,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    import_custom_renderer_surface(renderer, states, data, |renderer, buffer, states, damage| {
        renderer.import_buffer(buffer, Some(states), damage)
    })
}

/// Imports buffers of a surface using a given [`Renderer`] and import function.
///
/// This (or `import_surface_tree`) need to be called before`draw_surface_tree`, if used later.
///
/// Note: This will do nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
pub fn import_custom_surface<R, F>(
    renderer: &mut R,
    states: &SurfaceData,
    import: F,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer,
    <R as Renderer>::TextureId: 'static,
    F: Fn(
        &mut R,
        &WlBuffer,
        &SurfaceData,
        &[Rectangle<i32, BufferCoord>],
    ) -> Option<Result<<R as Renderer>::TextureId, <R as Renderer>::Error>>,
{
    if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
        import_custom_renderer_surface(renderer, states, &mut data.borrow_mut(), import)?;
    }

    Ok(())
}

/// Imports buffers handled by smithay of a surface using a given [`Renderer`]
///
/// This (or `import_surface_tree`) need to be called before`draw_surface_tree`, if used later.
///
/// Note: This will do nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[instrument(level = "trace", skip_all)]
pub fn import_surface<R>(renderer: &mut R, states: &SurfaceData) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
        import_renderer_surface(renderer, states, &mut data.borrow_mut())?;
    }

    Ok(())
}

/// Imports buffers of a surface and its subsurfaces using a given [`Renderer`] and import function.
///
/// This (or `import_surface`) need to be called before`draw_surface_tree`, if used later.
///
/// Note: This will do nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[instrument(level = "trace", skip_all)]
pub fn import_custom_surface_tree<R, F>(
    renderer: &mut R,
    surface: &WlSurface,
    import: F,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer,
    <R as Renderer>::TextureId: 'static,
    F: Fn(&mut R, &SurfaceData, &mut RendererSurfaceState) -> Result<(), <R as Renderer>::Error> + Copy,
{
    let mut result = Ok(());
    with_surface_tree_downward(
        surface,
        (),
        |_surface, states, _| {
            if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
                let mut data_ref = data.borrow_mut();
                let data = &mut *data_ref;

                if let Err(err) = import(renderer, states, data) {
                    result = Err(err);
                }

                // Now, should we be drawn ?
                if data.view().is_some() {
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
        |_, _, _| {},
        |_, _, _| true,
    );
    result
}

/// Imports buffers handled by smithay of a surface and its subsurfaces using a given [`Renderer`].
///
/// This (or `import_surface`) need to be called before`draw_surface_tree`, if used later.
///
/// Note: This will do nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[instrument(level = "trace", skip_all)]
pub fn import_surface_tree<R>(renderer: &mut R, surface: &WlSurface) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    import_custom_surface_tree(renderer, surface, import_renderer_surface)
}
