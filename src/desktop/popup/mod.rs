mod grab;
mod manager;

use std::sync::Mutex;

pub use grab::*;
pub use manager::*;
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    input::{keyboard::KeyboardTarget, pointer::PointerTarget, SeatHandler},
    utils::{IsAlive, Logical, Point, Rectangle},
    wayland::{
        compositor::with_states,
        seat::WaylandFocus,
        shell::xdg::{PopupSurface, SurfaceCachedState, XdgPopupSurfaceRoleAttributes},
    },
};

/// Focused objects that *might* be an XdgPopup.
pub trait PopupFocus<D>: PointerTarget<D> + KeyboardTarget<D> + WaylandFocus
where
    D: SeatHandler<KeyboardFocus = Self, PointerFocus = Self> + 'static,
{
    /// Returns the underlying popup, if any
    fn xdg_popup(&self) -> Option<PopupKind>;
}

impl<D> PopupFocus<D> for WlSurface
where
    D: SeatHandler<KeyboardFocus = WlSurface, PointerFocus = WlSurface> + 'static,
{
    fn xdg_popup(&self) -> Option<PopupKind> {
        PopupManager::popups_for_surface(self).next().map(|(p, _)| p)
    }
}

/// Represents a popup surface
#[derive(Debug, Clone)]
pub enum PopupKind {
    /// xdg-shell [`PopupSurface`]
    Xdg(PopupSurface),
}

impl IsAlive for PopupKind {
    fn alive(&self) -> bool {
        match self {
            PopupKind::Xdg(ref p) => p.alive(),
        }
    }
}

impl PopupKind {
    /// Retrieves the underlying [`WlSurface`]
    pub fn wl_surface(&self) -> &WlSurface {
        match *self {
            PopupKind::Xdg(ref t) => t.wl_surface(),
        }
    }

    fn parent(&self) -> Option<WlSurface> {
        match *self {
            PopupKind::Xdg(ref t) => t.get_parent_surface(),
        }
    }

    /// Returns the surface geometry as set by the client using `xdg_surface::set_window_geometry`
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let wl_surface = self.wl_surface();

        with_states(wl_surface, |states| {
            states
                .cached_state
                .current::<SurfaceCachedState>()
                .geometry
                .unwrap_or_default()
        })
    }

    fn send_done(&self) {
        match *self {
            PopupKind::Xdg(ref t) => t.send_popup_done(),
        }
    }

    fn location(&self) -> Point<i32, Logical> {
        let wl_surface = self.wl_surface();

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
        .loc
    }
}

impl From<PopupSurface> for PopupKind {
    fn from(p: PopupSurface) -> PopupKind {
        PopupKind::Xdg(p)
    }
}
