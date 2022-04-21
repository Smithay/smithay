use crate::{
    utils::{DeadResource, Logical, Point},
    wayland::{
        compositor::{get_role, with_states},
        seat::Seat,
        shell::xdg::{XdgPopupSurfaceRoleAttributes, XDG_POPUP_ROLE},
        Serial,
    },
};
use std::sync::{Arc, Mutex};
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle, Resource};

use super::{PopupGrab, PopupGrabError, PopupGrabInner, PopupKind};

/// Helper to track popups.
#[derive(Debug)]
pub struct PopupManager {
    unmapped_popups: Vec<PopupKind>,
    popup_trees: Vec<PopupTree>,
    popup_grabs: Vec<PopupGrabInner>,
    logger: ::slog::Logger,
}

impl PopupManager {
    /// Create a new [`PopupManager`].
    pub fn new<L: Into<Option<::slog::Logger>>>(logger: L) -> Self {
        PopupManager {
            unmapped_popups: Vec::new(),
            popup_trees: Vec::new(),
            popup_grabs: Vec::new(),
            logger: crate::slog_or_fallback(logger),
        }
    }

    /// Start tracking a new popup.
    pub fn track_popup(&mut self, kind: PopupKind) -> Result<(), DeadResource> {
        if kind.parent().is_some() {
            self.add_popup(kind)
        } else {
            slog::trace!(self.logger, "Adding unmapped popups: {:?}", kind);
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
                slog::trace!(self.logger, "Popup got mapped");
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
    pub fn grab_popup<D: 'static>(
        &mut self,
        dh: &mut DisplayHandle<'_>,
        popup: PopupKind,
        seat: &Seat<D>,
        serial: Serial,
    ) -> Result<PopupGrab, PopupGrabError> {
        let surface = popup.wl_surface();
        let root = find_popup_root_surface(&popup)?;

        match popup {
            PopupKind::Xdg(ref xdg) => {
                let surface = xdg.wl_surface();
                let committed = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .committed
                });

                if committed {
                    surface.post_error(
                        dh,
                        wayland_protocols::xdg_shell::server::xdg_popup::Error::InvalidGrab,
                        "xdg_popup already is mapped",
                    );
                    return Err(PopupGrabError::InvalidGrab);
                }
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
        if !toplevel_popups.alive() {
            self.popup_grabs.push(toplevel_popups.clone());
        }

        let previous_serial = match toplevel_popups.grab(&popup, serial) {
            Ok(serial) => serial,
            Err(err) => {
                match err {
                    PopupGrabError::ParentDismissed => {
                        let _ = PopupManager::dismiss_popup(dh, &root, &popup);
                    }
                    PopupGrabError::NotTheTopmostPopup => {
                        surface.post_error(
                            dh,
                            wayland_protocols::xdg_shell::server::xdg_wm_base::Error::NotTheTopmostPopup,
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
            slog::trace!(self.logger, "Adding popup {:?} to root {:?}", popup, root);
            tree.insert(popup);
        });

        Ok(())
    }

    /// Finds the popup belonging to a given [`WlSurface`], if any.
    pub fn find_popup(&self, surface: &WlSurface) -> Option<PopupKind> {
        self.unmapped_popups
            .iter()
            .find(|p| p.wl_surface() == surface)
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

    pub(crate) fn dismiss_popup(dh: &mut DisplayHandle<'_>, surface: &WlSurface, popup: &PopupKind) {
        with_states(surface, |states| {
            let tree = states.data_map.get::<PopupTree>();

            if let Some(tree) = tree {
                tree.dismiss_popup(dh, popup);
            }
        });
    }

    /// Needs to be called periodically (but not necessarily frequently)
    /// to cleanup internal resources.
    pub fn cleanup(&mut self) {
        // retain_mut is sadly still unstable
        self.popup_grabs.iter_mut().for_each(|grabs| grabs.cleanup());
        self.popup_grabs.retain(|grabs| grabs.alive());
        self.popup_trees.iter_mut().for_each(|tree| tree.cleanup());
        self.popup_trees.retain(|tree| tree.alive());
        // TODO(desktop-0.30)
        // self.unmapped_popups.retain(|surf| surf.alive());
    }
}

fn find_popup_root_surface(popup: &PopupKind) -> Result<WlSurface, DeadResource> {
    let mut parent = popup.parent().ok_or(DeadResource)?;
    while get_role(&parent) == Some(XDG_POPUP_ROLE) {
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
    Ok(parent)
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

    fn dismiss_popup(&self, dh: &mut DisplayHandle<'_>, popup: &PopupKind) {
        let mut children = self.0.lock().unwrap();

        let mut i = 0;
        while i < children.len() {
            let child = &mut children[i];

            if child.dismiss_popup(dh, popup) {
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
        // TODO(desktop-0.30)
        // children.retain(|n| n.surface.alive());
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
        std::iter::once((&self.surface, relative_to)).chain(self.children.iter().flat_map(move |x| {
            Box::new(x.iter_popups_relative_to(relative_to))
                as Box<dyn Iterator<Item = (&PopupKind, Point<i32, Logical>)>>
        }))
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

    fn send_done(&self, dh: &mut DisplayHandle<'_>) {
        for child in self.children.iter().rev() {
            child.send_done(dh);
        }

        self.surface.send_done(dh);
    }

    fn dismiss_popup(&mut self, dh: &mut DisplayHandle<'_>, popup: &PopupKind) -> bool {
        if self.surface.wl_surface() == popup.wl_surface() {
            self.send_done(dh);
            return true;
        }

        let mut i = 0;
        while i < self.children.len() {
            let child = &mut self.children[i];

            if child.dismiss_popup(dh, popup) {
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

        // TODO(desktop-0.30)
        // if !self.surface.alive() && !self.children.is_empty() {
        // TODO: The client destroyed a popup before
        // destroying all children, this is a protocol
        // error. As the surface is no longer alive we
        // can not retrieve the client here to send
        // the error.
        // }

        // self.children.retain(|n| n.surface.alive());
    }
}
