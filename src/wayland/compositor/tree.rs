use crate::wayland::Serial;

use super::{
    cache::MultiCache,
    get_children,
    handlers::{is_effectively_sync, SurfaceUserData},
    transaction::PendingTransaction,
    SurfaceData,
};
use std::{
    fmt,
    sync::{atomic::Ordering, Mutex},
};
use wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, DisplayHandle, Resource};

pub(crate) static SUBSURFACE_ROLE: &str = "subsurface";

/// Node of a subsurface tree, holding some user specified data type U
/// at each node
///
/// This type is internal to Smithay, and should not appear in the
/// public API
///
/// It is a bidirectional tree, meaning we can move along it in both
/// direction (top-bottom or bottom-up). We are taking advantage of the
/// fact that lifetime of objects are decided by Wayland-server to ensure
/// the cleanup will be done properly, and we won't leak anything.
///
/// Each node also appears within its children list, to allow relative placement
/// between them.
pub struct PrivateSurfaceData {
    parent: Option<WlSurface>,
    children: Vec<WlSurface>,
    public_data: SurfaceData,
    pending_transaction: PendingTransaction,
    current_txid: Serial,
    commit_hooks: Vec<fn(&WlSurface)>,
}

impl fmt::Debug for PrivateSurfaceData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateSurfaceData")
            .field("parent", &self.parent)
            .field("children", &self.children)
            .field("public_data", &self.public_data)
            .field("pending_transaction", &"...")
            .field("current_txid", &self.current_txid)
            .field("commit_hooks", &"...")
            .field("commit_hooks.len", &self.commit_hooks.len())
            .finish()
    }
}

/// An error type signifying that the surface already has a role and
/// cannot be assigned an other
///
/// Generated if you attempt a role operation on a surface that does
/// not have the role you asked for.
#[derive(Debug)]
pub struct AlreadyHasRole;

impl std::fmt::Display for AlreadyHasRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Surface already has a role.")
    }
}

impl std::error::Error for AlreadyHasRole {}

pub enum Location {
    Before,
    After,
}

/// Possible actions to do after handling a node during tree traversal
#[derive(Debug)]
pub enum TraversalAction<T> {
    /// Traverse its children as well, providing them the data T
    DoChildren(T),
    /// Skip its children
    SkipChildren,
    /// Stop traversal completely
    Break,
}

impl PrivateSurfaceData {
    pub fn new() -> Mutex<PrivateSurfaceData> {
        Mutex::new(PrivateSurfaceData {
            parent: None,
            children: vec![],
            public_data: SurfaceData {
                role: Default::default(),
                data_map: Default::default(),
                cached_state: MultiCache::new(),
            },
            pending_transaction: Default::default(),
            current_txid: Serial(0),
            commit_hooks: Vec::new(),
        })
    }

    /// Initializes the surface, must be called at creation for state coherence
    pub fn init(surface: &WlSurface) {
        let my_data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
        let mut my_data = my_data_mutex.lock().unwrap();
        debug_assert!(my_data.children.is_empty());
        my_data.children.push(surface.clone());
    }

    /// Cleans the `as_ref().user_data` of that surface, must be called when it is destroyed
    pub fn cleanup(surface_data: &SurfaceUserData, surface_id: ObjectId) {
        let my_data_mutex = &surface_data.inner;
        let mut my_data = my_data_mutex.lock().unwrap();
        if let Some(old_parent) = my_data.parent.take() {
            // We had a parent, lets unregister ourselves from it
            let old_parent_mutex = &old_parent.data::<SurfaceUserData>().unwrap().inner;
            let mut old_parent_guard = old_parent_mutex.lock().unwrap();
            old_parent_guard.children.retain(|c| c.id() != surface_id);
        }
        // orphan all our children
        for child in my_data.children.drain(..) {
            let child_mutex = &child.data::<SurfaceUserData>().unwrap().inner;
            if std::ptr::eq(child_mutex, my_data_mutex) {
                // This child is ourselves, don't do anything.
                continue;
            }

            let mut child_guard = child_mutex.lock().unwrap();
            child_guard.parent = None;
        }
    }

    pub fn set_role(surface: &WlSurface, role: &'static str) -> Result<(), AlreadyHasRole> {
        let my_data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
        let mut my_data = my_data_mutex.lock().unwrap();
        if my_data.public_data.role.is_some() {
            return Err(AlreadyHasRole);
        }
        my_data.public_data.role = Some(role);
        Ok(())
    }

    pub fn get_role(surface: &WlSurface) -> Option<&'static str> {
        let my_data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
        let my_data = my_data_mutex.lock().unwrap();
        my_data.public_data.role
    }

    pub fn with_states<T, F: FnOnce(&SurfaceData) -> T>(surface: &WlSurface, f: F) -> T {
        let my_data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
        let my_data = my_data_mutex.lock().unwrap();
        f(&my_data.public_data)
    }

    pub fn add_commit_hook(surface: &WlSurface, hook: fn(&WlSurface)) {
        let my_data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
        let mut my_data = my_data_mutex.lock().unwrap();
        my_data.commit_hooks.push(hook);
    }

    pub fn invoke_commit_hooks(surface: &WlSurface) {
        // don't hold the mutex while the hooks are invoked
        let hooks = {
            let my_data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
            let my_data = my_data_mutex.lock().unwrap();
            my_data.commit_hooks.clone()
        };
        for hook in hooks {
            hook(surface);
        }
    }

    pub fn commit(surface: &WlSurface, dh: &mut DisplayHandle<'_>) {
        let is_sync = is_effectively_sync(surface);
        let children = get_children(dh, surface);
        let my_data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
        let mut my_data = my_data_mutex.lock().unwrap();
        // commit our state
        let current_txid = my_data.current_txid;
        my_data.public_data.cached_state.commit(Some(current_txid), dh);
        // take all our children state into our pending transaction
        for child in children {
            let child_data_mutex = &child.data::<SurfaceUserData>().unwrap().inner;
            // if the child is effectively sync, take its state
            // this is the case if either we are effectively sync, or the child is explicitly sync
            let mut child_data = child_data_mutex.lock().unwrap();
            let is_child_sync = || {
                child_data
                    .public_data
                    .data_map
                    .get::<super::handlers::SubsurfaceState>()
                    .map(|s| s.sync.load(Ordering::Acquire))
                    .unwrap_or(false)
            };
            if is_sync || is_child_sync() {
                let child_tx = std::mem::take(&mut child_data.pending_transaction);
                child_tx.merge_into(&my_data.pending_transaction);
                child_data.current_txid.0 = child_data.current_txid.0.wrapping_add(1);
            }
        }
        my_data
            .pending_transaction
            .insert_state(surface.clone(), my_data.current_txid);
        if !is_sync {
            // if we are not sync, apply the transaction immediately
            let tx = std::mem::take(&mut my_data.pending_transaction);
            // release the mutex, as applying the transaction will try to lock it
            std::mem::drop(my_data);
            // apply the transaction
            tx.finalize().apply(dh);
        }
    }

    /// Checks if the first surface is an ancestor of the second
    pub fn is_ancestor(a: &WlSurface, b: &WlSurface) -> bool {
        let b_mutex = &b.data::<SurfaceUserData>().unwrap().inner;
        let b_guard = b_mutex.lock().unwrap();
        if let Some(ref parent) = b_guard.parent {
            if parent.id() == a.id() {
                true
            } else {
                Self::is_ancestor(a, parent)
            }
        } else {
            false
        }
    }

    /// Sets the parent of a surface
    ///
    /// if this surface already has a role, does nothing and fails, otherwise
    /// its role is now to be a subsurface
    pub fn set_parent(child: &WlSurface, parent: &WlSurface) -> Result<(), AlreadyHasRole> {
        // debug_assert!(child.as_ref().is_alive());
        // debug_assert!(parent.as_ref().is_alive());

        // ensure the child is not already a parent of the parent
        if Self::is_ancestor(child, parent) {
            return Err(AlreadyHasRole);
        }

        // change child's parent
        {
            let child_mutex = &child.data::<SurfaceUserData>().unwrap().inner;
            let mut child_guard = child_mutex.lock().unwrap();
            // if surface already has a role, it cannot become a subsurface
            if child_guard.public_data.role.is_some() && child_guard.public_data.role != Some(SUBSURFACE_ROLE)
            {
                return Err(AlreadyHasRole);
            }
            child_guard.public_data.role = Some(SUBSURFACE_ROLE);
            debug_assert!(child_guard.parent.is_none());
            child_guard.parent = Some(parent.clone());
        }
        // register child to new parent
        {
            let parent_mutex = &parent.data::<SurfaceUserData>().unwrap().inner;
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard.children.push(child.clone())
        }

        Ok(())
    }

    /// Remove a pre-existing parent of this child
    ///
    /// Does nothing if it has no parent
    pub fn unset_parent(child: &WlSurface) {
        // debug_assert!(child.as_ref().is_alive());
        let old_parent = {
            let child_mutex = &child.data::<SurfaceUserData>().unwrap().inner;
            let mut child_guard = child_mutex.lock().unwrap();
            child_guard.parent.take()
        };
        // unregister from our parent
        if let Some(old_parent) = old_parent {
            let parent_mutex = &old_parent.data::<SurfaceUserData>().unwrap().inner;
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard.children.retain(|c| c.id() != child.id());
        }
    }

    /// Retrieve the parent surface (if any) of this surface
    pub fn get_parent(child: &WlSurface) -> Option<WlSurface> {
        let child_mutex = &child.data::<SurfaceUserData>().unwrap().inner;
        let child_guard = child_mutex.lock().unwrap();
        child_guard.parent.as_ref().cloned()
    }

    /// Retrieve the children surface (if any) of this surface
    pub fn get_children(parent: &WlSurface) -> Vec<WlSurface> {
        let parent_mutex = &parent.data::<SurfaceUserData>().unwrap().inner;
        let parent_guard = parent_mutex.lock().unwrap();
        parent_guard
            .children
            .iter()
            .filter(|s| s.id() != parent.id())
            .cloned()
            .collect()
    }

    /// Reorders a surface relative to one of its sibling
    ///
    /// Fails if `relative_to` is not a sibling or parent of `surface`.
    pub fn reorder(surface: &WlSurface, to: Location, relative_to: &WlSurface) -> Result<(), ()> {
        let parent = {
            let data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
            let data_guard = data_mutex.lock().unwrap();
            data_guard.parent.as_ref().cloned().unwrap()
        };

        fn index_of(surface: &WlSurface, slice: &[WlSurface]) -> Option<usize> {
            for (i, s) in slice.iter().enumerate() {
                if s.id() == surface.id() {
                    return Some(i);
                }
            }
            None
        }

        let parent_mutex = &parent.data::<SurfaceUserData>().unwrap().inner;
        let mut parent_guard = parent_mutex.lock().unwrap();
        let my_index = index_of(surface, &parent_guard.children).unwrap();
        let mut other_index = match index_of(relative_to, &parent_guard.children) {
            Some(idx) => idx,
            None => return Err(()),
        };
        let me = parent_guard.children.remove(my_index);
        if my_index < other_index {
            other_index -= 1;
        }
        let new_index = match to {
            Location::Before => other_index,
            Location::After => other_index + 1,
        };
        parent_guard.children.insert(new_index, me);

        Ok(())
    }
}

impl PrivateSurfaceData {
    /// Access sequentially the attributes associated with a surface tree,
    /// in a depth-first order.
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time.
    ///
    /// The first callback determines if this node children should be processed or not.
    ///
    /// The second actually does the processing, being called on children in display depth
    /// order.
    ///
    /// The third is called once all the children of a node has been processed (including itself), only if the first
    /// returned `DoChildren`, and gives an opportunity to early stop
    pub fn map_tree<F1, F2, F3, T>(
        surface: &WlSurface,
        initial: &T,
        mut filter: F1,
        mut processor: F2,
        mut post_filter: F3,
        reverse: bool,
    ) where
        F1: FnMut(&WlSurface, &SurfaceData, &T) -> TraversalAction<T>,
        F2: FnMut(&WlSurface, &SurfaceData, &T),
        F3: FnMut(&WlSurface, &SurfaceData, &T) -> bool,
    {
        Self::map(
            surface,
            initial,
            &mut filter,
            &mut processor,
            &mut post_filter,
            reverse,
        );
    }

    // helper function for map_tree
    fn map<F1, F2, F3, T>(
        surface: &WlSurface,
        initial: &T,
        filter: &mut F1,
        processor: &mut F2,
        post_filter: &mut F3,
        reverse: bool,
    ) -> bool
    where
        F1: FnMut(&WlSurface, &SurfaceData, &T) -> TraversalAction<T>,
        F2: FnMut(&WlSurface, &SurfaceData, &T),
        F3: FnMut(&WlSurface, &SurfaceData, &T) -> bool,
    {
        let data_mutex = &surface.data::<SurfaceUserData>().unwrap().inner;
        let mut data_guard = data_mutex.lock().unwrap();
        let data_guard = &mut *data_guard;
        // call the filter on ourselves
        match filter(surface, &data_guard.public_data, initial) {
            TraversalAction::DoChildren(t) => {
                // loop over children
                if reverse {
                    for c in data_guard.children.iter().rev() {
                        if c.id() == surface.id() {
                            processor(surface, &data_guard.public_data, initial);
                        } else if !Self::map(c, &t, filter, processor, post_filter, true) {
                            return false;
                        }
                    }
                } else {
                    for c in &data_guard.children {
                        if c.id() == surface.id() {
                            processor(surface, &data_guard.public_data, initial);
                        } else if !Self::map(c, &t, filter, processor, post_filter, false) {
                            return false;
                        }
                    }
                }
                post_filter(surface, &data_guard.public_data, initial)
            }
            TraversalAction::SkipChildren => {
                // still process ourselves
                processor(surface, &data_guard.public_data, initial);
                true
            }
            TraversalAction::Break => false,
        }
    }
}
