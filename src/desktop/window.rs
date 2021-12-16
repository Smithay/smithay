use crate::{
    backend::renderer::{utils::draw_surface_tree, Frame, ImportAll, Renderer, Texture},
    desktop::{utils::*, PopupManager, Space},
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::with_states,
        output::Output,
        shell::xdg::{SurfaceCachedState, ToplevelSurface},
    },
};
use std::{
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
use wayland_server::protocol::wl_surface;

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
    pub fn surface_under<P: Into<Point<f64, Logical>>>(
        &self,
        point: P,
    ) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)> {
        let point = point.into();
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
    pub(super) fn accumulated_damage(
        &self,
        for_values: Option<(&Space, &Output)>,
    ) -> Vec<Rectangle<i32, Logical>> {
        let mut damage = Vec::new();
        if let Some(surface) = self.0.toplevel.get_surface() {
            damage.extend(damage_from_surface_tree(surface, (0, 0), for_values));
            for (popup, location) in PopupManager::popups_for_surface(surface)
                .ok()
                .into_iter()
                .flatten()
            {
                if let Some(surface) = popup.get_surface() {
                    let popup_damage = damage_from_surface_tree(surface, location, for_values);
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

pub fn draw_window<R, E, F, T, P>(
    renderer: &mut R,
    frame: &mut F,
    window: &Window,
    scale: f64,
    location: P,
    damage: &[Rectangle<i32, Logical>],
    log: &slog::Logger,
) -> Result<(), R::Error>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
    P: Into<Point<i32, Logical>>,
{
    let location = location.into();
    if let Some(surface) = window.toplevel().get_surface() {
        draw_surface_tree(renderer, frame, surface, scale, location, damage, log)?;
        for (popup, p_location) in PopupManager::popups_for_surface(surface)
            .ok()
            .into_iter()
            .flatten()
        {
            if let Some(surface) = popup.get_surface() {
                let damage = damage
                    .iter()
                    .cloned()
                    .map(|mut geo| {
                        geo.loc -= p_location;
                        geo
                    })
                    .collect::<Vec<_>>();
                draw_surface_tree(
                    renderer,
                    frame,
                    surface,
                    scale,
                    location + p_location,
                    &damage,
                    log,
                )?;
            }
        }
    }
    Ok(())
}
