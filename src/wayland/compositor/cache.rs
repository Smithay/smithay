// The caching logic is used to process surface synchronization. It creates
// an effective decoupling between the moment the client sends wl_surface.commit
// and the moment where the state that was commited is actually applied by the
// compositor.
//
// The way this is modelled in Smithay is through the `Cache` type, which is a container
// representing a cached state for a particular type. The full cached state of a surface
// is thus composed of a a set of `Cache<T>` for all relevant `T`, as modelled by the
// `MultiCache`.
//
// The logic of the `Cache` is as follows:
//
// - The protocol handlers mutably access the `pending` state to modify it accord to
//   the client requests
// - On commit, a snapshot of this pending state is created by invoking `Cacheable::commit`
//   and stored in the cache alongside an externally provided id
// - When the compositor decices that a given state (represented by its commit id) should
//   become active, `Cache::apply_state` is invoked with that commit id. The associated state
//   is then applied to the `current` state, that the compositor can then use as a reference
//   for the current window state. Note that, to preserve the commit ordering, all states
//   with a commit id older than the one requested are applied as well, in order.
//
// The logic for generating these commit ids and deciding when to apply them is implemented
// and described in `transaction.rs`.

use std::{
    cell::{RefCell, RefMut},
    collections::VecDeque,
};

use downcast_rs::{impl_downcast, Downcast};
use wayland_server::DisplayHandle;

use crate::wayland::Serial;

/// Trait representing a value that can be used in double-buffered storage
///
/// The type needs to implement the [`Default`] trait, which will be used
/// to initialize. You further need to provide two methods:
/// [`Cacheable::commit`] and [`Cacheable::merge_into`].
///
/// Double-buffered state works by having a "pending" instance of your type,
/// into which new values provided by the client are inserted. When the client
/// sends `wl_surface.commit`, the [`Cacheable::commit`] method will be
/// invoked on your value. This method is expected to produce a new instance of
/// your type, that will be stored in the cache, and eventually merged into the
/// current state.
///
/// In most cases, this method will simply produce a copy of the pending state,
/// but you might need additional logic in some cases, such as for handling
/// non-cloneable resources (which thus need to be moved into the produce value).
///
/// Then at some point the [`Cacheable::merge_into`] method of your type will be
/// invoked. In this method, `self` acts as the update that should be merged into
/// the current state provided as argument. In simple cases, the action would just
/// be to copy `self` into the current state, but more complex cases require
/// additional logic.
pub trait Cacheable: Default {
    /// Produce a new state to be cached from the pending state
    fn commit(&mut self, dh: &mut DisplayHandle<'_>) -> Self;
    /// Merge a state update into the current state
    fn merge_into(self, into: &mut Self, dh: &mut DisplayHandle<'_>);
}

struct CachedState<T> {
    pending: T,
    cache: VecDeque<(Serial, T)>,
    current: T,
}

impl<T: Default> Default for CachedState<T> {
    fn default() -> Self {
        CachedState {
            pending: T::default(),
            cache: VecDeque::new(),
            current: T::default(),
        }
    }
}

trait Cache: Downcast {
    fn commit(&self, commit_id: Option<Serial>, dh: &mut DisplayHandle<'_>);
    fn apply_state(&self, commit_id: Serial, dh: &mut DisplayHandle<'_>);
}

impl_downcast!(Cache);

impl<T: Cacheable + 'static> Cache for RefCell<CachedState<T>> {
    fn commit(&self, commit_id: Option<Serial>, dh: &mut DisplayHandle<'_>) {
        let mut guard = self.borrow_mut();
        let me = &mut *guard;
        let new_state = me.pending.commit(dh);
        if let Some(id) = commit_id {
            match me.cache.back_mut() {
                Some(&mut (cid, ref mut state)) if cid == id => new_state.merge_into(state, dh),
                _ => me.cache.push_back((id, new_state)),
            }
        } else {
            for (_, state) in me.cache.drain(..) {
                state.merge_into(&mut me.current, dh);
            }
            new_state.merge_into(&mut me.current, dh);
        }
    }

    fn apply_state(&self, commit_id: Serial, dh: &mut DisplayHandle<'_>) {
        let mut me = self.borrow_mut();
        loop {
            if me.cache.front().map(|&(s, _)| s > commit_id).unwrap_or(true) {
                // if the cache is empty or the next state has a commit_id greater than the requested one
                break;
            }
            me.cache.pop_front().unwrap().1.merge_into(&mut me.current, dh);
        }
    }
}

/// A typemap-like container for double-buffered values
///
/// All values inserted into this container must implement the [`Cacheable`] trait,
/// which defines their buffering semantics. They furthermore must be `Send` as the surface state
/// can be accessed from multiple threads (but `Sync` is not required, the surface internally synchronizes
/// access to its state).
///
/// Consumers of surface state (like compositor applications using Smithay) will mostly be concerned
/// with the [`MultiCache::current`] method, which gives access to the current state of the surface for
/// a particular type.
///
/// Writers of protocol extensions logic will mostly be concerned with the [`MultiCache::pending`] method,
/// which provides access to the pending state of the surface, in which new state from clients will be
/// stored.
///
/// This contained has [`RefCell`]-like semantics: values of multiple stored types can be accessed at the
/// same time. The stored values are initialized lazily the first time `current()` or `pending()` are
/// invoked with this type as argument.
pub struct MultiCache {
    caches: appendlist::AppendList<Box<dyn Cache + Send>>,
}

impl std::fmt::Debug for MultiCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiCache").finish_non_exhaustive()
    }
}

impl MultiCache {
    pub(crate) fn new() -> Self {
        Self {
            caches: appendlist::AppendList::new(),
        }
    }

    fn find_or_insert<T: Cacheable + Send + 'static>(&self) -> &RefCell<CachedState<T>> {
        for cache in &self.caches {
            if let Some(v) = (**cache).as_any().downcast_ref() {
                return v;
            }
        }
        // if we reach here, then the value is not yet in the list, insert it
        self.caches
            .push(Box::new(RefCell::new(CachedState::<T>::default())) as Box<_>);
        (*self.caches[self.caches.len() - 1])
            .as_any()
            .downcast_ref()
            .unwrap()
    }

    /// Access the pending state associated with type `T`
    pub fn pending<T: Cacheable + Send + 'static>(&self) -> RefMut<'_, T> {
        RefMut::map(self.find_or_insert::<T>().borrow_mut(), |cs| &mut cs.pending)
    }

    /// Access the current state associated with type `T`
    pub fn current<T: Cacheable + Send + 'static>(&self) -> RefMut<'_, T> {
        RefMut::map(self.find_or_insert::<T>().borrow_mut(), |cs| &mut cs.current)
    }

    /// Check if the container currently contains values for type `T`
    pub fn has<T: Cacheable + Send + 'static>(&self) -> bool {
        self.caches
            .iter()
            .any(|c| (**c).as_any().is::<RefCell<CachedState<T>>>())
    }

    /// Commits the pending state, invoking Cacheable::commit()
    ///
    /// If commit_id is None, then the pending state is directly merged
    /// into the current state. Otherwise, this id is used to store the
    /// cached state. An id ca no longer be re-used as soon as not new id
    /// has been used in between. Provided IDs are expected to be provided
    /// in increasing order according to `Serial` semantics.
    ///
    /// If a None commit is given but there are some cached states, they'll
    /// all be merged into the current state before merging the pending one.
    pub(crate) fn commit(&mut self, commit_id: Option<Serial>, dh: &mut DisplayHandle<'_>) {
        // none of the underlying borrow_mut() can panic, as we hold
        // a &mut reference to the container, non are borrowed.
        for cache in &self.caches {
            cache.commit(commit_id, dh);
        }
    }

    /// Apply given identified cached state to the current one
    ///
    /// All other preceding states are applied as well, to preserve commit ordering
    pub(crate) fn apply_state(&self, commit_id: Serial, dh: &mut DisplayHandle<'_>) {
        // none of the underlying borrow_mut() can panic, as we hold
        // a &mut reference to the container, non are borrowed.
        for cache in &self.caches {
            cache.apply_state(commit_id, dh);
        }
    }
}
