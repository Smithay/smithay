/// Common iterator types
use wayland_server::{backend::ClientId, Resource, Weak};

use std::{fmt, sync::MutexGuard};

/// Iterator helper over a mutex of client objects
pub struct LockedClientObjsIter<'a, T: 'static, G, F> {
    pub iterator: std::iter::FilterMap<std::slice::Iter<'static, Weak<T>>, F>,
    pub guard: MutexGuard<'a, G>,
}

pub(crate) fn new_locked_obj_iter_from_vec<T: Resource + 'static>(
    guard: MutexGuard<'_, Vec<Weak<T>>>,
    client: ClientId,
) -> impl Iterator<Item = T> + '_ {
    new_locked_obj_iter(guard, client, |guard| guard.iter())
}

pub(crate) fn new_locked_obj_iter<
    'a,
    T: Resource + 'static,
    G,
    F: for<'b> FnOnce(&'b G) -> std::slice::Iter<'b, Weak<T>>,
>(
    guard: MutexGuard<'a, G>,
    client: ClientId,
    iterator_fn: F,
) -> impl Iterator<Item = T> + 'a {
    let iterator = unsafe {
        std::mem::transmute::<std::slice::Iter<'_, Weak<T>>, std::slice::Iter<'static, Weak<T>>>(iterator_fn(
            &*guard,
        ))
    };

    let iterator = iterator.filter_map(move |p| {
        let client = &client;
        p.upgrade()
            .ok()
            .filter(|p| p.client().is_some_and(|c| c.id() == *client))
    });

    LockedClientObjsIter::<'a, T, G, _>::new_internal(iterator, guard)
}

impl<'a, T, G, F> LockedClientObjsIter<'a, T, G, F> {
    fn new_internal(
        iterator: std::iter::FilterMap<std::slice::Iter<'static, Weak<T>>, F>,
        guard: MutexGuard<'a, G>,
    ) -> Self {
        LockedClientObjsIter { iterator, guard }
    }
}

impl<T, G, F> Iterator for LockedClientObjsIter<'_, T, G, F>
where
    F: FnMut(&'static Weak<T>) -> Option<T>,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.iterator.next()
    }
}

impl<T, G: fmt::Debug, F> fmt::Debug for LockedClientObjsIter<'_, T, G, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LockedClientObjsIter")
            .field("inner", &self.guard)
            .finish()
    }
}
