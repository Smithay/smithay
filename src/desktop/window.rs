use crate::{
    backend::renderer::{utils::draw_surface_tree, ImportAll, Renderer},
    desktop::{utils::*, PopupManager, Space},
    utils::{user_data::UserDataMap, IsAlive, Logical, Physical, Point, Rectangle, Scale},
    wayland::{
        compositor::with_states,
        output::Output,
        shell::xdg::{SurfaceCachedState, ToplevelSurface},
    },
};
use std::{
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::protocol::wl_surface;

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

/// A transform that can be attached to a surface
/// to change how it will be presented on screen
/// Can be used to crop and scale on compositor side.
///
/// This applies to the whole surface tree and will
/// override all transforms on it's children.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowTransform {
    /// Defines an optional source [`Rectangle`] that can
    /// be used for cropping a window
    pub src: Option<Rectangle<i32, Logical>>,

    /// Defines an optional scale which will be used for
    /// the window
    pub scale: Scale<f64>,

    /// Defines an optional offset for the window
    pub offset: Point<i32, Logical>,
}

impl Default for WindowTransform {
    fn default() -> Self {
        Self {
            src: Default::default(),
            scale: Scale::from(1.0),
            offset: Default::default(),
        }
    }
}

#[derive(Debug)]
pub(super) struct WindowInner {
    pub(super) id: usize,
    toplevel: Kind,
    bbox: Mutex<Rectangle<i32, Logical>>,
    pub(super) transform: Mutex<WindowTransform>,
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
            transform: Mutex::new(WindowTransform::default()),
            user_data: UserDataMap::new(),
        }))
    }

    /// Returns the geometry of this window.
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geometry = self.real_geometry();
        let transform = *self.0.transform.lock().unwrap();
        if let Some(src) = transform.src {
            geometry = geometry.intersection(src).unwrap_or_default();
            let pos_rect = Rectangle::from_extemities(
                (0, 0),
                (i32::max(geometry.loc.x, 0), i32::max(geometry.loc.y, 0)),
            );
            geometry.loc -= src.loc.constrain(pos_rect);
        }

        let mut geometry = geometry.to_f64().upscale(transform.scale);
        geometry.loc -= transform.offset.to_f64();
        geometry.to_i32_round()
    }

    /// Returns a bounding box over this window and its children.
    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = *self.0.bbox.lock().unwrap();
        let transform = *self.0.transform.lock().unwrap();
        if let Some(src) = transform.src {
            bbox = bbox.intersection(src).unwrap_or_default();
            bbox.loc = Point::from((i32::min(bbox.loc.x, 0), i32::min(bbox.loc.y, 0)));
        }

        bbox.to_f64().upscale(transform.scale).to_i32_round()
    }

    /// Returns a bounding box over this window and children including popups.
    ///
    /// Note: You need to use a [`PopupManager`] to track popups, otherwise the bounding box
    /// will not include the popups.
    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        let mut bounding_box = self.bbox();
        let surface = self.0.toplevel.wl_surface();
        let transform = *self.0.transform.lock().unwrap();
        for (popup, location) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            let popup_offset = (location - popup.geometry().loc)
                .to_f64()
                .upscale(transform.scale)
                .to_i32_round();
            let offset = self.geometry().loc + popup_offset;
            bounding_box = bounding_box.merge(bbox_from_surface_tree(surface, offset, transform.scale, None));
        }

        bounding_box
    }

    /// Returns the [`Physical`] bounding box over this window, it subsurfaces as well as any popups.
    ///
    /// This differs from using [`bbox_with_popups`](Window::bbox_with_popups) and translating the returned [`Rectangle`]
    /// to [`Physical`] space as it rounds the subsurface and popup offsets.
    /// See [`physical_bbox_from_surface_tree`] for more information.
    ///
    /// Note: You need to use a [`PopupManager`] to track popups, otherwise the bounding box
    /// will not include the popups.
    pub fn physical_bbox_with_popups(
        &self,
        location: impl Into<Point<f64, Physical>>,
        scale: impl Into<Scale<f64>>,
    ) -> Rectangle<i32, Physical> {
        let location = location.into();
        let transform = *self.0.transform.lock().unwrap();
        let scale = scale.into() * transform.scale;
        let surface = self.0.toplevel.wl_surface();
        let mut geo = physical_bbox_from_surface_tree(surface, location, scale, transform.src);
        for (popup, p_location) in PopupManager::popups_for_surface(surface) {
            let offset = (self.geometry().loc + p_location - popup.geometry().loc)
                .to_f64()
                .to_physical(scale)
                .to_i32_round();
            geo = geo.merge(physical_bbox_from_surface_tree(
                surface,
                location + offset,
                scale,
                None,
            ));
        }
        geo
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
        *self.0.bbox.lock().unwrap() =
            bbox_from_surface_tree(self.0.toplevel.wl_surface(), (0, 0), 1.0, None);
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
        let transform = *self.0.transform.lock().unwrap();
        let surface = self.0.toplevel.wl_surface();
        if surface_type.contains(WindowSurfaceType::POPUP) {
            for (popup, location) in PopupManager::popups_for_surface(surface) {
                let popup_offset = (location - popup.geometry().loc)
                    .to_f64()
                    .upscale(transform.scale)
                    .to_i32_round();
                let offset = self.geometry().loc + popup_offset;
                if let Some(result) = under_from_surface_tree(
                    popup.wl_surface(),
                    point,
                    offset,
                    transform.scale,
                    None,
                    surface_type,
                ) {
                    return Some(result);
                }
            }
        }

        under_from_surface_tree(
            surface,
            point,
            (0, 0),
            transform.scale,
            transform.src,
            surface_type,
        )
    }

    /// Damage of all the surfaces of this window.
    ///
    /// If `for_values` is `Some(_)` it will only return the damage on the
    /// first call for a given [`Space`] and [`Output`], if the buffer hasn't changed.
    /// Subsequent calls will return an empty vector until the buffer is updated again.
    pub fn accumulated_damage(
        &self,
        location: impl Into<Point<f64, Physical>>,
        scale: impl Into<Scale<f64>>,
        for_values: Option<(&Space, &Output)>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let surface = self.0.toplevel.wl_surface();
        let transform = *self.0.transform.lock().unwrap();
        let scale = scale.into() * transform.scale;
        damage_from_surface_tree(surface, location, scale, transform.src, for_values)
    }

    /// Returns the opaque regions of this window
    pub fn opaque_regions(
        &self,
        location: impl Into<Point<f64, Physical>>,
        scale: impl Into<Scale<f64>>,
    ) -> Option<Vec<Rectangle<i32, Physical>>> {
        let surface = self.0.toplevel.wl_surface();
        let transform = *self.0.transform.lock().unwrap();
        let scale = scale.into() * transform.scale;
        opaque_regions_from_surface_tree(surface, location, scale, transform.src)
    }

    /// Returns the underlying toplevel
    pub fn toplevel(&self) -> &Kind {
        &self.0.toplevel
    }

    /// Returns a [`UserDataMap`] to allow associating arbitrary data with this window.
    pub fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }

    /// Returns the real geometry of this window without
    /// applying the crop and scale
    fn real_geometry(&self) -> Rectangle<i32, Logical> {
        let surface = self.0.toplevel.wl_surface();
        let bbox = *self.0.bbox.lock().unwrap();
        let geometry = with_states(surface, |states| {
            states.cached_state.current::<SurfaceCachedState>().geometry
        });

        if let Some(geometry) = geometry {
            // When applied, the effective window geometry will be the set window geometry clamped to the
            // bounding rectangle of the combined geometry of the surface of the xdg_surface and the associated subsurfaces.
            geometry.intersection(bbox).unwrap_or_default()
        } else {
            // If never set, the value is the full bounds of the surface, including any subsurfaces.
            // This updates dynamically on every commit. This unset is meant for extremely simple clients.
            bbox
        }
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
    let surface = window.toplevel().wl_surface();
    let transform = *window.0.transform.lock().unwrap();
    let scale = scale.into() * transform.scale;
    draw_surface_tree(
        renderer,
        frame,
        surface,
        scale,
        location,
        damage,
        transform.src,
        log,
    )
}

/// Renders popups of a given [`Window`] using a provided renderer and frame
///
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `location` is the position the window would be drawn at (popups will be drawn relative to that coordiante).
/// - `damage` is the set of regions of the layer surface that should be drawn.
///
/// Note: This function will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[allow(clippy::too_many_arguments)]
pub fn draw_window_popups<R, S, P>(
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
    let surface = window.toplevel().wl_surface();
    let transform = *window.0.transform.lock().unwrap();
    let scale = scale.into() * transform.scale;
    super::popup::draw_popups(
        renderer,
        frame,
        surface,
        location,
        window.geometry().loc,
        scale,
        damage,
        None,
        log,
    )
}
