use crate::{
    backend::renderer::{
        output::element::surface::WaylandSurfaceRenderElement, utils::draw_render_elements, ImportAll,
        Renderer,
    },
    desktop::{utils::*, PopupManager},
    output::Output,
    utils::{user_data::UserDataMap, IsAlive, Logical, Physical, Point, Rectangle, Scale},
    wayland::{
        compositor::with_states,
        shell::xdg::{SurfaceCachedState, ToplevelSurface},
    },
};
use std::{
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::protocol::wl_surface;

use super::space::SpaceElement;

crate::utils::ids::id_gen!(next_window_id, WINDOW_ID, WINDOW_IDS);

/// Abstraction around different toplevel kinds
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// xdg-shell [`ToplevelSurface`]
    Xdg(ToplevelSurface),
    /// XWayland surface (TODO)
    #[cfg(feature = "xwayland")]
    X11(X11Surface),
}

/// Xwayland surface
#[derive(Debug, Clone)]
#[cfg(feature = "xwayland")]
pub struct X11Surface {
    /// underlying wl_surface
    pub surface: wl_surface::WlSurface,
}

#[cfg(feature = "xwayland")]
impl std::cmp::PartialEq for X11Surface {
    fn eq(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.surface == other.surface
    }
}

#[cfg(feature = "xwayland")]
impl IsAlive for X11Surface {
    fn alive(&self) -> bool {
        self.surface.alive()
    }
}

#[cfg(feature = "xwayland")]
impl X11Surface {
    /// Returns the underlying [`WlSurface`](wl_surface::WlSurface), if still any.
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        &self.surface
    }
}

impl Kind {
    /// Returns the underlying [`WlSurface`](wl_surface::WlSurface), if still any.
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        match *self {
            Kind::Xdg(ref t) => t.wl_surface(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => t.wl_surface(),
        }
    }
}

impl IsAlive for Kind {
    fn alive(&self) -> bool {
        match self {
            Kind::Xdg(ref t) => t.alive(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => t.alive(),
        }
    }
}

#[derive(Debug)]
pub(super) struct WindowInner {
    pub(super) id: usize,
    toplevel: Kind,
    bbox: Mutex<Rectangle<i32, Logical>>,
    user_data: UserDataMap,
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        WINDOW_IDS.lock().unwrap().remove(&self.id);
    }
}

/// Represents a single application window
#[derive(Debug, Clone)]
pub struct Window(pub(super) Arc<WindowInner>);

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for Window {}

impl Hash for Window {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}

impl IsAlive for Window {
    fn alive(&self) -> bool {
        self.0.toplevel.alive()
    }
}

bitflags::bitflags! {
    /// Defines the surface types that can be
    /// queried with [`Window::surface_under`]
    pub struct WindowSurfaceType: u32 {
        /// Include the toplevel surface
        const TOPLEVEL = 1;
        /// Include all subsurfaces
        const SUBSURFACE = 2;
        /// Include all popup surfaces
        const POPUP = 4;
        /// Query all surfaces
        const ALL = Self::TOPLEVEL.bits | Self::SUBSURFACE.bits | Self::POPUP.bits;
    }
}

impl Window {
    /// Construct a new [`Window`] from a given compatible toplevel surface
    pub fn new(toplevel: Kind) -> Window {
        let id = next_window_id();

        Window(Arc::new(WindowInner {
            id,
            toplevel,
            bbox: Mutex::new(Rectangle::from_loc_and_size((0, 0), (0, 0))),
            user_data: UserDataMap::new(),
        }))
    }

    /// Returns the geometry of this window.
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let surface = self.0.toplevel.wl_surface();
        // It's the set geometry with the full bounding box as the fallback.
        with_states(surface, |states| {
            states.cached_state.current::<SurfaceCachedState>().geometry
        })
        .unwrap_or_else(|| self.bbox())
    }

    /// Returns a bounding box over this window and its children.
    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        *self.0.bbox.lock().unwrap()
    }

    /// Returns a bounding box over this window and children including popups.
    ///
    /// Note: You need to use a [`PopupManager`] to track popups, otherwise the bounding box
    /// will not include the popups.
    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        let mut bounding_box = self.bbox();
        let surface = self.0.toplevel.wl_surface();
        for (popup, location) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            let offset = self.geometry().loc + location - popup.geometry().loc;
            bounding_box = bounding_box.merge(bbox_from_surface_tree(surface, offset));
        }

        bounding_box
    }

    /// Activate/Deactivate this window
    pub fn set_activated(&self, active: bool) -> bool {
        match self.0.toplevel {
            Kind::Xdg(ref t) => t.with_pending_state(|state| {
                if active {
                    state.states.set(xdg_toplevel::State::Activated)
                } else {
                    state.states.unset(xdg_toplevel::State::Activated)
                }
            }),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref _t) => false,
        }
    }

    /// Commit any changes to this window
    pub fn configure(&self) {
        match self.0.toplevel {
            Kind::Xdg(ref t) => t.send_configure(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref _t) => unimplemented!(),
        }
    }

    /// Sends the frame callback to all the subsurfaces in this
    /// window that requested it
    pub fn send_frame(&self, time: u32) {
        let surface = self.0.toplevel.wl_surface();
        send_frames_surface_tree(surface, time);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            send_frames_surface_tree(surface, time);
        }
    }

    /// Updates internal values
    ///
    /// Needs to be called whenever the toplevel surface or any unsynchronized subsurfaces of this window are updated
    /// to correctly update the bounding box of this window.
    pub fn refresh(&self) {
        *self.0.bbox.lock().unwrap() = bbox_from_surface_tree(self.0.toplevel.wl_surface(), (0, 0));
    }

    /// Finds the topmost surface under this point matching the input regions of the surface and returns
    /// it together with the location of this surface.
    ///
    /// In case no surface input region matches the point [`None`] is returned.
    ///
    /// - `point` should be relative to (0,0) of the window.
    pub fn surface_under<P: Into<Point<f64, Logical>>>(
        &self,
        point: P,
        surface_type: WindowSurfaceType,
    ) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)> {
        let point = point.into();
        let surface = self.0.toplevel.wl_surface();
        if surface_type.contains(WindowSurfaceType::POPUP) {
            for (popup, location) in PopupManager::popups_for_surface(surface) {
                let offset = self.geometry().loc + location - popup.geometry().loc;
                if let Some(result) = under_from_surface_tree(popup.wl_surface(), point, offset, surface_type)
                {
                    return Some(result);
                }
            }
        }

        under_from_surface_tree(surface, point, (0, 0), surface_type)
    }

    /// Returns the underlying toplevel
    pub fn toplevel(&self) -> &Kind {
        &self.0.toplevel
    }

    /// Returns a [`UserDataMap`] to allow associating arbitrary data with this window.
    pub fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }
}

/// Renders a given [`Window`] using a provided renderer and frame.
///
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `location` is the position the window should be drawn at.
/// - `damage` is the set of regions of the window that should be drawn.
///
/// Note: This function will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[allow(clippy::too_many_arguments)]
pub fn draw_window<R, P, S>(
    renderer: &mut R,
    frame: &mut <R as Renderer>::Frame,
    window: &Window,
    scale: S,
    location: P,
    damage: &[Rectangle<i32, Physical>],
    log: &slog::Logger,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    S: Into<Scale<f64>>,
    P: Into<Point<f64, Physical>>,
{
    let location = location.into();
    let scale = scale.into();

    let elements = SpaceElement::<R, WaylandSurfaceRenderElement>::render_elements(
        window,
        location.to_i32_round(),
        scale,
    );

    draw_render_elements(renderer, frame, scale, &*elements, damage, log)?;

    Ok(())
}
