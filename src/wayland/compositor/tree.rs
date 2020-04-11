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
/// Each node also appears within its children list, to allow relative placement
/// between them.
pub struct SurfaceData<R> {
    parent: Option<WlSurface>,
    children: Vec<WlSurface>,
    role: R,
    attributes: SurfaceAttributes,
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

impl<R: Default> SurfaceData<R> {
    pub fn new() -> Mutex<SurfaceData<R>> {
        Mutex::new(SurfaceData {
            parent: None,
            children: vec![],
            role: Default::default(),
            attributes: Default::default(),
        })
    }
}

impl<R> SurfaceData<R>
where
    R: 'static,
{
    /// Initializes the surface, must be called at creation for state coherence
    pub fn init(surface: &WlSurface) {
        let my_data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
        let mut my_data = my_data_mutex.lock().unwrap();
        debug_assert!(my_data.children.is_empty());
        my_data.children.push(surface.clone());
    }

    /// Cleans the `as_ref().user_data` of that surface, must be called when it is destroyed
    pub fn cleanup(surface: &WlSurface) {
        let my_data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
        let mut my_data = my_data_mutex.lock().unwrap();
        if let Some(old_parent) = my_data.parent.take() {
            // We had a parent, lets unregister ourselves from it
            let old_parent_mutex = old_parent
                .as_ref()
                .user_data()
                .get::<Mutex<SurfaceData<R>>>()
                .unwrap();
            let mut old_parent_guard = old_parent_mutex.lock().unwrap();
            old_parent_guard
                .children
                .retain(|c| !c.as_ref().equals(surface.as_ref()));
        }
        // orphan all our children
        for child in &my_data.children {
            let child_mutex = child.as_ref().user_data().get::<Mutex<SurfaceData<R>>>().unwrap();
            if std::ptr::eq(child_mutex, my_data_mutex) {
                // This child is ourselves, don't do anything.
                continue;
            }

            let mut child_guard = child_mutex.lock().unwrap();
            child_guard.parent = None;
        }
    }
}

impl<R: RoleType + 'static> SurfaceData<R> {
    pub fn has_a_role(surface: &WlSurface) -> bool {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
        let data_guard = data_mutex.lock().unwrap();
        <R as RoleType>::has_role(&data_guard.role)
    }

    /// Check whether a surface has a given role
    pub fn has_role<RoleData>(surface: &WlSurface) -> bool
    where
        R: Role<RoleData>,
    {
        debug_assert!(surface.as_ref().is_alive());
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
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
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
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
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
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
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
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
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
        let mut data_guard = data_mutex.lock().unwrap();
        let data = <R as Role<RoleData>>::data_mut(&mut data_guard.role)?;
        Ok(f(data))
    }
}

impl<R: RoleType + Role<SubsurfaceRole> + 'static> SurfaceData<R> {
    /// Checks if the first surface is an ancestor of the second
    pub fn is_ancestor(a: &WlSurface, b: &WlSurface) -> bool {
        let b_mutex = b.as_ref().user_data().get::<Mutex<SurfaceData<R>>>().unwrap();
        let b_guard = b_mutex.lock().unwrap();
        if let Some(ref parent) = b_guard.parent {
            if parent.as_ref().equals(a.as_ref()) {
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
    pub fn set_parent(child: &WlSurface, parent: &WlSurface) -> Result<(), ()> {
        debug_assert!(child.as_ref().is_alive());
        debug_assert!(parent.as_ref().is_alive());
        // ensure the child is not already a parent of the parent
        if Self::is_ancestor(child, parent) {
            return Err(());
        }

        // change child's parent
        {
            let child_mutex = child.as_ref().user_data().get::<Mutex<SurfaceData<R>>>().unwrap();
            let mut child_guard = child_mutex.lock().unwrap();
            // if surface already has a role, it cannot become a subsurface
            <R as Role<SubsurfaceRole>>::set(&mut child_guard.role)?;
            debug_assert!(child_guard.parent.is_none());
            child_guard.parent = Some(parent.clone());
        }
        // register child to new parent
        {
            let parent_mutex = parent
                .as_ref()
                .user_data()
                .get::<Mutex<SurfaceData<R>>>()
                .unwrap();
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
            let child_mutex = child.as_ref().user_data().get::<Mutex<SurfaceData<R>>>().unwrap();
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
                .user_data()
                .get::<Mutex<SurfaceData<R>>>()
                .unwrap();
            let mut parent_guard = parent_mutex.lock().unwrap();
            parent_guard
                .children
                .retain(|c| !c.as_ref().equals(child.as_ref()));
        }
    }

    /// Retrieve the parent surface (if any) of this surface
    pub fn get_parent(child: &WlSurface) -> Option<WlSurface> {
        let child_mutex = child.as_ref().user_data().get::<Mutex<SurfaceData<R>>>().unwrap();
        let child_guard = child_mutex.lock().unwrap();
        child_guard.parent.as_ref().cloned()
    }

    /// Retrieve the children surface (if any) of this surface
    pub fn get_children(parent: &WlSurface) -> Vec<WlSurface> {
        let parent_mutex = parent
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
        let parent_guard = parent_mutex.lock().unwrap();
        parent_guard
            .children
            .iter()
            .filter(|s| !s.as_ref().equals(parent.as_ref()))
            .cloned()
            .collect()
    }

    /// Reorders a surface relative to one of its sibling
    ///
    /// Fails if `relative_to` is not a sibling or parent of `surface`.
    pub fn reorder(surface: &WlSurface, to: Location, relative_to: &WlSurface) -> Result<(), ()> {
        let parent = {
            let data_mutex = surface
                .as_ref()
                .user_data()
                .get::<Mutex<SurfaceData<R>>>()
                .unwrap();
            let data_guard = data_mutex.lock().unwrap();
            data_guard.parent.as_ref().cloned().unwrap()
        };

        fn index_of(surface: &WlSurface, slice: &[WlSurface]) -> Option<usize> {
            for (i, s) in slice.iter().enumerate() {
                if s.as_ref().equals(surface.as_ref()) {
                    return Some(i);
                }
            }
            None
        }

        let parent_mutex = parent
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
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

impl<R: 'static> SurfaceData<R> {
    /// Access the attributes associated with a surface
    ///
    /// Note that an internal lock is taken during access of this data,
    /// so the tree cannot be manipulated at the same time
    pub fn with_data<T, F>(surface: &WlSurface, f: F) -> T
    where
        F: FnOnce(&mut SurfaceAttributes) -> T,
    {
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
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
        F1: FnMut(&WlSurface, &mut SurfaceAttributes, &mut R, &T) -> TraversalAction<T>,
        F2: FnMut(&WlSurface, &mut SurfaceAttributes, &mut R, &T),
        F3: FnMut(&WlSurface, &mut SurfaceAttributes, &mut R, &T) -> bool,
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
        F1: FnMut(&WlSurface, &mut SurfaceAttributes, &mut R, &T) -> TraversalAction<T>,
        F2: FnMut(&WlSurface, &mut SurfaceAttributes, &mut R, &T),
        F3: FnMut(&WlSurface, &mut SurfaceAttributes, &mut R, &T) -> bool,
    {
        let data_mutex = surface
            .as_ref()
            .user_data()
            .get::<Mutex<SurfaceData<R>>>()
            .unwrap();
        let mut data_guard = data_mutex.lock().unwrap();
        let data_guard = &mut *data_guard;
        // call the filter on ourselves
        match filter(surface, &mut data_guard.attributes, &mut data_guard.role, initial) {
            TraversalAction::DoChildren(t) => {
                // loop over children
                if reverse {
                    for c in data_guard.children.iter().rev() {
                        if c.as_ref().equals(surface.as_ref()) {
                            processor(surface, &mut data_guard.attributes, &mut data_guard.role, initial);
                        } else if !Self::map(c, &t, filter, processor, post_filter, true) {
                            return false;
                        }
                    }
                } else {
                    for c in &data_guard.children {
                        if c.as_ref().equals(surface.as_ref()) {
                            processor(surface, &mut data_guard.attributes, &mut data_guard.role, initial);
                        } else if !Self::map(c, &t, filter, processor, post_filter, false) {
                            return false;
                        }
                    }
                }
                post_filter(surface, &mut data_guard.attributes, &mut data_guard.role, initial)
            }
            TraversalAction::SkipChildren => {
                // still process ourselves
                processor(surface, &mut data_guard.attributes, &mut data_guard.role, initial);
                true
            }
            TraversalAction::Break => false,
        }
    }
}
