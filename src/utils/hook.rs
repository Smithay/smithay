use std::sync::Arc;

crate::utils::ids::id_gen!(hooks_id);

/// Unique hook identifier used to unregister commit/descruction hooks
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct HookId(usize);

pub(crate) struct Hook<T: ?Sized> {
    pub id: HookId,
    pub cb: Arc<T>,
}

impl<T: ?Sized> std::fmt::Debug for Hook<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hook")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl<T: ?Sized> Clone for Hook<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            cb: self.cb.clone(),
        }
    }
}

impl<T: ?Sized> Hook<T> {
    pub fn new(cb: Arc<T>) -> Self {
        Self {
            id: HookId(hooks_id::next()),
            cb,
        }
    }
}

impl<T: ?Sized> Drop for Hook<T> {
    fn drop(&mut self) {
        hooks_id::remove(self.id.0);
    }
}
