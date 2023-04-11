//! Utility module for helpers around drawing [`WlSurface`](wayland_server::protocol::wl_surface::WlSurface)s
//! and [`RenderElement`](super::element::RenderElement)s with [`Renderer`](super::Renderer)s.

use tracing::instrument;

use crate::utils::{Buffer as BufferCoord, Coordinate, Logical, Physical, Point, Rectangle, Scale, Size};
use std::{collections::VecDeque, fmt, sync::Arc};

#[cfg(feature = "wayland_frontend")]
mod wayland;
#[cfg(feature = "wayland_frontend")]
pub use self::wayland::*;

use super::{element::RenderElement, Renderer};

/// A simple wrapper for counting commits
///
/// The purpose of the counter is to keep track
/// on the number of times something has changed.
/// It provides an easy way to obtain the distance
/// between two instances of a [`CommitCounter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct CommitCounter(usize);

impl CommitCounter {
    /// Increment the commit counter
    pub fn increment(&mut self) {
        self.0 = self.0.wrapping_add(1)
    }

    /// Get the distance between two [`CommitCounter`]s
    ///
    /// If the [`CommitCounter`] is incremented on each recorded
    /// damage this returns the count of damage that happened
    /// between the [`CommitCounter`]s
    ///
    /// Returns `None` in case the distance could not be calculated.
    /// If uses as part of damage tracking the tracked element
    /// should be considered as fully damaged.
    pub fn distance(&self, previous_commit: Option<CommitCounter>) -> Option<usize> {
        // if commit > commit_count we have overflown, in that case the following map might result
        // in a false-positive, if commit is still very large. So we force false in those cases.
        // That will result in a potentially sub-optimal full damage every usize::MAX frames,
        // which is acceptable.
        previous_commit
            .filter(|commit| commit <= self)
            .map(|commit| self.0.wrapping_sub(commit.0))
    }
}

impl From<usize> for CommitCounter {
    fn from(counter: usize) -> Self {
        CommitCounter(counter)
    }
}

/// A tracker for holding damage
///
/// It keeps track of the submitted damage
/// and automatically caps the damage
/// with the specified limit.
///
/// See [`DamageSnapshot`] for more
/// information.
pub struct DamageBag<N, Kind> {
    limit: usize,
    state: DamageSnapshot<N, Kind>,
}

/// A snapshot of the current state of a [`DamageBag`]
///
/// The snapshot can be used to get an immutable view
/// into the current state of a [`DamageBag`].
/// It provides an easy way to get the damage between two
/// [`CommitCounter`]s.
pub struct DamageSnapshot<N, Kind> {
    limit: usize,
    commit_counter: CommitCounter,
    damage: Arc<VecDeque<Vec<Rectangle<N, Kind>>>>,
}

impl<N, Kind> Clone for DamageSnapshot<N, Kind> {
    fn clone(&self) -> Self {
        Self {
            limit: self.limit,
            commit_counter: self.commit_counter,
            damage: self.damage.clone(),
        }
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageBag<N, BufferCoord> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageBag")
            .field("limit", &self.limit)
            .field("state", &self.state)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageBag<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageBag")
            .field("limit", &self.limit)
            .field("state", &self.state)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageBag<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageBag")
            .field("limit", &self.limit)
            .field("state", &self.state)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSnapshot<N, BufferCoord> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSnapshot")
            .field("commit_counter", &self.commit_counter)
            .field("damage", &self.damage)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSnapshot<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSnapshot")
            .field("commit_counter", &self.commit_counter)
            .field("damage", &self.damage)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSnapshot<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSnapshot")
            .field("commit_counter", &self.commit_counter)
            .field("damage", &self.damage)
            .finish()
    }
}

const MAX_DAMAGE: usize = 4;

impl<N: Clone, Kind> Default for DamageBag<N, Kind> {
    fn default() -> Self {
        DamageBag::new(MAX_DAMAGE)
    }
}

impl<N: Clone, Kind> DamageSnapshot<N, Kind> {
    fn new(limit: usize) -> Self {
        DamageSnapshot {
            limit,
            commit_counter: CommitCounter::default(),
            damage: Arc::new(VecDeque::with_capacity(limit)),
        }
    }

    /// Create an empty damage snapshot
    pub fn empty() -> Self {
        DamageSnapshot {
            limit: 0,
            commit_counter: CommitCounter::default(),
            damage: Default::default(),
        }
    }

    /// Gets the current [`CommitCounter`] of this snapshot
    ///
    /// The returned [`CommitCounter`] should be stored after
    /// calling [`damage_since`](DamageSnapshot::damage_since)
    /// and provided to the next call of [`damage_since`](DamageSnapshot::damage_since)
    /// to query the damage between these two [`CommitCounter`]s.
    pub fn current_commit(&self) -> CommitCounter {
        self.commit_counter
    }

    /// Provides raw access to the stored damage
    pub fn damage(&self) -> impl Iterator<Item = &Vec<Rectangle<N, Kind>>> {
        self.damage.iter()
    }

    fn reset(&mut self) {
        Arc::make_mut(&mut self.damage).clear();
        self.commit_counter.increment();
    }
}

impl<N: Coordinate, Kind> DamageSnapshot<N, Kind> {
    /// Get the damage since the last commit
    ///
    /// Returns `None` in case the [`CommitCounter`] is too old
    /// or the damage has been reset. In that case the whole
    /// element geometry should be considered as damaged
    ///
    /// If the commit is recent enough and no damage has occurred
    /// an empty `Vec` will be returned
    pub fn damage_since(&self, commit: Option<CommitCounter>) -> Option<Vec<Rectangle<N, Kind>>> {
        let distance = self.commit_counter.distance(commit);

        if distance
            .map(|distance| distance <= self.damage.len())
            .unwrap_or(false)
        {
            Some(
                self.damage
                    .iter()
                    .take(distance.unwrap())
                    .fold(Vec::new(), |mut acc, elem| {
                        acc.extend(elem);
                        acc
                    }),
            )
        } else {
            None
        }
    }

    fn add(&mut self, damage: impl IntoIterator<Item = Rectangle<N, Kind>>) {
        let damage = damage.into_iter().collect::<Vec<_>>();

        if damage.is_empty() || damage.iter().all(|d| d.is_empty()) {
            // do not track empty damage
            return;
        }

        let mut damage = damage
            .iter()
            .copied()
            .filter(|d| !d.is_empty())
            .collect::<Vec<_>>();
        damage.dedup();

        let inner_damage = Arc::make_mut(&mut self.damage);
        inner_damage.push_front(damage);
        inner_damage.truncate(self.limit);

        self.commit_counter.increment();
    }
}

impl<N: Clone, Kind> DamageBag<N, Kind> {
    /// Initialize a a new [`DamageBag`] with the specified limit
    pub fn new(limit: usize) -> Self {
        DamageBag {
            limit,
            state: DamageSnapshot::new(limit),
        }
    }

    /// Gets the current [`CommitCounter`] of this tracker
    pub fn current_commit(&self) -> CommitCounter {
        self.state.current_commit()
    }

    /// Provides raw access to the stored damage
    pub fn damage(&self) -> impl Iterator<Item = &Vec<Rectangle<N, Kind>>> {
        self.state.damage()
    }

    /// Reset the damage
    ///
    /// This should be called when the
    /// tracked item has been resized
    pub fn reset(&mut self) {
        self.state.reset()
    }
}

impl<N, Kind> DamageBag<N, Kind> {
    /// Get a snapshot of the current damage
    pub fn snapshot(&self) -> DamageSnapshot<N, Kind> {
        self.state.clone()
    }
}

impl<N: Coordinate, Kind> DamageBag<N, Kind> {
    /// Add some damage to the tracker
    pub fn add(&mut self, damage: impl IntoIterator<Item = Rectangle<N, Kind>>) {
        self.state.add(damage)
    }

    /// Get the damage since the last commit
    ///
    /// Returns `None` in case the [`CommitCounter`] is too old
    /// or the damage has been reset. In that case the whole
    /// element geometry should be considered as damaged
    ///
    /// If the commit is recent enough and no damage has occurred
    /// an empty `Vec` will be returned
    pub fn damage_since(&self, commit: Option<CommitCounter>) -> Option<Vec<Rectangle<N, Kind>>> {
        self.state.damage_since(commit)
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
        let is_hidden = opaque_regions
            .iter()
            .fold([element_geometry].to_vec(), |geometry, opaque_region| {
                geometry
                    .into_iter()
                    .flat_map(|g| g.subtract_rect(*opaque_region))
                    .collect::<Vec<_>>()
            })
            .is_empty();

        if is_hidden {
            // No need to draw a completely hidden element
            continue;
        }

        let damage = opaque_regions
            .iter()
            .fold(damage.to_vec(), |damage, opaque_region| {
                damage
                    .into_iter()
                    .flat_map(|damage| damage.subtract_rect(*opaque_region))
                    .collect::<Vec<_>>()
            });

        render_damage.extend(damage);

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
