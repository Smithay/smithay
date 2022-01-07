use crate::{
    utils::{DeadResource, Logical, Point, Rectangle},
    wayland::{
        compositor::{get_role, with_states},
        shell::xdg::{PopupSurface, SurfaceCachedState, XdgPopupSurfaceRoleAttributes, XDG_POPUP_ROLE},
    },
};
use std::sync::{Arc, Mutex};
use wayland_server::protocol::wl_surface::WlSurface;

/// Helper to track popups.
#[derive(Debug)]
pub struct PopupManager {
    unmapped_popups: Vec<PopupKind>,
    popup_trees: Vec<PopupTree>,
    logger: ::slog::Logger,
}

impl PopupManager {
    /// Create a new [`PopupManager`].
    pub fn new<L: Into<Option<::slog::Logger>>>(logger: L) -> Self {
        PopupManager {
            unmapped_popups: Vec::new(),
            popup_trees: Vec::new(),
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
                .position(|p| p.get_surface() == Some(surface))
            {
                slog::trace!(self.logger, "Popup got mapped");
                let popup = self.unmapped_popups.swap_remove(i);
                // at this point the popup must have a parent,
                // or it would have raised a protocol error
                let _ = self.add_popup(popup);
            }
        }
    }

    fn add_popup(&mut self, popup: PopupKind) -> Result<(), DeadResource> {
        let mut parent = popup.parent().unwrap();
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
            })?;
        }

        with_states(&parent, |states| {
            let tree = PopupTree::default();
            if states.data_map.insert_if_missing(|| tree.clone()) {
                self.popup_trees.push(tree);
            };
            let tree = states.data_map.get::<PopupTree>().unwrap();
            if !tree.alive() {
                // if it previously had no popups, we likely removed it from our list already
                self.popup_trees.push(tree.clone());
            }
            slog::trace!(self.logger, "Adding popup {:?} to parent {:?}", popup, parent);
            tree.insert(popup);
        })
    }

    /// Finds the popup belonging to a given [`WlSurface`], if any.
    pub fn find_popup(&self, surface: &WlSurface) -> Option<PopupKind> {
        self.unmapped_popups
            .iter()
            .find(|p| p.get_surface() == Some(surface))
            .cloned()
            .or_else(|| {
                self.popup_trees
                    .iter()
                    .map(|tree| tree.iter_popups())
                    .flatten()
                    .find(|(p, _)| p.get_surface() == Some(surface))
                    .map(|(p, _)| p)
            })
    }

    /// Returns the popups and their relative positions for a given toplevel surface, if any.
    pub fn popups_for_surface(
        surface: &WlSurface,
    ) -> Result<impl Iterator<Item = (PopupKind, Point<i32, Logical>)>, DeadResource> {
        with_states(surface, |states| {
            states
                .data_map
                .get::<PopupTree>()
                .map(|x| x.iter_popups())
                .into_iter()
                .flatten()
        })
    }

    /// Needs to be called periodically (but not necessarily frequently)
    /// to cleanup internal resources.
    pub fn cleanup(&mut self) {
        // retain_mut is sadly still unstable
        self.popup_trees.iter_mut().for_each(|tree| tree.cleanup());
        self.popup_trees.retain(|tree| tree.alive());
        self.unmapped_popups.retain(|surf| surf.alive());
    }
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
            .map(|n| n.iter_popups_relative_to((0, 0)).map(|(p, l)| (p.clone(), l)))
            .flatten()
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
        std::iter::once((&self.surface, relative_to)).chain(
            self.children
                .iter()
                .map(move |x| {
                    Box::new(x.iter_popups_relative_to(relative_to))
                        as Box<dyn Iterator<Item = (&PopupKind, Point<i32, Logical>)>>
                })
                .flatten(),
        )
    }

    fn insert(&mut self, popup: PopupKind) -> bool {
        let parent = popup.parent().unwrap();
        if self.surface.get_surface() == Some(&parent) {
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

    fn cleanup(&mut self) {
        for child in &mut self.children {
            child.cleanup();
        }
        self.children.retain(|n| n.surface.alive());
    }
}

/// Represents a popup surface
#[derive(Debug, Clone)]
pub enum PopupKind {
    /// xdg-shell [`PopupSurface`]
    Xdg(PopupSurface),
}

impl PopupKind {
    fn alive(&self) -> bool {
        match *self {
            PopupKind::Xdg(ref t) => t.alive(),
        }
    }

    /// Retrieves the underlying [`WlSurface`]
    pub fn get_surface(&self) -> Option<&WlSurface> {
        match *self {
            PopupKind::Xdg(ref t) => t.get_surface(),
        }
    }

    fn parent(&self) -> Option<WlSurface> {
        match *self {
            PopupKind::Xdg(ref t) => t.get_parent_surface(),
        }
    }

    /// Returns the surface geometry as set by the client using `xdg_surface::set_window_geometry`
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let wl_surface = match self.get_surface() {
            Some(s) => s,
            None => return Rectangle::from_loc_and_size((0, 0), (0, 0)),
        };

        with_states(wl_surface, |states| {
            states
                .cached_state
                .current::<SurfaceCachedState>()
                .geometry
                .unwrap_or_default()
        })
        .unwrap()
    }

    fn location(&self) -> Point<i32, Logical> {
        let wl_surface = match self.get_surface() {
            Some(s) => s,
            None => return (0, 0).into(),
        };
        with_states(wl_surface, |states| {
            states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .current
                .geometry
        })
        .unwrap_or_default()
        .loc
    }
}

impl From<PopupSurface> for PopupKind {
    fn from(p: PopupSurface) -> PopupKind {
        PopupKind::Xdg(p)
    }
}
