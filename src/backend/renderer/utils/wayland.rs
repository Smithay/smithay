use crate::{
    backend::renderer::{buffer_dimensions, buffer_has_alpha, element::RenderElement, ImportAll, Renderer},
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
use std::sync::Arc;
use std::{
    any::TypeId,
    cell::RefCell,
    collections::{hash_map::Entry, HashMap},
};
use tracing::{error, instrument, warn};

use wayland_server::protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface};

use super::{CommitCounter, DamageBag, SurfaceView};

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
    #[profiling::function]
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
                    self.reset();
                    return;
                }
                self.buffer_has_alpha = buffer_has_alpha(&buffer);
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
                let surface_view =
                    SurfaceView::from_states(states, surface_size, self.accumulated_buffer_delta);
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
                                        let added_regions = rect.subtract_rects(
                                            new_regions
                                                .iter()
                                                .filter(|region| region.overlaps_or_touches(rect))
                                                .copied(),
                                        );
                                        new_regions.extend(added_regions);
                                    }
                                    RectangleKind::Subtract => {
                                        new_regions =
                                            Rectangle::subtract_rects_many_in_place(new_regions, [rect]);
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
                .unwrap_or_default()
        })
    }

    /// Gets the raw damage of this surface
    pub fn damage(&self) -> impl Iterator<Item = &Vec<Rectangle<i32, BufferCoord>>> {
        self.damage.damage()
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
/// not be accessible anymore, but [`draw_render_elements`] and other
/// `draw_*` helpers of the [desktop module](`crate::desktop`) will
/// become usable for surfaces handled this way.
#[profiling::function]
pub fn on_commit_buffer_handler<D: 'static>(surface: &WlSurface) {
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
            add_destruction_hook(surf, |_: &mut D, data| {
                // We reset the state on destruction before the user_data is dropped
                // to prevent a deadlock which can happen if we try to send a buffer
                // release during drop. This also enables us to free resources earlier
                // like the stored textures
                if let Some(mut state) = data
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .map(|s| s.borrow_mut())
                {
                    state.reset();
                }
            });
        }
    }
}

impl SurfaceView {
    fn from_states(
        states: &SurfaceData,
        surface_size: Size<i32, Logical>,
        buffer_delta: Point<i32, Logical>,
    ) -> SurfaceView {
        viewporter::ensure_viewport_valid(states, surface_size);
        let viewport = states.cached_state.current::<viewporter::ViewportCachedState>();
        let src = viewport
            .src
            .unwrap_or_else(|| Rectangle::from_loc_and_size((0.0, 0.0), surface_size.to_f64()));
        let dst = viewport.size().unwrap_or(surface_size);
        let mut offset = if states.role == Some("subsurface") {
            states.cached_state.current::<SubsurfaceCachedState>().location
        } else {
            Default::default()
        };
        offset += buffer_delta;
        SurfaceView { src, dst, offset }
    }

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
/// Calls [`compositor::with_states`] internally.
///
/// Returns `None`, if there never was an commit processed through `on_commit_buffer_handler`.
pub fn with_renderer_surface_state<F, T>(surface: &WlSurface, cb: F) -> Option<T>
where
    F: FnOnce(&mut RendererSurfaceState) -> T,
{
    compositor::with_states(surface, |states| {
        let data = states.data_map.get::<RendererSurfaceStateUserData>()?;
        Some(cb(&mut data.borrow_mut()))
    })
}

/// Imports buffers of a surface using a given [`Renderer`]
///
/// This (or `import_surface_tree`) need to be called before`draw_render_elements`, if used later.
///
/// Note: This will do nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[instrument(level = "trace", skip_all)]
#[profiling::function]
pub fn import_surface<R>(renderer: &mut R, states: &SurfaceData) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
        let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
        let mut data_ref = data.borrow_mut();
        let data = &mut *data_ref;

        let last_commit = data.renderer_seen.get(&texture_id);
        let buffer_damage = data.damage_since(last_commit.copied());
        if let Entry::Vacant(e) = data.textures.entry(texture_id) {
            if let Some(buffer) = data.buffer.as_ref() {
                match renderer.import_buffer(buffer, Some(states), &buffer_damage) {
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
    }

    Ok(())
}

/// Imports buffers of a surface and its subsurfaces using a given [`Renderer`].
///
/// This (or `import_surface`) need to be called before`draw_render_elements`, if used later.
///
/// Note: This will do nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[instrument(level = "trace", skip_all)]
#[profiling::function]
pub fn import_surface_tree<R>(renderer: &mut R, surface: &WlSurface) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    let scale = 1.0;
    let location: Point<f64, Physical> = (0.0, 0.0).into();

    let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
    let mut result = Ok(());
    with_surface_tree_downward(
        surface,
        location,
        |_surface, states, location| {
            let mut location = *location;
            // Import a new buffer if necessary
            if let Err(err) = import_surface(renderer, states) {
                result = Err(err);
            }

            if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() {
                let mut data_ref = data.borrow_mut();
                let data = &mut *data_ref;
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
        |_, _, _| {},
        |_, _, _| true,
    );
    result
}

/// Draws the render elements using a given [`Renderer`] and [`Frame`](crate::backend::renderer::Frame)
///
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `location` is the position the surface should be drawn at.
/// - `damage` is the set of regions that should be drawn relative to the same origin as the location.
///
/// Note: This element will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[instrument(level = "trace", skip(frame, scale, elements))]
#[profiling::function]
pub fn draw_render_elements<'a, R, S, E>(
    frame: &mut <R as Renderer>::Frame<'a>,
    scale: S,
    elements: &[E],
    damage: &[Rectangle<i32, Physical>],
) -> Result<Option<Vec<Rectangle<i32, Physical>>>, <R as Renderer>::Error>
where
    R: Renderer,
    <R as Renderer>::TextureId: 'static,
    S: Into<Scale<f64>>,
    E: RenderElement<R>,
{
    let scale = scale.into();

    let mut render_elements: Vec<&E> = Vec::with_capacity(elements.len());
    let mut opaque_regions: Vec<Rectangle<i32, Physical>> = Vec::new();
    let mut render_damage: Vec<Rectangle<i32, Physical>> = Vec::with_capacity(damage.len());

    for element in elements {
        let element_geometry = element.geometry(scale);

        // Then test if the element is completely hidden behind opaque regions
        let is_hidden = element_geometry
            .subtract_rects(opaque_regions.iter().copied())
            .is_empty();

        if is_hidden {
            // No need to draw a completely hidden element
            continue;
        }

        render_damage.extend(Rectangle::subtract_rects_many(
            damage.iter().copied(),
            opaque_regions.iter().copied(),
        ));

        opaque_regions.extend(element.opaque_regions(scale).into_iter().map(|mut region| {
            region.loc += element_geometry.loc;
            region
        }));
        render_elements.insert(0, element);
    }

    // Optimize the damage for rendering
    render_damage.dedup();
    render_damage.retain(|rect| !rect.is_empty());
    // filter damage outside of the output gep and merge overlapping rectangles
    render_damage = render_damage
        .into_iter()
        .fold(Vec::new(), |new_damage, mut rect| {
            // replace with drain_filter, when that becomes stable to reuse the original Vec's memory
            let (overlapping, mut new_damage): (Vec<_>, Vec<_>) = new_damage
                .into_iter()
                .partition(|other| other.overlaps_or_touches(rect));

            for overlap in overlapping {
                rect = rect.merge(overlap);
            }
            new_damage.push(rect);
            new_damage
        });

    if render_damage.is_empty() {
        return Ok(None);
    }

    for element in render_elements.iter() {
        let element_geometry = element.geometry(scale);

        let element_damage = damage
            .iter()
            .filter_map(|d| d.intersection(element_geometry))
            .map(|mut d| {
                d.loc -= element_geometry.loc;
                d
            })
            .collect::<Vec<_>>();

        if element_damage.is_empty() {
            continue;
        }

        element.draw(frame, element.src(), element_geometry, &element_damage)?;
    }

    Ok(Some(render_damage))
}
