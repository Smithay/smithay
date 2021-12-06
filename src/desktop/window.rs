use crate::{
    backend::renderer::{
        utils::{draw_surface_tree, SurfaceState},
        Frame, ImportAll, Renderer, Texture,
    },
    desktop::PopupManager,
    utils::{Logical, Point, Rectangle, Size},
    wayland::{
        compositor::{
            with_states, with_surface_tree_downward, with_surface_tree_upward, Damage, SubsurfaceCachedState,
            SurfaceAttributes, TraversalAction,
        },
        shell::xdg::{SurfaceCachedState, ToplevelSurface},
    },
};
use std::{
    cell::RefCell,
    collections::HashSet,
    hash::{Hash, Hasher},
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
use wayland_commons::user_data::UserDataMap;
use wayland_protocols::xdg_shell::server::xdg_toplevel;
use wayland_server::protocol::{wl_buffer, wl_surface};

static WINDOW_ID: AtomicUsize = AtomicUsize::new(0);
lazy_static::lazy_static! {
    static ref WINDOW_IDS: Mutex<HashSet<usize>> = Mutex::new(HashSet::new());
}

fn next_window_id() -> usize {
    let mut ids = WINDOW_IDS.lock().unwrap();
    if ids.len() == usize::MAX {
        // Theoretically the code below wraps around correctly,
        // but that is hard to detect and might deadlock.
        // Maybe make this a debug_assert instead?
        panic!("Out of window ids");
    }

    let mut id = WINDOW_ID.fetch_add(1, Ordering::SeqCst);
    while ids.iter().any(|k| *k == id) {
        id = WINDOW_ID.fetch_add(1, Ordering::SeqCst);
    }

    ids.insert(id);
    id
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Xdg(ToplevelSurface),
    #[cfg(feature = "xwayland")]
    X11(X11Surface),
}

// Big TODO
#[derive(Debug, Clone)]
pub struct X11Surface {
    surface: wl_surface::WlSurface,
}

impl std::cmp::PartialEq for X11Surface {
    fn eq(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.surface == other.surface
    }
}

impl X11Surface {
    pub fn alive(&self) -> bool {
        self.surface.as_ref().is_alive()
    }

    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        if self.alive() {
            Some(&self.surface)
        } else {
            None
        }
    }
}

impl Kind {
    pub fn alive(&self) -> bool {
        match *self {
            Kind::Xdg(ref t) => t.alive(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => t.alive(),
        }
    }

    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        match *self {
            Kind::Xdg(ref t) => t.get_surface(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => t.get_surface(),
        }
    }
}

impl SurfaceState {
    /// Returns the size of the surface.
    pub fn size(&self) -> Option<Size<i32, Logical>> {
        self.buffer_dimensions
            .map(|dims| dims.to_logical(self.buffer_scale))
    }

    fn contains_point(&self, attrs: &SurfaceAttributes, point: Point<f64, Logical>) -> bool {
        let size = match self.size() {
            None => return false, // If the surface has no size, it can't have an input region.
            Some(size) => size,
        };

        let rect = Rectangle {
            loc: (0, 0).into(),
            size,
        }
        .to_f64();

        // The input region is always within the surface itself, so if the surface itself doesn't contain the
        // point we can return false.
        if !rect.contains(point) {
            return false;
        }

        // If there's no input region, we're done.
        if attrs.input_region.is_none() {
            return true;
        }

        attrs
            .input_region
            .as_ref()
            .unwrap()
            .contains(point.to_i32_floor())
    }
}

#[derive(Debug)]
pub(super) struct WindowInner {
    pub(super) id: usize,
    toplevel: Kind,
    user_data: UserDataMap,
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        WINDOW_IDS.lock().unwrap().remove(&self.id);
    }
}

#[derive(Debug, Clone)]
pub struct Window(pub(super) Rc<WindowInner>);

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

impl Window {
    pub fn new(toplevel: Kind) -> Window {
        let id = next_window_id();

        // TODO: Do we want this? For new lets add Window::commit
        //add_commit_hook(toplevel.get_surface().unwrap(), surface_commit);

        Window(Rc::new(WindowInner {
            id,
            toplevel,
            user_data: UserDataMap::new(),
        }))
    }

    /// Returns the geometry of this window.
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        // It's the set geometry with the full bounding box as the fallback.
        with_states(self.0.toplevel.get_surface().unwrap(), |states| {
            states.cached_state.current::<SurfaceCachedState>().geometry
        })
        .unwrap()
        .unwrap_or_else(|| self.bbox())
    }

    /// A bounding box over this window and its children.
    // TODO: Cache and document when to trigger updates. If possible let space do it
    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        if let Some(surface) = self.0.toplevel.get_surface() {
            bbox_from_surface_tree(surface, (0, 0))
        } else {
            Rectangle::from_loc_and_size((0, 0), (0, 0))
        }
    }

    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        let mut bounding_box = self.bbox();
        if let Some(surface) = self.0.toplevel.get_surface() {
            for (popup, location) in PopupManager::popups_for_surface(surface)
                .ok()
                .into_iter()
                .flatten()
            {
                if let Some(surface) = popup.get_surface() {
                    bounding_box = bounding_box.merge(bbox_from_surface_tree(surface, location));
                }
            }
        }
        bounding_box
    }

    /// Activate/Deactivate this window
    // TODO: Add more helpers for Maximize? Minimize? Fullscreen? I dunno
    pub fn set_activated(&self, active: bool) -> bool {
        match self.0.toplevel {
            Kind::Xdg(ref t) => t
                .with_pending_state(|state| {
                    if active {
                        state.states.set(xdg_toplevel::State::Activated)
                    } else {
                        state.states.unset(xdg_toplevel::State::Activated)
                    }
                })
                .unwrap_or(false),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => unimplemented!(),
        }
    }

    /// Commit any changes to this window
    pub fn configure(&self) {
        match self.0.toplevel {
            Kind::Xdg(ref t) => t.send_configure(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => unimplemented!(),
        }
    }

    /// Sends the frame callback to all the subsurfaces in this
    /// window that requested it
    pub fn send_frame(&self, time: u32) {
        if let Some(surface) = self.0.toplevel.get_surface() {
            send_frames_surface_tree(surface, time);
            for (popup, _) in PopupManager::popups_for_surface(surface)
                .ok()
                .into_iter()
                .flatten()
            {
                if let Some(surface) = popup.get_surface() {
                    send_frames_surface_tree(surface, time);
                }
            }
        }
    }

    /// Finds the topmost surface under this point if any and returns it together with the location of this
    /// surface.
    pub fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)> {
        if let Some(surface) = self.0.toplevel.get_surface() {
            for (popup, location) in PopupManager::popups_for_surface(surface)
                .ok()
                .into_iter()
                .flatten()
            {
                if let Some(result) = popup
                    .get_surface()
                    .and_then(|surface| under_from_surface_tree(surface, point, location))
                {
                    return Some(result);
                }
            }

            under_from_surface_tree(surface, point, (0, 0))
        } else {
            None
        }
    }

    /// Damage of all the surfaces of this window
    pub(super) fn accumulated_damage(&self) -> Vec<Rectangle<i32, Logical>> {
        let mut damage = Vec::new();
        if let Some(surface) = self.0.toplevel.get_surface() {
            damage.extend(damage_from_surface_tree(surface, (0, 0)));
            for (popup, location) in PopupManager::popups_for_surface(surface)
                .ok()
                .into_iter()
                .flatten()
            {
                if let Some(surface) = popup.get_surface() {
                    let popup_damage = damage_from_surface_tree(surface, location);
                    damage.extend(popup_damage);
                }
            }
        }
        damage
    }

    pub fn toplevel(&self) -> &Kind {
        &self.0.toplevel
    }

    pub fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }
}

fn damage_from_surface_tree<P>(surface: &wl_surface::WlSurface, location: P) -> Vec<Rectangle<i32, Logical>>
where
    P: Into<Point<i32, Logical>>,
{
    let mut damage = Vec::new();
    with_surface_tree_upward(
        surface,
        location.into(),
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                let data = data.borrow();
                if data.texture.is_none() {
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        location += current.location;
                    }
                    return TraversalAction::DoChildren(location);
                }
            }
            TraversalAction::SkipChildren
        },
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                let data = data.borrow();
                let attributes = states.cached_state.current::<SurfaceAttributes>();

                if data.texture.is_none() {
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        location += current.location;
                    }

                    damage.extend(attributes.damage.iter().map(|dmg| {
                        let mut rect = match dmg {
                            Damage::Buffer(rect) => rect.to_logical(attributes.buffer_scale),
                            Damage::Surface(rect) => *rect,
                        };
                        rect.loc += location;
                        rect
                    }));
                }
            }
        },
        |_, _, _| true,
    );
    damage
}

fn bbox_from_surface_tree<P>(surface: &wl_surface::WlSurface, location: P) -> Rectangle<i32, Logical>
where
    P: Into<Point<i32, Logical>>,
{
    let location = location.into();
    let mut bounding_box = Rectangle::from_loc_and_size(location, (0, 0));
    with_surface_tree_downward(
        surface,
        location,
        |_, states, loc: &Point<i32, Logical>| {
            let mut loc = *loc;
            let data = states.data_map.get::<RefCell<SurfaceState>>();

            if let Some(size) = data.and_then(|d| d.borrow().size()) {
                if states.role == Some("subsurface") {
                    let current = states.cached_state.current::<SubsurfaceCachedState>();
                    loc += current.location;
                }

                // Update the bounding box.
                bounding_box = bounding_box.merge(Rectangle::from_loc_and_size(loc, size));

                TraversalAction::DoChildren(loc)
            } else {
                // If the parent surface is unmapped, then the child surfaces are hidden as
                // well, no need to consider them here.
                TraversalAction::SkipChildren
            }
        },
        |_, _, _| {},
        |_, _, _| true,
    );
    bounding_box
}

pub fn under_from_surface_tree<P>(
    surface: &wl_surface::WlSurface,
    point: Point<f64, Logical>,
    location: P,
) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)>
where
    P: Into<Point<i32, Logical>>,
{
    let found = RefCell::new(None);
    with_surface_tree_downward(
        surface,
        location.into(),
        |wl_surface, states, location: &Point<i32, Logical>| {
            let mut location = *location;
            let data = states.data_map.get::<RefCell<SurfaceState>>();

            if states.role == Some("subsurface") {
                let current = states.cached_state.current::<SubsurfaceCachedState>();
                location += current.location;
            }

            let contains_the_point = data
                .map(|data| {
                    data.borrow()
                        .contains_point(&*states.cached_state.current(), point - location.to_f64())
                })
                .unwrap_or(false);
            if contains_the_point {
                *found.borrow_mut() = Some((wl_surface.clone(), location));
            }

            TraversalAction::DoChildren(location)
        },
        |_, _, _| {},
        |_, _, _| {
            // only continue if the point is not found
            found.borrow().is_none()
        },
    );
    found.into_inner()
}

fn send_frames_surface_tree(surface: &wl_surface::WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            // the surface may not have any user_data if it is a subsurface and has not
            // yet been commited
            for callback in states
                .cached_state
                .current::<SurfaceAttributes>()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

pub fn draw_window<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    window: &Window,
    scale: f64,
    location: Point<i32, Logical>,
    log: &slog::Logger,
) -> Result<(), R::Error>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
{
    if let Some(surface) = window.toplevel().get_surface() {
        draw_surface_tree(renderer, frame, surface, scale, location, log)?;
        for (popup, p_location) in PopupManager::popups_for_surface(surface)
            .ok()
            .into_iter()
            .flatten()
        {
            if let Some(surface) = popup.get_surface() {
                draw_surface_tree(renderer, frame, surface, scale, location + p_location, log)?;
            }
        }
    }
    Ok(())
}
