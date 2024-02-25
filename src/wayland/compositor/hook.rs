use std::sync::Arc;

crate::utils::ids::id_gen!(next_hooks_id, HOOK_ID, HOOKS_IDS);

/// Unique hook identifier used to unregister commit/descruction hooks
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct HookId(usize);

pub(super) struct Hook<T: ?Sized> {
    pub id: HookId,
    pub cb: Arc<T>,
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
            id: HookId(next_hooks_id()),
            cb,
        }
    }
}

impl<T: ?Sized> Drop for Hook<T> {
    fn drop(&mut self) {
        HOOKS_IDS.lock().unwrap().remove(&self.id.0);
    }
}
