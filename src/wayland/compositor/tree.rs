use super::{roles::*, SubsurfaceRole, SurfaceAttributes};
use std::sync::Mutex;
use wayland_server::protocol::wl_surface::WlSurface;

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
/// This implementation is not strictly a tree, but rather a directed graph
/// with the constraint that node can have at most one incoming edge. Aka like
/// a tree, but with loops allowed. This is because the Wayland protocol does not
/// have a failure case to forbid this. Note that if any node in such a graph does not
/// have a parent, then the graph is a tree and this node is its root.
pub struct SurfaceData<U, R> {
    parent: Option<WlSurface>,
    children: Vec<WlSurface>,
    role: R,
    attributes: SurfaceAttributes<U>,
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

impl<U: Default, R: Default> SurfaceData<U, R> {
    pub fn new() -> Mutex<SurfaceData<U, R>> {
        Mutex::new(SurfaceData {
            parent: None,
            children: Vec::new(),
            role: Default::default(),
            attributes: Default::default(),
        })
    }
}

impl<U, R> SurfaceData<U, R>
where
    U: 'static,
    R: 'static,
{
    /// Cleans the `as_ref().user_data` of that surface, must be called when it is destroyed
    pub fn cleanup(surface: &WlSurface) {
        let my_data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let mut my_data = my_data_mutex.lock().unwrap();
        if let Some(old_parent) = my_data.parent.take() {
            if !old_parent.as_ref().equals(surface.as_ref()) {
                // We had a parent that is not ourselves, lets unregister ourselves from it
                let old_parent_mutex = old_parent
                    .as_ref()
                    .user_data::<Mutex<SurfaceData<U, R>>>()
                    .unwrap();
                let mut old_parent_guard = old_parent_mutex.lock().unwrap();
                old_parent_guard
                    .children
                    .retain(|c| !c.as_ref().equals(surface.as_ref()));
            }
        }
        // orphan all our children
        for child in &my_data.children {
            // don't do anything if this child is ourselves
            if child.as_ref().equals(surface.as_ref()) {
                continue;
            }
            let child_mutex = child.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
            let mut child_guard = child_mutex.lock().unwrap();
            child_guard.parent = None;
        }
    }
}

impl<U: 'static, R: RoleType + 'static> SurfaceData<U, R> {
    pub fn has_a_role(surface: &WlSurface) -> bool {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let data_guard = data_mutex.lock().unwrap();
        <R as RoleType>::has_role(&data_guard.role)
    }

    /// Check whether a surface has a given role
    pub fn has_role<RoleData>(surface: &WlSurface) -> bool
    where
        R: Role<RoleData>,
    {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::has(&data_guard.role)
    }

    /// Register that this surface has a role, fails if it already has one
    pub fn give_role<RoleData>(surface: &WlSurface) -> Result<(), ()>
    where
        R: Role<RoleData>,
        RoleData: Default,
    {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let mut data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::set(&mut data_guard.role)
    }

    /// Register that this surface has a role with given data
    ///
    /// Fails if it already has one and returns the data
    pub fn give_role_with<RoleData>(surface: &WlSurface, data: RoleData) -> Result<(), RoleData>
    where
        R: Role<RoleData>,
    {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let mut data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::set_with(&mut data_guard.role, data)
    }

    /// Register that this surface has no role and returns the data
    ///
    /// It is a noop if this surface already didn't have one, but fails if
    /// the role was "subsurface", it must be removed by the `unset_parent` method.
    pub fn remove_role<RoleData>(surface: &WlSurface) -> Result<RoleData, WrongRole>
    where
        R: Role<RoleData>,
    {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let mut data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::unset(&mut data_guard.role)
    }

    /// Access to the role data
    pub fn with_role_data<RoleData, F, T>(surface: &WlSurface, f: F) -> Result<T, WrongRole>
    where
        R: Role<RoleData>,
        F: FnOnce(&mut RoleData) -> T,
    {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let mut data_guard = data_mutex.lock().unwrap();
        let data = <R as Role<RoleData>>::data_mut(&mut data_guard.role)?;
        Ok(f(data))
    }
}

impl<U: 'static, R: RoleType + Role<SubsurfaceRole> + 'static> SurfaceData<U, R> {
    /// Sets the parent of a surface
    ///
    /// if this surface already has a role, does nothing and fails, otherwise
    /// its role is now to be a subsurface
    pub fn set_parent(child: &WlSurface, parent: &WlSurface) -> Result<(), ()> {
        debug_assert!(child.as_ref().is_alive());
        debug_assert!(parent.as_ref().is_alive());

        // change child's parent
        {
            let child_mutex = child.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
            let mut child_guard = child_mutex.lock().unwrap();
            // if surface already has a role, it cannot become a subsurface
            <R as Role<SubsurfaceRole>>::set(&mut child_guard.role)?;
            debug_assert!(child_guard.parent.is_none());
            child_guard.parent = Some(parent.clone());
        }
        // register child to new parent
        // double scoping is to be robust to have a child be its own parent
        {
            let parent_mutex = parent.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard.children.push(child.clone())
        }
        Ok(())
    }

    /// Remove a pre-existing parent of this child
    ///
    /// Does nothing if it has no parent
    pub fn unset_parent(child: &WlSurface) {
        debug_assert!(child.as_ref().is_alive());
        let old_parent = {
            let child_mutex = child.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
            let mut child_guard = child_mutex.lock().unwrap();
            let old_parent = child_guard.parent.take();
            if old_parent.is_some() {
                // We had a parent, so this does not have a role any more
                <R as Role<SubsurfaceRole>>::unset(&mut child_guard.role)
                    .expect("Surface had a parent but not the subsurface role?!");
            }
            old_parent
        };
        // unregister from our parent
        if let Some(old_parent) = old_parent {
            let parent_mutex = old_parent
                .as_ref()
                .user_data::<Mutex<SurfaceData<U, R>>>()
                .unwrap();
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard
                .children
                .retain(|c| !c.as_ref().equals(child.as_ref()));
        }
    }

    /// Retrieve the parent surface (if any) of this surface
    pub fn get_parent(child: &WlSurface) -> Option<WlSurface> {
        let child_mutex = child.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let child_guard = child_mutex.lock().unwrap();
        child_guard.parent.as_ref().cloned()
    }

    /// Retrieve the parent surface (if any) of this surface
    pub fn get_children(child: &WlSurface) -> Vec<WlSurface> {
        let child_mutex = child.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let child_guard = child_mutex.lock().unwrap();
        child_guard.children.to_vec()
    }

    /// Reorders a surface relative to one of its sibling
    ///
    /// Fails if `relative_to` is not a sibling or parent of `surface`.
    pub fn reorder(surface: &WlSurface, to: Location, relative_to: &WlSurface) -> Result<(), ()> {
        let parent = {
            let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
            let data_guard = data_mutex.lock().unwrap();
            data_guard.parent.as_ref().cloned().unwrap()
        };
        if parent.as_ref().equals(relative_to.as_ref()) {
            // TODO: handle positioning relative to parent
            return Ok(());
        }

        fn index_of(surface: &WlSurface, slice: &[WlSurface]) -> Option<usize> {
            for (i, s) in slice.iter().enumerate() {
                if s.as_ref().equals(surface.as_ref()) {
                    return Some(i);
                }
            }
            None
        }

        let parent_mutex = parent.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
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
}

impl<U: 'static, R: 'static> SurfaceData<U, R> {
    /// Access the attributes associated with a surface
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time
    pub fn with_data<T, F>(surface: &WlSurface, f: F) -> T
    where
        F: FnOnce(&mut SurfaceAttributes<U>) -> T,
    {
        let data_mutex = surface
            .as_ref()
            .user_data::<Mutex<SurfaceData<U, R>>>()
            .expect("Accessing the data of foreign surfaces is not supported.");
        let mut data_guard = data_mutex.lock().unwrap();
        f(&mut data_guard.attributes)
    }

    /// Access sequentially the attributes associated with a surface tree,
    /// in a depth-first order.
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time.
    ///
    /// The callback returns whether the traversal should continue or not. Returning
    /// false will cause an early-stopping.
    pub fn map_tree<F, T>(root: &WlSurface, initial: T, mut f: F, reverse: bool)
    where
        F: FnMut(&WlSurface, &mut SurfaceAttributes<U>, &mut R, &T) -> TraversalAction<T>,
    {
        // helper function for recursion
        fn map<U: 'static, R: 'static, F, T>(
            surface: &WlSurface,
            root: &WlSurface,
            initial: &T,
            f: &mut F,
            reverse: bool,
        ) -> bool
        where
            F: FnMut(&WlSurface, &mut SurfaceAttributes<U>, &mut R, &T) -> TraversalAction<T>,
        {
            // stop if we met the root, so to not deadlock/inifinte loop
            if surface.as_ref().equals(root.as_ref()) {
                return true;
            }

            let data_mutex = surface.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
            let mut data_guard = data_mutex.lock().unwrap();
            let data_guard = &mut *data_guard;
            // call the callback on ourselves
            match f(surface, &mut data_guard.attributes, &mut data_guard.role, initial) {
                TraversalAction::DoChildren(t) => {
                    // loop over children
                    if reverse {
                        for c in data_guard.children.iter().rev() {
                            if !map::<U, R, _, _>(c, root, &t, f, true) {
                                return false;
                            }
                        }
                    } else {
                        for c in &data_guard.children {
                            if !map::<U, R, _, _>(c, root, &t, f, false) {
                                return false;
                            }
                        }
                    }
                    true
                }
                TraversalAction::SkipChildren => true,
                TraversalAction::Break => false,
            }
        }

        let data_mutex = root.as_ref().user_data::<Mutex<SurfaceData<U, R>>>().unwrap();
        let mut data_guard = data_mutex.lock().unwrap();
        let data_guard = &mut *data_guard;
        // call the callback on ourselves
        if let TraversalAction::DoChildren(t) =
            f(root, &mut data_guard.attributes, &mut data_guard.role, &initial)
        {
            // loop over children
            if reverse {
                for c in data_guard.children.iter().rev() {
                    if !map::<U, R, _, _>(c, root, &t, &mut f, true) {
                        break;
                    }
                }
            } else {
                for c in &data_guard.children {
                    if !map::<U, R, _, _>(c, root, &t, &mut f, false) {
                        break;
                    }
                }
            }
        }
    }
}
