use crate::{
    input::{Seat, SeatHandler},
    utils::{DeadResource, IsAlive, Logical, Point, Serial},
    wayland::{
        compositor::{get_role, with_states},
        seat::WaylandFocus,
        shell::xdg::{PopupCachedState, XdgPopupSurfaceData, XdgPopupSurfaceRoleAttributes, XDG_POPUP_ROLE},
    },
};
use std::sync::{Arc, Mutex};
use tracing::trace;
use wayland_protocols::xdg::shell::server::xdg_wm_base;
use wayland_server::{protocol::wl_surface::WlSurface, Resource};

use super::{PopupGrab, PopupGrabError, PopupGrabInner, PopupKind};

/// Helper to track popups.
#[derive(Debug, Default)]
pub struct PopupManager {
    unmapped_popups: Vec<PopupKind>,
    popup_trees: Vec<PopupTree>,
    popup_grabs: Vec<PopupGrabInner>,
}

impl PopupManager {
    /// Start tracking a new popup.
    pub fn track_popup(&mut self, kind: PopupKind) -> Result<(), DeadResource> {
        if kind.parent().is_some() {
            self.add_popup(kind)
        } else {
            trace!("Adding unmapped popups: {:?}", kind);
            self.unmapped_popups.push(kind);
            Ok(())
        }
    }

    /// Needs to be called for [`PopupManager`] to correctly update its internal state.
    pub fn commit(&mut self, surface: &WlSurface) {
        if get_role(surface) == Some(XDG_POPUP_ROLE) {
            if let Some(i) = self
                .unmapped_popups
                .iter()
                .position(|p| p.wl_surface() == surface)
            {
                trace!("Popup got mapped");
                let popup = self.unmapped_popups.swap_remove(i);
                // at this point the popup must have a parent,
                // or it would have raised a protocol error
                let _ = self.add_popup(popup);
            }
        }
    }

    /// Take an explicit grab for the provided [`PopupKind`]
    ///
    /// Returns a [`PopupGrab`] on success or an [`PopupGrabError`]
    /// if the grab has been denied.
    pub fn grab_popup<D>(
        &mut self,
        root: <D as SeatHandler>::KeyboardFocus,
        popup: PopupKind,
        seat: &Seat<D>,
        serial: Serial,
    ) -> Result<PopupGrab<D>, PopupGrabError>
    where
        D: SeatHandler + 'static,
        <D as SeatHandler>::KeyboardFocus: WaylandFocus + From<PopupKind>,
        <D as SeatHandler>::PointerFocus: From<<D as SeatHandler>::KeyboardFocus> + WaylandFocus,
    {
        let surface = popup.wl_surface();
        assert_eq!(
            root.wl_surface().as_deref(),
            Some(&find_popup_root_surface(&popup)?)
        );

        match popup {
            PopupKind::Xdg(ref _xdg) => (),
            PopupKind::InputMethod(ref _input_method) => {
                return Err(PopupGrabError::InvalidGrab);
            }
        }

        // The primary store for the grab is the seat, additional we store it
        // in the popupmanager for active cleanup
        seat.user_data().insert_if_missing(PopupGrabInner::default);
        let toplevel_popups = seat.user_data().get::<PopupGrabInner>().unwrap().clone();

        // It the popup grab is not alive it is likely
        // that it either is new and have never been
        // added to the popupmanager or that it has been
        // cleaned up.
        if !toplevel_popups.has_any_grabs() {
            self.popup_grabs.push(toplevel_popups.clone());
        }

        let previous_serial = match toplevel_popups.grab(&popup, serial) {
            Ok(serial) => serial,
            Err(err) => {
                match err {
                    PopupGrabError::ParentDismissed => {
                        let _ = PopupManager::dismiss_popup(&root.wl_surface().unwrap(), &popup);
                    }
                    PopupGrabError::NotTheTopmostPopup => {
                        surface.post_error(
                            xdg_wm_base::Error::NotTheTopmostPopup,
                            "xdg_popup was not created on the topmost popup",
                        );
                    }
                    _ => {}
                }

                return Err(err);
            }
        };

        Ok(PopupGrab::new(
            toplevel_popups,
            root,
            serial,
            previous_serial,
            seat.get_keyboard(),
        ))
    }

    fn add_popup(&mut self, popup: PopupKind) -> Result<(), DeadResource> {
        let root = find_popup_root_surface(&popup)?;

        with_states(&root, |states| {
            let tree = PopupTree::default();
            if states.data_map.insert_if_missing_threadsafe(|| tree.clone()) {
                trace!("Adding popup {:?} to new PopupTree on root {:?}", popup, root);
                tree.insert(popup);

                tree.set_registered(true);
                self.popup_trees.push(tree);
            } else {
                let tree = states.data_map.get::<PopupTree>().unwrap();
                trace!(
                    "Adding popup {:?} to existing PopupTree on root {:?}",
                    popup,
                    root
                );
                tree.insert(popup);

                if tree.set_registered(true) {
                    self.popup_trees.push(tree.clone());
                }
            }
        });

        Ok(())
    }

    /// Finds the popup belonging to a given [`WlSurface`], if any.
    pub fn find_popup(&self, surface: &WlSurface) -> Option<PopupKind> {
        self.unmapped_popups
            .iter()
            .find(|p| p.wl_surface() == surface && p.alive())
            .cloned()
            .or_else(|| {
                self.popup_trees
                    .iter()
                    .flat_map(|tree| tree.iter_popups())
                    .find(|(p, _)| p.wl_surface() == surface)
                    .map(|(p, _)| p)
            })
    }

    /// Returns the popups and their relative positions for a given toplevel surface, if any.
    pub fn popups_for_surface(surface: &WlSurface) -> impl Iterator<Item = (PopupKind, Point<i32, Logical>)> {
        with_states(surface, |states| {
            states
                .data_map
                .get::<PopupTree>()
                .map(|x| x.iter_popups())
                .into_iter()
                .flatten()
        })
    }

    /// Dismiss the `popup` associated with the `surface.
    pub fn dismiss_popup(surface: &WlSurface, popup: &PopupKind) -> Result<(), DeadResource> {
        if !surface.alive() {
            return Err(DeadResource);
        }
        with_states(surface, |states| {
            let tree = states.data_map.get::<PopupTree>();

            if let Some(tree) = tree {
                tree.dismiss_popup(popup);
            }
        });
        Ok(())
    }

    /// Needs to be called periodically (but not necessarily frequently)
    /// to cleanup internal resources.
    pub fn cleanup(&mut self) {
        self.popup_grabs.retain_mut(|grabs| {
            grabs.cleanup();
            grabs.has_any_grabs()
        });
        self.popup_trees.retain_mut(|tree| {
            let alive = tree.cleanup_and_get_alive();
            if !alive {
                let _ = tree.set_registered(false);
            }
            alive
        });
        self.unmapped_popups.retain(|surf| surf.alive());
    }
}

/// Finds the toplevel wl_surface this popup belongs to.
///
/// Either because the parent of this popup is said toplevel
/// or because its parent popup belongs (indirectly) to said toplevel.
pub fn find_popup_root_surface(popup: &PopupKind) -> Result<WlSurface, DeadResource> {
    let mut parent = popup.parent().ok_or(DeadResource)?;
    while get_role(&parent) == Some(XDG_POPUP_ROLE) {
        parent = with_states(&parent, |states| {
            states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .parent
                .as_ref()
                .cloned()
        })
        .ok_or(DeadResource)?;
    }
    Ok(parent)
}

/// Computes this popup's location relative to its toplevel wl_surface's geometry.
///
/// This function will go up the parent stack and add up the relative locations. Useful for
/// transitive children popups.
pub fn get_popup_toplevel_coords(popup: &PopupKind) -> Point<i32, Logical> {
    let mut parent = match popup.parent() {
        Some(parent) => parent,
        None => return (0, 0).into(),
    };

    let mut offset = (0, 0).into();
    while get_role(&parent) == Some(XDG_POPUP_ROLE) {
        offset += with_states(&parent, |states| {
            states
                .cached_state
                .get::<PopupCachedState>()
                .current()
                .last_acked
                .map(|c| c.state.geometry.loc)
                .unwrap_or_default()
        });
        parent = with_states(&parent, |states| {
            states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .parent
                .as_ref()
                .cloned()
                .unwrap()
        });
    }

    offset
}

#[derive(Debug, Default, Clone)]
struct PopupTree(Arc<Mutex<PopupTreeInner>>);

#[derive(Debug, Default, Clone)]
struct PopupTreeInner {
    children: Vec<PopupNode>,
    registered: bool,
}

#[derive(Debug, Clone)]
struct PopupNode {
    surface: PopupKind,
    children: Vec<PopupNode>,
}

impl PopupTree {
    fn iter_popups(&self) -> impl Iterator<Item = (PopupKind, Point<i32, Logical>)> {
        self.0
            .lock()
            .unwrap()
            .children
            .iter()
            // A newly created xdg_popup is stacked on top of all previously created xdg_popups. We
            // push new ones to the end of the vector, so we should iterate in reverse, from newest
            // to oldest.
            .rev()
            .filter(|node| node.surface.alive())
            .flat_map(|n| n.iter_popups_relative_to((0, 0)).map(|(p, l)| (p.clone(), l)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn insert(&self, popup: PopupKind) {
        let tree = &mut *self.0.lock().unwrap();
        for child in tree.children.iter_mut() {
            if child.try_insert(&popup) {
                return;
            }
        }
        tree.children.push(PopupNode::new(popup));
    }

    fn dismiss_popup(&self, popup: &PopupKind) {
        let mut tree = self.0.lock().unwrap();

        let mut i = 0;
        while i < tree.children.len() {
            let child = &mut tree.children[i];

            if child.dismiss_popup(popup) {
                let _ = tree.children.remove(i);
                break;
            } else {
                i += 1;
            }
        }
    }

    fn cleanup_and_get_alive(&mut self) -> bool {
        let mut tree = self.0.lock().unwrap();
        tree.children
            .retain_mut(|child| child.cleanup_and_get_alive(false));
        !tree.children.is_empty()
    }

    /// Marks whether this tree is registered in the PopupManager's popup_trees
    ///
    /// Returns true if the registration status changed
    fn set_registered(&self, registered: bool) -> bool {
        let mut tree = self.0.lock().unwrap();
        let was_registered = tree.registered;
        tree.registered = registered;
        was_registered != registered
    }
}

impl PopupNode {
    fn new(surface: PopupKind) -> Self {
        PopupNode {
            surface,
            children: Vec::new(),
        }
    }

    fn iter_popups_relative_to<P: Into<Point<i32, Logical>>>(
        &self,
        loc: P,
    ) -> impl Iterator<Item = (&PopupKind, Point<i32, Logical>)> {
        let relative_to = loc.into() + self.surface.location();
        self.children
            .iter()
            // A newly created xdg_popup is stacked on top of all previously created xdg_popups. We
            // push new ones to the end of the vector, so we should iterate in reverse, from newest
            // to oldest.
            .rev()
            .filter(|node| node.surface.alive())
            .flat_map(move |x| {
                Box::new(x.iter_popups_relative_to(relative_to))
                    as Box<dyn Iterator<Item = (&PopupKind, Point<i32, Logical>)>>
            })
            .chain(std::iter::once((&self.surface, relative_to)))
    }

    fn try_insert(&mut self, popup: &PopupKind) -> bool {
        let parent = popup.parent().unwrap();
        if self.surface.wl_surface() == &parent {
            self.children.push(PopupNode::new(popup.clone()));
            true
        } else {
            for child in &mut self.children {
                if child.try_insert(popup) {
                    return true;
                }
            }
            false
        }
    }

    fn send_done(&self) {
        for child in self.children.iter().rev() {
            child.send_done();
        }

        self.surface.send_done();
    }

    fn dismiss_popup(&mut self, popup: &PopupKind) -> bool {
        if self.surface.wl_surface() == popup.wl_surface() {
            self.send_done();
            return true;
        }

        let mut i = 0;
        while i < self.children.len() {
            let child = &mut self.children[i];

            if child.dismiss_popup(popup) {
                let _ = self.children.remove(i);
                return false;
            } else {
                i += 1;
            }
        }

        false
    }

    fn cleanup_and_get_alive(&mut self, xdg_parent_destroyed: bool) -> bool {
        let alive = self.surface.alive();
        let mut xdg_destroyed = false;

        if let PopupKind::Xdg(_) = self.surface {
            if alive && xdg_parent_destroyed {
                self.surface.wl_surface().post_error(
                    xdg_wm_base::Error::NotTheTopmostPopup,
                    "xdg_popup was destroyed while it was not the topmost popup",
                );
            }
            xdg_destroyed = !alive;
        }

        self.children
            .retain_mut(|child| child.cleanup_and_get_alive(xdg_destroyed));

        alive
    }
}
