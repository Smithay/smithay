use super::{SubsurfaceRole, SurfaceAttributes};
use super::roles::*;
use std::sync::Mutex;
use wayland_server::Resource;
use wayland_server::protocol::wl_surface::WlSurface;

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
/// All the methods here are unsafe, because they assume the provided `wl_surface` object
/// is correctly initialized regarding its `user_data`.
pub struct SurfaceData<U, R> {
    parent: Option<Resource<WlSurface>>,
    children: Vec<Resource<WlSurface>>,
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
    fn new() -> SurfaceData<U, R> {
        SurfaceData {
            parent: None,
            children: Vec::new(),
            role: Default::default(),
            attributes: Default::default(),
        }
    }

    /// Initialize the user_data of a surface, must be called right when the surface is created
    pub unsafe fn init(surface: &Resource<WlSurface>) {
        surface.set_user_data(Box::into_raw(Box::new(Mutex::new(SurfaceData::<U, R>::new()))) as *mut _)
    }
}

impl<U, R> SurfaceData<U, R>
where
    U: 'static,
    R: 'static,
{
    unsafe fn get_data(surface: &Resource<WlSurface>) -> &Mutex<SurfaceData<U, R>> {
        let ptr = surface.get_user_data();
        &*(ptr as *mut _)
    }

    /// Cleans the user_data of that surface, must be called when it is destroyed
    pub unsafe fn cleanup(surface: &Resource<WlSurface>) {
        let ptr = surface.get_user_data();
        surface.set_user_data(::std::ptr::null_mut());
        let my_data_mutex: Box<Mutex<SurfaceData<U, R>>> = Box::from_raw(ptr as *mut _);
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
}

impl<U: 'static, R: RoleType + 'static> SurfaceData<U, R> {
    pub unsafe fn has_a_role(surface: &Resource<WlSurface>) -> bool {
        debug_assert!(surface.is_alive());
        let data_mutex = Self::get_data(surface);
        let data_guard = data_mutex.lock().unwrap();
        <R as RoleType>::has_role(&data_guard.role)
    }

    /// Check wether a surface has a given role
    pub unsafe fn has_role<RoleData>(surface: &Resource<WlSurface>) -> bool
    where
        R: Role<RoleData>,
    {
        debug_assert!(surface.is_alive());
        let data_mutex = Self::get_data(surface);
        let data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::has(&data_guard.role)
    }

    /// Register that this surface has a role, fails if it already has one
    pub unsafe fn give_role<RoleData>(surface: &Resource<WlSurface>) -> Result<(), ()>
    where
        R: Role<RoleData>,
        RoleData: Default,
    {
        debug_assert!(surface.is_alive());
        let data_mutex = Self::get_data(surface);
        let mut data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::set(&mut data_guard.role)
    }

    /// Register that this surface has a role with given data
    ///
    /// Fails if it already has one and returns the data
    pub unsafe fn give_role_with<RoleData>(
        surface: &Resource<WlSurface>,
        data: RoleData,
    ) -> Result<(), RoleData>
    where
        R: Role<RoleData>,
    {
        debug_assert!(surface.is_alive());
        let data_mutex = Self::get_data(surface);
        let mut data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::set_with(&mut data_guard.role, data)
    }

    /// Register that this surface has no role and returns the data
    ///
    /// It is a noop if this surface already didn't have one, but fails if
    /// the role was "subsurface", it must be removed by the `unset_parent` method.
    pub unsafe fn remove_role<RoleData>(surface: &Resource<WlSurface>) -> Result<RoleData, WrongRole>
    where
        R: Role<RoleData>,
    {
        debug_assert!(surface.is_alive());
        let data_mutex = Self::get_data(surface);
        let mut data_guard = data_mutex.lock().unwrap();
        <R as Role<RoleData>>::unset(&mut data_guard.role)
    }

    /// Access to the role data
    pub unsafe fn with_role_data<RoleData, F, T>(surface: &Resource<WlSurface>, f: F) -> Result<T, WrongRole>
    where
        R: Role<RoleData>,
        F: FnOnce(&mut RoleData) -> T,
    {
        debug_assert!(surface.is_alive());
        let data_mutex = Self::get_data(surface);
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
    pub unsafe fn set_parent(child: &Resource<WlSurface>, parent: &Resource<WlSurface>) -> Result<(), ()> {
        debug_assert!(child.is_alive());
        debug_assert!(parent.is_alive());

        // change child's parent
        {
            let child_mutex = Self::get_data(child);
            let mut child_guard = child_mutex.lock().unwrap();
            // if surface already has a role, it cannot become a subsurface
            <R as Role<SubsurfaceRole>>::set(&mut child_guard.role)?;
            debug_assert!(child_guard.parent.is_none());
            child_guard.parent = Some(parent.clone());
        }
        // register child to new parent
        // double scoping is to be robust to have a child be its own parent
        {
            let parent_mutex = Self::get_data(parent);
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard.children.push(child.clone())
        }
        Ok(())
    }

    /// Remove a pre-existing parent of this child
    ///
    /// Does nothing if it has no parent
    pub unsafe fn unset_parent(child: &Resource<WlSurface>) {
        debug_assert!(child.is_alive());
        let old_parent = {
            let child_mutex = Self::get_data(child);
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
            let parent_mutex = Self::get_data(&old_parent);
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard.children.retain(|c| !c.equals(child));
        }
    }

    /// Retrieve the parent surface (if any) of this surface
    pub unsafe fn get_parent(child: &Resource<WlSurface>) -> Option<Resource<WlSurface>> {
        let child_mutex = Self::get_data(child);
        let child_guard = child_mutex.lock().unwrap();
        child_guard.parent.as_ref().cloned()
    }

    /// Retrieve the parent surface (if any) of this surface
    pub unsafe fn get_children(child: &Resource<WlSurface>) -> Vec<Resource<WlSurface>> {
        let child_mutex = Self::get_data(child);
        let child_guard = child_mutex.lock().unwrap();
        child_guard.children.to_vec()
    }

    /// Reorders a surface relative to one of its sibling
    ///
    /// Fails if `relative_to` is not a sibling or parent of `surface`.
    pub unsafe fn reorder(
        surface: &Resource<WlSurface>,
        to: Location,
        relative_to: &Resource<WlSurface>,
    ) -> Result<(), ()> {
        let parent = {
            let data_mutex = Self::get_data(surface);
            let data_guard = data_mutex.lock().unwrap();
            data_guard.parent.as_ref().cloned().unwrap()
        };
        if parent.equals(relative_to) {
            // TODO: handle positioning relative to parent
            return Ok(());
        }

        fn index_of(surface: &Resource<WlSurface>, slice: &[Resource<WlSurface>]) -> Option<usize> {
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
}

impl<U: 'static, R: 'static> SurfaceData<U, R> {
    /// Access the attributes associated with a surface
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time
    pub unsafe fn with_data<T, F>(surface: &Resource<WlSurface>, f: F) -> T
    where
        F: FnOnce(&mut SurfaceAttributes<U>) -> T,
    {
        let data_mutex = Self::get_data(surface);
        let mut data_guard = data_mutex.lock().unwrap();
        f(&mut data_guard.attributes)
    }

    /// Access sequentially the attributes associated with a surface tree,
    /// in a depth-first order.
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time.
    ///
    /// The callback returns wether the traversal should continue or not. Returning
    /// false will cause an early-stopping.
    pub unsafe fn map_tree<F, T>(root: &Resource<WlSurface>, initial: T, mut f: F, reverse: bool)
    where
        F: FnMut(&Resource<WlSurface>, &mut SurfaceAttributes<U>, &mut R, &T) -> TraversalAction<T>,
    {
        // helper function for recursion
        unsafe fn map<U: 'static, R: 'static, F, T>(
            surface: &Resource<WlSurface>,
            root: &Resource<WlSurface>,
            initial: &T,
            f: &mut F,
            reverse: bool,
        ) -> bool
        where
            F: FnMut(&Resource<WlSurface>, &mut SurfaceAttributes<U>, &mut R, &T) -> TraversalAction<T>,
        {
            // stop if we met the root, so to not deadlock/inifinte loop
            if surface.equals(root) {
                return true;
            }

            let data_mutex = SurfaceData::<U, R>::get_data(surface);
            let mut data_guard = data_mutex.lock().unwrap();
            let data_guard = &mut *data_guard;
            // call the callback on ourselves
            match f(
                surface,
                &mut data_guard.attributes,
                &mut data_guard.role,
                initial,
            ) {
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

        let data_mutex = Self::get_data(root);
        let mut data_guard = data_mutex.lock().unwrap();
        let data_guard = &mut *data_guard;
        // call the callback on ourselves
        if let TraversalAction::DoChildren(t) = f(
            root,
            &mut data_guard.attributes,
            &mut data_guard.role,
            &initial,
        ) {
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
