use super::SurfaceAttributes;
use std::sync::Mutex;

use wayland_server::{Liveness, Resource};
use wayland_server::protocol::wl_surface;

/// Node of a subsurface tree, holding some user specified data type U
/// at each node
///
/// This type is internal to Smithay, and should not appear in the
/// public API
///
/// It is a bidirectionnal tree, meaning we can move along it in both
/// direction (top-bottom or bottom-up). We are taking advantage of the
/// fact that lifetime of objects are decided by wayland-server to ensure
/// the cleanup will be done properly, and we won't leak anything.
///
/// This implementation is not strictly a tree, but rather a directed graph
/// with the constraint that node can have at most one incoming edge. Aka like
/// a tree, but with loops allowed. This is because the wayland protocol does not
/// have a failure case to forbid this. Note that if any node in such a graph does not
/// have a parent, then the graph is a tree and this node is its root.
///
/// All the methods here are unsafe, because they assume the provided wl_surface object
/// is correctly initialized regarding its user_data.
pub struct SurfaceData<U> {
    parent: Option<wl_surface::WlSurface>,
    children: Vec<wl_surface::WlSurface>,
    has_role: bool,
    attributes: SurfaceAttributes<U>,
}

/// Status of a surface regarding its role
pub enum RoleStatus {
    /// This surface does not have any role
    NoRole,
    /// This surface is a subsurface
    Subsurface,
    /// This surface has a role other than subsurface
    ///
    /// It is thus the root of a subsurface tree that will
    /// have to be displayed
    HasRole,
}

pub enum Location {
    Before,
    After,
}

/// Possible actions to do after handling a node diring tree traversal
pub enum TraversalAction<T> {
    /// Traverse its children as well, providing them the data T
    DoChildren(T),
    /// Skip its children
    SkipChildren,
    /// Stop traversal completely
    Break,
}

impl<U: Default> SurfaceData<U> {
    fn new() -> SurfaceData<U> {
        SurfaceData {
            parent: None,
            children: Vec::new(),
            has_role: false,
            attributes: Default::default(),
        }
    }

    /// Initialize the user_data of a surface, must be called right when the surface is created
    pub unsafe fn init(surface: &wl_surface::WlSurface) {
        surface.set_user_data(Box::into_raw(Box::new(Mutex::new(SurfaceData::<U>::new()))) as *mut _)
    }
}

impl<U> SurfaceData<U> {
    unsafe fn get_data(surface: &wl_surface::WlSurface) -> &Mutex<SurfaceData<U>> {
        let ptr = surface.get_user_data();
        &*(ptr as *mut _)
    }

    /// Cleans the user_data of that surface, must be called when it is destroyed
    pub unsafe fn cleanup(surface: &wl_surface::WlSurface) {
        let ptr = surface.get_user_data();
        surface.set_user_data(::std::ptr::null_mut());
        let my_data_mutex: Box<Mutex<SurfaceData<U>>> = Box::from_raw(ptr as *mut _);
        let mut my_data = my_data_mutex.into_inner().unwrap();
        if let Some(old_parent) = my_data.parent.take() {
            if !old_parent.equals(surface) {
                // We had a parent that is not ourselves, lets unregister ourselves from it
                let old_parent_mutex = Self::get_data(&old_parent);
                let mut old_parent_guard = old_parent_mutex.lock().unwrap();
                old_parent_guard.children.retain(|c| !c.equals(surface));
            }
        }
        // orphan all our children
        for child in &my_data.children {
            // don't do anything if this child is ourselves
            if child.equals(surface) {
                continue;
            }
            let child_mutex = Self::get_data(child);
            let mut child_guard = child_mutex.lock().unwrap();
            child_guard.parent = None;
        }
    }

    /// Retrieve the current role status of this surface
    pub unsafe fn role_status(surface: &wl_surface::WlSurface) -> RoleStatus {
        debug_assert!(surface.status() == Liveness::Alive);
        let data_mutex = Self::get_data(surface);
        let data_guard = data_mutex.lock().unwrap();
        match (data_guard.has_role, data_guard.parent.is_some()) {
            (true, true) => RoleStatus::Subsurface,
            (true, false) => RoleStatus::HasRole,
            (false, false) => RoleStatus::NoRole,
            (false, true) => unreachable!(),
        }
    }

    /// Register that this surface has a role, fails if it already has one
    pub unsafe fn give_role(surface: &wl_surface::WlSurface) -> Result<(), ()> {
        debug_assert!(surface.status() == Liveness::Alive);
        let data_mutex = Self::get_data(surface);
        let mut data_guard = data_mutex.lock().unwrap();
        if data_guard.has_role {
            return Err(());
        }
        data_guard.has_role = true;
        Ok(())
    }

    /// Register that this surface has no role
    ///
    /// It is a noop if this surface already didn't have one, but fails if
    /// the role was "subsurface", it must be removed by the `unset_parent` method.
    pub unsafe fn remove_role(surface: &wl_surface::WlSurface) -> Result<(), ()> {
        debug_assert!(surface.status() == Liveness::Alive);
        let data_mutex = Self::get_data(surface);
        let mut data_guard = data_mutex.lock().unwrap();
        if data_guard.has_role && data_guard.parent.is_some() {
            return Err(());
        }
        data_guard.has_role = false;
        Ok(())
    }

    /// Sets the parent of a surface
    /// if this surface already has a role, does nothing and fails, otherwise
    /// its role is now to be a subsurface
    pub unsafe fn set_parent(child: &wl_surface::WlSurface, parent: &wl_surface::WlSurface)
                             -> Result<(), ()> {
        debug_assert!(child.status() == Liveness::Alive);
        debug_assert!(parent.status() == Liveness::Alive);

        // change child's parent
        {
            let child_mutex = Self::get_data(child);
            let mut child_guard = child_mutex.lock().unwrap();
            // if surface already has a role, it cannot be a subsurface
            if child_guard.has_role {
                return Err(());
            }
            debug_assert!(child_guard.parent.is_none());
            child_guard.parent = Some(parent.clone_unchecked());
            child_guard.has_role = true;
        }
        // register child to new parent
        // double scoping is to be robust to have a child be its own parent
        {
            let parent_mutex = Self::get_data(parent);
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard.children.push(child.clone_unchecked())
        }
        Ok(())
    }

    /// Remove a pre-existing parent of this child
    ///
    /// Does nothing if it has no parent
    pub unsafe fn unset_parent(child: &wl_surface::WlSurface) {
        debug_assert!(child.status() == Liveness::Alive);
        let old_parent = {
            let child_mutex = Self::get_data(child);
            let mut child_guard = child_mutex.lock().unwrap();
            let old_parent = child_guard.parent.take();
            if old_parent.is_some() {
                // We had a parent, so this does not have a role any more
                child_guard.has_role = false;
            }
            old_parent
        };
        // unregister from our parent
        if let Some(old_parent) = old_parent {
            let parent_mutex = Self::get_data(&old_parent);
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard.children.retain(|c| !c.equals(child));
        }
    }

    /// Retrieve the parent surface (if any) of this surface
    pub unsafe fn get_parent(child: &wl_surface::WlSurface) -> Option<wl_surface::WlSurface> {
        let child_mutex = Self::get_data(child);
        let child_guard = child_mutex.lock().unwrap();
        child_guard.parent.as_ref().map(|p| p.clone_unchecked())
    }

    /// Retrieve the parent surface (if any) of this surface
    pub unsafe fn get_children(child: &wl_surface::WlSurface) -> Vec<wl_surface::WlSurface> {
        let child_mutex = Self::get_data(child);
        let child_guard = child_mutex.lock().unwrap();
        child_guard
            .children
            .iter()
            .map(|p| p.clone_unchecked())
            .collect()
    }

    /// Reorders a surface relative to one of its sibling
    ///
    /// Fails if `relative_to` is not a sibling or parent of `surface`.
    pub unsafe fn reorder(surface: &wl_surface::WlSurface, to: Location,
                          relative_to: &wl_surface::WlSurface)
                          -> Result<(), ()> {
        let parent = {
            let data_mutex = Self::get_data(surface);
            let data_guard = data_mutex.lock().unwrap();
            data_guard
                .parent
                .as_ref()
                .map(|p| p.clone_unchecked())
                .unwrap()
        };
        if parent.equals(relative_to) {
            // TODO: handle positioning relative to parent
            return Ok(());
        }

        fn index_of(surface: &wl_surface::WlSurface, slice: &[wl_surface::WlSurface]) -> Option<usize> {
            for (i, s) in slice.iter().enumerate() {
                if s.equals(surface) {
                    return Some(i);
                }
            }
            None
        }

        let parent_mutex = Self::get_data(&parent);
        let mut parent_guard = parent_mutex.lock().unwrap();
        let my_index = index_of(surface, &parent_guard.children).unwrap();
        let mut other_index = match index_of(surface, &parent_guard.children) {
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

    /// Access the attributes associated with a surface
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time
    pub unsafe fn with_data<F>(surface: &wl_surface::WlSurface, f: F)
        where F: FnOnce(&mut SurfaceAttributes<U>)
    {
        let data_mutex = Self::get_data(surface);
        let mut data_guard = data_mutex.lock().unwrap();
        f(&mut data_guard.attributes)
    }

    /// Access sequentially the attributes associated with a surface tree,
    /// in a depth-first order
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time.
    ///
    /// The callback returns wether the traversal should continue or not. Returning
    /// false will cause an early-stopping.
    pub unsafe fn map_tree<F, T>(root: &wl_surface::WlSurface, initial: T, mut f: F)
        where F: FnMut(&wl_surface::WlSurface, &mut SurfaceAttributes<U>, &T) -> TraversalAction<T>
    {
        // helper function for recursion
        unsafe fn map<U, F, T>(surface: &wl_surface::WlSurface, root: &wl_surface::WlSurface, initial: &T,
                               f: &mut F)
                               -> bool
            where F: FnMut(&wl_surface::WlSurface, &mut SurfaceAttributes<U>, &T) -> TraversalAction<T>
        {
            // stop if we met the root, so to not deadlock/inifinte loop
            if surface.equals(root) {
                return true;
            }

            let data_mutex = SurfaceData::<U>::get_data(surface);
            let mut data_guard = data_mutex.lock().unwrap();
            // call the callback on ourselves
            match f(surface, &mut data_guard.attributes, initial) {
                TraversalAction::DoChildren(t) => {
                    // loop over children
                    for c in &data_guard.children {
                        if !map(c, root, &t, f) {
                            return false;
                        }
                    }
                    true
                }
                TraversalAction::SkipChildren => true,
                TraversalAction::Break => false,
            }
        }

        let data_mutex = Self::get_data(root);
        let mut data_guard = data_mutex.lock().unwrap();
        // call the callback on ourselves
        match f(root, &mut data_guard.attributes, &initial) {
            TraversalAction::DoChildren(t) => {
                // loop over children
                for c in &data_guard.children {
                    if !map::<U, _, _>(c, root, &t, &mut f) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
}
