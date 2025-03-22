//! Utility module for helpers around drawing [`WlSurface`](wayland_server::protocol::wl_surface::WlSurface)s
//! and [`RenderElement`](super::element::RenderElement)s with [`Renderer`](super::Renderer)s.

use crate::utils::{Buffer as BufferCoord, Coordinate, Logical, Physical, Point, Rectangle, Size};
use std::{collections::VecDeque, fmt, sync::Arc};

#[cfg(feature = "wayland_frontend")]
mod wayland;
#[cfg(feature = "wayland_frontend")]
pub use self::wayland::*;

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
    #[inline]
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
    damage: Arc<VecDeque<smallvec::SmallVec<[Rectangle<N, Kind>; MAX_DAMAGE_RECTS]>>>,
}

impl<N, Kind> Clone for DamageSnapshot<N, Kind> {
    #[inline]
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

const MAX_DAMAGE_AGE: usize = 4;
const MAX_DAMAGE_RECTS: usize = 16;
const MAX_DAMAGE_SET: usize = MAX_DAMAGE_RECTS * 2;

impl<N: Clone, Kind> Default for DamageBag<N, Kind> {
    #[inline]
    fn default() -> Self {
        DamageBag::new(MAX_DAMAGE_AGE)
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
    #[inline]
    pub fn current_commit(&self) -> CommitCounter {
        self.commit_counter
    }

    /// Provides raw access to the stored damage
    pub fn raw(&self) -> impl Iterator<Item = impl Iterator<Item = &Rectangle<N, Kind>>> {
        self.damage.iter().map(|d| d.iter())
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
    pub fn damage_since(&self, commit: Option<CommitCounter>) -> Option<DamageSet<N, Kind>> {
        let distance = self.commit_counter.distance(commit);

        if distance
            .map(|distance| distance <= self.damage.len())
            .unwrap_or(false)
        {
            let mut damage_set = DamageSet::default();
            for damage in self.damage.iter().take(distance.unwrap()) {
                damage_set.damage.extend_from_slice(damage);
            }
            Some(damage_set)
        } else {
            None
        }
    }

    fn add(&mut self, damage: impl IntoIterator<Item = Rectangle<N, Kind>>) {
        // FIXME: Get rid of this allocation here
        let mut damage = damage.into_iter().filter(|d| !d.is_empty()).collect::<Vec<_>>();

        if damage.is_empty() {
            // do not track empty damage
            return;
        }

        damage.dedup();

        let inner_damage = Arc::make_mut(&mut self.damage);
        inner_damage.push_front(smallvec::SmallVec::from_vec(damage));
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
    #[inline]
    pub fn current_commit(&self) -> CommitCounter {
        self.state.current_commit()
    }

    /// Provides raw access to the stored damage
    pub fn raw(&self) -> impl Iterator<Item = impl Iterator<Item = &Rectangle<N, Kind>>> {
        self.state.raw()
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
    pub fn damage_since(&self, commit: Option<CommitCounter>) -> Option<DamageSet<N, Kind>> {
        self.state.damage_since(commit)
    }
}

/// A set of damage returned from [`DamageBag::damage_since`] of [`DamageSnapshot::damage_since`]
pub struct DamageSet<N, Kind> {
    damage: smallvec::SmallVec<[Rectangle<N, Kind>; MAX_DAMAGE_SET]>,
}

impl<N, Kind> Default for DamageSet<N, Kind> {
    fn default() -> Self {
        Self {
            damage: Default::default(),
        }
    }
}

impl<N: Copy, Kind> DamageSet<N, Kind> {
    /// Copy the damage from a slice into a new `DamageSet`.
    #[inline]
    pub fn from_slice(slice: &[Rectangle<N, Kind>]) -> Self {
        Self {
            damage: smallvec::SmallVec::from_slice(slice),
        }
    }
}

impl<N, Kind> std::ops::Deref for DamageSet<N, Kind> {
    type Target = [Rectangle<N, Kind>];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.damage
    }
}

impl<N, Kind> IntoIterator for DamageSet<N, Kind> {
    type Item = Rectangle<N, Kind>;

    type IntoIter = DamageSetIter<N, Kind>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        DamageSetIter {
            inner: self.damage.into_iter(),
        }
    }
}

impl<N, Kind> FromIterator<Rectangle<N, Kind>> for DamageSet<N, Kind> {
    #[inline]
    fn from_iter<T: IntoIterator<Item = Rectangle<N, Kind>>>(iter: T) -> Self {
        Self {
            damage: smallvec::SmallVec::from_iter(iter),
        }
    }
}

/// Iterator for [`DamageSet::into_iter`]
pub struct DamageSetIter<N, Kind> {
    inner: smallvec::IntoIter<[Rectangle<N, Kind>; MAX_DAMAGE_SET]>,
}

impl<N: fmt::Debug> fmt::Debug for DamageSetIter<N, BufferCoord> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSetIter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSetIter<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSetIter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSetIter<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSetIter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<N, Kind> Iterator for DamageSetIter<N, Kind> {
    type Item = Rectangle<N, Kind>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSet<N, BufferCoord> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSet").field("damage", &self.damage).finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSet<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSet").field("damage", &self.damage).finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for DamageSet<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DamageSet").field("damage", &self.damage).finish()
    }
}

const MAX_OPAQUE_REGIONS: usize = 16;

/// Wrapper for a set of opaque regions
pub struct OpaqueRegions<N, Kind> {
    regions: smallvec::SmallVec<[Rectangle<N, Kind>; MAX_OPAQUE_REGIONS]>,
}

impl<N, Kind> Default for OpaqueRegions<N, Kind>
where
    N: Default,
{
    #[inline]
    fn default() -> Self {
        Self {
            regions: Default::default(),
        }
    }
}

impl<N: Copy, Kind> OpaqueRegions<N, Kind> {
    /// Copy the opaque regions from a slice into a new `OpaqueRegions`.
    #[inline]
    pub fn from_slice(slice: &[Rectangle<N, Kind>]) -> Self {
        Self {
            regions: smallvec::SmallVec::from_slice(slice),
        }
    }
}

impl<N, Kind> std::ops::Deref for OpaqueRegions<N, Kind> {
    type Target = [Rectangle<N, Kind>];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.regions
    }
}

impl<N, Kind> IntoIterator for OpaqueRegions<N, Kind> {
    type Item = Rectangle<N, Kind>;

    type IntoIter = OpaqueRegionsIter<N, Kind>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        OpaqueRegionsIter {
            inner: self.regions.into_iter(),
        }
    }
}

impl<N, Kind> FromIterator<Rectangle<N, Kind>> for OpaqueRegions<N, Kind> {
    #[inline]
    fn from_iter<T: IntoIterator<Item = Rectangle<N, Kind>>>(iter: T) -> Self {
        Self {
            regions: smallvec::SmallVec::from_iter(iter),
        }
    }
}

/// Iterator for [`OpaqueRegions::into_iter`]
pub struct OpaqueRegionsIter<N, Kind> {
    inner: smallvec::IntoIter<[Rectangle<N, Kind>; MAX_OPAQUE_REGIONS]>,
}

impl<N: fmt::Debug> fmt::Debug for OpaqueRegionsIter<N, BufferCoord> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueRegionsIter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for OpaqueRegionsIter<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueRegionsIter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for OpaqueRegionsIter<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueRegionsIter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<N, Kind> Iterator for OpaqueRegionsIter<N, Kind> {
    type Item = Rectangle<N, Kind>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<N: fmt::Debug> fmt::Debug for OpaqueRegions<N, BufferCoord> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueRegions")
            .field("regions", &self.regions)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for OpaqueRegions<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueRegions")
            .field("regions", &self.regions)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for OpaqueRegions<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueRegions")
            .field("regions", &self.regions)
            .finish()
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
