mod grab;
mod manager;

pub use grab::*;
pub use manager::*;
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    utils::{IsAlive, Logical, Point, Rectangle},
    wayland::{
        compositor::with_states,
        input_method,
        shell::xdg::{self, SurfaceCachedState, XdgPopupSurfaceData},
    },
};

/// Represents a popup surface
#[derive(Debug, Clone, PartialEq)]
pub enum PopupKind {
    /// xdg-shell [`PopupSurface`](xdg::PopupSurface)
    Xdg(xdg::PopupSurface),
    /// input-method [`PopupSurface`](input_method::PopupSurface)
    InputMethod(input_method::PopupSurface),
}

impl IsAlive for PopupKind {
    fn alive(&self) -> bool {
        match self {
            PopupKind::Xdg(ref p) => p.alive(),
            PopupKind::InputMethod(ref p) => p.alive(),
        }
    }
}

impl From<PopupKind> for WlSurface {
    fn from(p: PopupKind) -> Self {
        p.wl_surface().clone()
    }
}

impl PopupKind {
    /// Retrieves the underlying [`WlSurface`]
    pub fn wl_surface(&self) -> &WlSurface {
        match *self {
            PopupKind::Xdg(ref t) => t.wl_surface(),
            PopupKind::InputMethod(ref t) => t.wl_surface(),
        }
    }

    fn parent(&self) -> Option<WlSurface> {
        match *self {
            PopupKind::Xdg(ref t) => t.get_parent_surface(),
            PopupKind::InputMethod(ref t) => t.get_parent().map(|parent| parent.surface.clone()),
        }
    }

    /// Returns the surface geometry as set by the client using `xdg_surface::set_window_geometry`
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let wl_surface = self.wl_surface();
        match *self {
            PopupKind::Xdg(_) => with_states(wl_surface, |states| {
                states
                    .cached_state
                    .current::<SurfaceCachedState>()
                    .geometry
                    .unwrap_or_default()
            }),
            PopupKind::InputMethod(ref t) => t.get_parent().map(|parent| parent.location).unwrap_or_default(),
        }
    }

    fn send_done(&self) {
        match *self {
            PopupKind::Xdg(ref t) => t.send_popup_done(),
            PopupKind::InputMethod(_) => {} //Nothing to do the IME takes care of this itself
        }
    }

    fn location(&self) -> Point<i32, Logical> {
        let wl_surface = self.wl_surface();

        match *self {
            PopupKind::Xdg(_) => {
                with_states(wl_surface, |states| {
                    states
                        .data_map
                        .get::<XdgPopupSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .current
                        .geometry
                })
                .loc
            }
            PopupKind::InputMethod(ref t) => t.location(),
        }
    }
}

impl From<xdg::PopupSurface> for PopupKind {
    fn from(p: xdg::PopupSurface) -> PopupKind {
        PopupKind::Xdg(p)
    }
}

impl From<input_method::PopupSurface> for PopupKind {
    fn from(p: input_method::PopupSurface) -> PopupKind {
        PopupKind::InputMethod(p)
    }
}
