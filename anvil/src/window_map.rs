use std::cell::RefCell;
use std::sync::Mutex;

use smithay::{
    reexports::{wayland_protocols::xdg_shell::server::xdg_toplevel, wayland_server::protocol::wl_surface},
    utils::Rectangle,
    wayland::{
        compositor::{with_states, with_surface_tree_downward, SubsurfaceCachedState, TraversalAction},
        shell::{
            legacy::ShellSurface,
            xdg::{PopupSurface, SurfaceCachedState, ToplevelSurface, XdgPopupSurfaceRoleAttributes},
        },
    },
};

use crate::shell::SurfaceData;
#[cfg(feature = "xwayland")]
use crate::xwayland::X11Surface;

#[derive(Clone)]
pub enum Kind {
    Xdg(ToplevelSurface),
    Wl(ShellSurface),
    #[cfg(feature = "xwayland")]
    X11(X11Surface),
}

impl Kind {
    pub fn alive(&self) -> bool {
        match *self {
            Kind::Xdg(ref t) => t.alive(),
            Kind::Wl(ref t) => t.alive(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => t.alive(),
        }
    }

    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        match *self {
            Kind::Xdg(ref t) => t.get_surface(),
            Kind::Wl(ref t) => t.get_surface(),
            #[cfg(feature = "xwayland")]
            Kind::X11(ref t) => t.get_surface(),
        }
    }

    /// Do this handle and the other one actually refer to the same toplevel surface?
    pub fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Kind::Xdg(a), Kind::Xdg(b)) => a.equals(b),
            (Kind::Wl(a), Kind::Wl(b)) => a.equals(b),
            #[cfg(feature = "xwayland")]
            (Kind::X11(a), Kind::X11(b)) => a.equals(b),
            _ => false,
        }
    }

    /// Activate/Deactivate this window
    pub fn set_activated(&self, active: bool) {
        if let Kind::Xdg(ref t) = self {
            let changed = t.with_pending_state(|state| {
                if active {
                    state.states.set(xdg_toplevel::State::Activated)
                } else {
                    state.states.unset(xdg_toplevel::State::Activated)
                }
            });
            if let Ok(true) = changed {
                t.send_configure();
            }
        }
    }
}

#[derive(Clone)]
pub enum PopupKind {
    Xdg(PopupSurface),
}

impl PopupKind {
    fn alive(&self) -> bool {
        match *self {
            PopupKind::Xdg(ref t) => t.alive(),
        }
    }

    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        match *self {
            PopupKind::Xdg(ref t) => t.get_surface(),
        }
    }

    fn parent(&self) -> Option<wl_surface::WlSurface> {
        let wl_surface = match self.get_surface() {
            Some(s) => s,
            None => return None,
        };
        with_states(wl_surface, |states| {
            states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .parent
                .clone()
        })
        .ok()
        .flatten()
    }

    pub fn location(&self) -> (i32, i32) {
        let wl_surface = match self.get_surface() {
            Some(s) => s,
            None => return (0, 0),
        };
        let geometry = with_states(wl_surface, |states| {
            states
                .data_map
                .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .current
                .geometry
        })
        .unwrap_or_default();
        (geometry.x, geometry.y)
    }
}

struct Window {
    location: (i32, i32),
    /// A bounding box over this window and its children.
    ///
    /// Used for the fast path of the check in `matching`, and as the fall-back for the window
    /// geometry if that's not set explicitly.
    bbox: Rectangle,
    toplevel: Kind,
}

impl Window {
    /// Finds the topmost surface under this point if any and returns it together with the location of this
    /// surface.
    fn matching(&self, point: (f64, f64)) -> Option<(wl_surface::WlSurface, (f64, f64))> {
        if !self.bbox.contains((point.0 as i32, point.1 as i32)) {
            return None;
        }
        // need to check more carefully
        let found = RefCell::new(None);
        if let Some(wl_surface) = self.toplevel.get_surface() {
            with_surface_tree_downward(
                wl_surface,
                self.location,
                |wl_surface, states, &(mut x, mut y)| {
                    let data = states.data_map.get::<RefCell<SurfaceData>>();

                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        x += current.location.0;
                        y += current.location.1;
                    }

                    let surface_local_point = (point.0 - x as f64, point.1 - y as f64);
                    let contains_the_point = data
                        .map(|data| {
                            data.borrow()
                                .contains_point(&*states.cached_state.current(), surface_local_point)
                        })
                        .unwrap_or(false);
                    if contains_the_point {
                        *found.borrow_mut() = Some((wl_surface.clone(), (x as f64, y as f64)));
                    }

                    TraversalAction::DoChildren((x, y))
                },
                |_, _, _| {},
                |_, _, _| {
                    // only continue if the point is not found
                    found.borrow().is_none()
                },
            );
        }
        found.into_inner()
    }

    fn self_update(&mut self) {
        let (base_x, base_y) = self.location;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (base_x, base_y, base_x, base_y);
        if let Some(wl_surface) = self.toplevel.get_surface() {
            with_surface_tree_downward(
                wl_surface,
                (base_x, base_y),
                |_, states, &(mut x, mut y)| {
                    let data = states.data_map.get::<RefCell<SurfaceData>>();

                    if let Some((w, h)) = data.and_then(|d| d.borrow().size()) {
                        if states.role == Some("subsurface") {
                            let current = states.cached_state.current::<SubsurfaceCachedState>();
                            x += current.location.0;
                            y += current.location.1;
                        }

                        // Update the bounding box.
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x + w);
                        max_y = max_y.max(y + h);

                        TraversalAction::DoChildren((x, y))
                    } else {
                        // If the parent surface is unmapped, then the child surfaces are hidden as
                        // well, no need to consider them here.
                        TraversalAction::SkipChildren
                    }
                },
                |_, _, _| {},
                |_, _, _| true,
            );
        }
        self.bbox = Rectangle {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        };
    }

    /// Returns the geometry of this window.
    pub fn geometry(&self) -> Rectangle {
        // It's the set geometry with the full bounding box as the fallback.
        with_states(self.toplevel.get_surface().unwrap(), |states| {
            states.cached_state.current::<SurfaceCachedState>().geometry
        })
        .unwrap()
        .unwrap_or(self.bbox)
    }

    /// Sends the frame callback to all the subsurfaces in this
    /// window that requested it
    pub fn send_frame(&self, time: u32) {
        if let Some(wl_surface) = self.toplevel.get_surface() {
            with_surface_tree_downward(
                wl_surface,
                (),
                |_, _, &()| TraversalAction::DoChildren(()),
                |_, states, &()| {
                    // the surface may not have any user_data if it is a subsurface and has not
                    // yet been commited
                    SurfaceData::send_frame(&mut *states.cached_state.current(), time)
                },
                |_, _, &()| true,
            );
        }
    }
}

pub struct Popup {
    popup: PopupKind,
}

pub struct WindowMap {
    windows: Vec<Window>,
    popups: Vec<Popup>,
}

impl WindowMap {
    pub fn new() -> Self {
        WindowMap {
            windows: Vec::new(),
            popups: Vec::new(),
        }
    }

    pub fn insert(&mut self, toplevel: Kind, location: (i32, i32)) {
        let mut window = Window {
            location,
            bbox: Rectangle::default(),
            toplevel,
        };
        window.self_update();
        self.windows.insert(0, window);
    }

    pub fn insert_popup(&mut self, popup: PopupKind) {
        let popup = Popup { popup };
        self.popups.push(popup);
    }

    pub fn get_surface_under(&self, point: (f64, f64)) -> Option<(wl_surface::WlSurface, (f64, f64))> {
        for w in &self.windows {
            if let Some(surface) = w.matching(point) {
                return Some(surface);
            }
        }
        None
    }

    pub fn get_surface_and_bring_to_top(
        &mut self,
        point: (f64, f64),
    ) -> Option<(wl_surface::WlSurface, (f64, f64))> {
        let mut found = None;
        for (i, w) in self.windows.iter().enumerate() {
            if let Some(surface) = w.matching(point) {
                found = Some((i, surface));
                break;
            }
        }
        if let Some((i, surface)) = found {
            let winner = self.windows.remove(i);

            // Take activation away from all the windows
            for window in self.windows.iter() {
                window.toplevel.set_activated(false);
            }

            // Give activation to our winner
            winner.toplevel.set_activated(true);

            self.windows.insert(0, winner);
            Some(surface)
        } else {
            None
        }
    }

    pub fn with_windows_from_bottom_to_top<Func>(&self, mut f: Func)
    where
        Func: FnMut(&Kind, (i32, i32), &Rectangle),
    {
        for w in self.windows.iter().rev() {
            f(&w.toplevel, w.location, &w.bbox)
        }
    }
    pub fn with_child_popups<Func>(&self, base: &wl_surface::WlSurface, mut f: Func)
    where
        Func: FnMut(&PopupKind),
    {
        for w in self
            .popups
            .iter()
            .rev()
            .filter(move |w| w.popup.parent().as_ref() == Some(base))
        {
            f(&w.popup)
        }
    }

    pub fn refresh(&mut self) {
        self.windows.retain(|w| w.toplevel.alive());
        self.popups.retain(|p| p.popup.alive());
        for w in &mut self.windows {
            w.self_update();
        }
    }

    /// Refreshes the state of the toplevel, if it exists.
    pub fn refresh_toplevel(&mut self, toplevel: &Kind) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.toplevel.equals(toplevel)) {
            w.self_update();
        }
    }

    pub fn clear(&mut self) {
        self.windows.clear();
    }

    /// Finds the toplevel corresponding to the given `WlSurface`.
    pub fn find(&self, surface: &wl_surface::WlSurface) -> Option<Kind> {
        self.windows.iter().find_map(|w| {
            if w.toplevel
                .get_surface()
                .map(|s| s.as_ref().equals(surface.as_ref()))
                .unwrap_or(false)
            {
                Some(w.toplevel.clone())
            } else {
                None
            }
        })
    }

    /// Finds the popup corresponding to the given `WlSurface`.
    pub fn find_popup(&self, surface: &wl_surface::WlSurface) -> Option<PopupKind> {
        self.popups.iter().find_map(|p| {
            if p.popup
                .get_surface()
                .map(|s| s.as_ref().equals(surface.as_ref()))
                .unwrap_or(false)
            {
                Some(p.popup.clone())
            } else {
                None
            }
        })
    }

    /// Returns the location of the toplevel, if it exists.
    pub fn location(&self, toplevel: &Kind) -> Option<(i32, i32)> {
        self.windows
            .iter()
            .find(|w| w.toplevel.equals(toplevel))
            .map(|w| w.location)
    }

    /// Sets the location of the toplevel, if it exists.
    pub fn set_location(&mut self, toplevel: &Kind, location: (i32, i32)) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.toplevel.equals(toplevel)) {
            w.location = location;
            w.self_update();
        }
    }

    /// Returns the geometry of the toplevel, if it exists.
    pub fn geometry(&self, toplevel: &Kind) -> Option<Rectangle> {
        self.windows
            .iter()
            .find(|w| w.toplevel.equals(toplevel))
            .map(|w| w.geometry())
    }

    pub fn send_frames(&self, time: u32) {
        for window in &self.windows {
            window.send_frame(time);
        }
    }
}
