use crate::{
    input::{Seat, SeatHandler},
    utils::{DeadResource, IsAlive, Logical, Point, Serial},
    wayland::{
        compositor::{get_role, with_states},
        seat::WaylandFocus,
        shell::xdg::{XdgPopupSurfaceData, XdgPopupSurfaceRoleAttributes, XDG_POPUP_ROLE},
    },
};
use std::sync::{Arc, Mutex};
use tracing::trace;
use wayland_protocols::xdg::shell::server::{xdg_popup, xdg_wm_base};
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
        assert_eq!(root.wl_surface(), Some(find_popup_root_surface(&popup)?));

        match popup {
            PopupKind::Xdg(ref xdg) => {
                let surface = xdg.wl_surface();
                let committed = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgPopupSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .committed
                });

                if committed {
                    surface.post_error(xdg_popup::Error::InvalidGrab, "xdg_popup already is mapped");
                    return Err(PopupGrabError::InvalidGrab);
                }
            }
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
        if !toplevel_popups.active() {
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
            if states.data_map.insert_if_missing(|| tree.clone()) {
                self.popup_trees.push(tree);
            };
            let tree = states.data_map.get::<PopupTree>().unwrap();
            if !tree.alive() {
                // if it previously had no popups, we likely removed it from our list already
                self.popup_trees.push(tree.clone());
            }
            trace!("Adding popup {:?} to root {:?}", popup, root);
            tree.insert(popup);
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
        // retain_mut is sadly still unstable
        self.popup_grabs.iter_mut().for_each(|grabs| grabs.cleanup());
        self.popup_grabs.retain(|grabs| grabs.active());
        self.popup_trees.iter_mut().for_each(|tree| tree.cleanup());
        self.popup_trees.retain(|tree| tree.alive());
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
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .current
                .geometry
                .loc
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
struct PopupTree(Arc<Mutex<Vec<PopupNode>>>);

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
            .iter()
            .filter(|node| node.surface.alive())
            .flat_map(|n| n.iter_popups_relative_to((0, 0)).map(|(p, l)| (p.clone(), l)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn insert(&self, popup: PopupKind) {
        let children = &mut *self.0.lock().unwrap();
        for child in children.iter_mut() {
            if child.insert(popup.clone()) {
                return;
            }
        }
        children.push(PopupNode::new(popup));
    }

    fn dismiss_popup(&self, popup: &PopupKind) {
        let mut children = self.0.lock().unwrap();

        let mut i = 0;
        while i < children.len() {
            let child = &mut children[i];

            if child.dismiss_popup(popup) {
                let _ = children.remove(i);
                break;
            } else {
                i += 1;
            }
        }
    }

    fn cleanup(&mut self) {
        let mut children = self.0.lock().unwrap();
        for child in children.iter_mut() {
            child.cleanup();
        }
        children.retain(|n| n.surface.alive());
    }

    fn alive(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
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
            .filter(|node| node.surface.alive())
            .flat_map(move |x| {
                Box::new(x.iter_popups_relative_to(relative_to))
                    as Box<dyn Iterator<Item = (&PopupKind, Point<i32, Logical>)>>
            })
            .chain(std::iter::once((&self.surface, relative_to)))
    }

    fn insert(&mut self, popup: PopupKind) -> bool {
        let parent = popup.parent().unwrap();
        if self.surface.wl_surface() == &parent {
            self.children.push(PopupNode::new(popup));
            true
        } else {
            for child in &mut self.children {
                if child.insert(popup.clone()) {
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

    fn cleanup(&mut self) {
        for child in &mut self.children {
            child.cleanup();
        }

        if !self.surface.alive() && !self.children.is_empty() {
            // TODO: The client destroyed a popup before
            // destroying all children, this is a protocol
            // error. As the surface is no longer alive we
            // can not retrieve the client here to send
            // the error.
        }

        self.children.retain(|n| n.surface.alive());
    }
}
