mod grab;
mod manager;

use std::sync::Mutex;

pub use grab::*;
pub use manager::*;
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    backend::renderer::{utils::draw_surface_tree, ImportAll, Renderer},
    input::{keyboard::KeyboardTarget, pointer::PointerTarget, SeatHandler},
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale},
    wayland::{
        compositor::with_states,
        seat::WaylandFocus,
        shell::xdg::{PopupSurface, SurfaceCachedState, XdgPopupSurfaceRoleAttributes},
    },
};

pub trait PopupFocus<D>: PointerTarget<D> + KeyboardTarget<D> + WaylandFocus
where
    D: SeatHandler<KeyboardFocus = Self, PointerFocus = Self> + 'static,
{
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

/// Renders popups of a given [`WlSurface`] using a provided renderer and frame.
///
/// - `surface_location` os the location the surface would been draw at
/// - `offset` will further offset the popups location (e.g. window popups should be offset by the windows geometry)
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `damage` is the set of regions of the layer surface that should be drawn.
///
/// Note: This function will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[allow(clippy::too_many_arguments)]
pub fn draw_popups<R, P1, P2, S>(
    renderer: &mut R,
    frame: &mut <R as Renderer>::Frame,
    for_surface: &WlSurface,
    surface_location: P1,
    offset: P2,
    scale: S,
    damage: &[Rectangle<i32, Physical>],
    log: &slog::Logger,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    P1: Into<Point<f64, Physical>>,
    P2: Into<Point<i32, Logical>>,
    S: Into<Scale<f64>>,
{
    let location = surface_location.into();
    let offset = offset.into();
    let scale = scale.into();
    for (popup, p_location) in PopupManager::popups_for_surface(for_surface) {
        let surface = popup.wl_surface();
        let offset = (offset + p_location - popup.geometry().loc)
            .to_f64()
            .to_physical(scale);
        draw_surface_tree(renderer, frame, surface, scale, location + offset, damage, log)?;
    }
    Ok(())
}
