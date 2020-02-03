use std::cell::RefCell;

use smithay::{
    reexports::wayland_server::protocol::wl_surface,
    utils::Rectangle,
    wayland::{
        compositor::{roles::Role, CompositorToken, SubsurfaceRole, SurfaceAttributes, TraversalAction},
        shell::{
            legacy::{ShellSurface, ShellSurfaceRole},
            xdg::{ToplevelSurface, XdgSurfaceRole},
        },
    },
};

pub enum Kind<R> {
    Xdg(ToplevelSurface<R>),
    Wl(ShellSurface<R>),
}

impl<R> Kind<R>
where
    R: Role<SubsurfaceRole> + Role<XdgSurfaceRole> + Role<ShellSurfaceRole> + 'static,
{
    pub fn alive(&self) -> bool {
        match *self {
            Kind::Xdg(ref t) => t.alive(),
            Kind::Wl(ref t) => t.alive(),
        }
    }
    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        match *self {
            Kind::Xdg(ref t) => t.get_surface(),
            Kind::Wl(ref t) => t.get_surface(),
        }
    }

    /// Do this handle and the other one actually refer to the same toplevel surface?
    pub fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Kind::Xdg(a), Kind::Xdg(b)) => a.equals(b),
            (Kind::Wl(a), Kind::Wl(b)) => a.equals(b),
            _ => false,
        }
    }
}

struct Window<R> {
    location: (i32, i32),
    /// A bounding box over this window and its children.
    ///
    /// Used for the fast path of the check in `matching`, and as the fall-back for the window
    /// geometry if that's not set explicitly.
    bbox: Rectangle,
    toplevel: Kind<R>,
}

impl<R> Window<R>
where
    R: Role<SubsurfaceRole> + Role<XdgSurfaceRole> + Role<ShellSurfaceRole> + 'static,
{
    /// Finds the topmost surface under this point if any and returns it together with the location of this
    /// surface.
    ///
    /// You need to provide a `contains_point` function which checks if the point (in surface-local
    /// coordinates) is within the input region of the given `SurfaceAttributes`.
    fn matching<F>(
        &self,
        point: (f64, f64),
        ctoken: CompositorToken<R>,
        contains_point: F,
    ) -> Option<(wl_surface::WlSurface, (f64, f64))>
    where
        F: Fn(&SurfaceAttributes, (f64, f64)) -> bool,
    {
        if !self.bbox.contains((point.0 as i32, point.1 as i32)) {
            return None;
        }
        // need to check more carefully
        let found = RefCell::new(None);
        if let Some(wl_surface) = self.toplevel.get_surface() {
            ctoken.with_surface_tree_downward(
                wl_surface,
                self.location,
                |wl_surface, attributes, role, &(mut x, mut y)| {
                    if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                        x += subdata.location.0;
                        y += subdata.location.1;
                    }

                    let surface_local_point = (point.0 - x as f64, point.1 - y as f64);
                    if contains_point(attributes, surface_local_point) {
                        *found.borrow_mut() = Some((wl_surface.clone(), (x as f64, y as f64)));
                    }

                    TraversalAction::DoChildren((x, y))
                },
                |_, _, _, _| {},
                |_, _, _, _| {
                    // only continue if the point is not found
                    found.borrow().is_none()
                },
            );
        }
        found.into_inner()
    }

    fn self_update<F>(&mut self, ctoken: CompositorToken<R>, get_size: F)
    where
        F: Fn(&SurfaceAttributes) -> Option<(i32, i32)>,
    {
        let (base_x, base_y) = self.location;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (base_x, base_y, base_x, base_y);
        if let Some(wl_surface) = self.toplevel.get_surface() {
            ctoken.with_surface_tree_downward(
                wl_surface,
                (base_x, base_y),
                |_, attributes, role, &(mut x, mut y)| {
                    // The input region is intersected with the surface size, so the surface size
                    // can serve as an approximation for the input bounding box.
                    if let Some((w, h)) = get_size(attributes) {
                        if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                            x += subdata.location.0;
                            y += subdata.location.1;
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
                |_, _, _, _| {},
                |_, _, _, _| true,
            );
        }
        self.bbox = Rectangle {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        };
    }
}

pub struct WindowMap<R, F, G> {
    ctoken: CompositorToken<R>,
    windows: Vec<Window<R>>,
    /// A function returning the surface size.
    get_size: F,
    /// A function that checks if the point is in the surface's input region.
    contains_point: G,
}

impl<R, F, G> WindowMap<R, F, G>
where
    F: Fn(&SurfaceAttributes) -> Option<(i32, i32)>,
    G: Fn(&SurfaceAttributes, (f64, f64)) -> bool,
    R: Role<SubsurfaceRole> + Role<XdgSurfaceRole> + Role<ShellSurfaceRole> + 'static,
{
    pub fn new(ctoken: CompositorToken<R>, get_size: F, contains_point: G) -> Self {
        WindowMap {
            ctoken,
            windows: Vec::new(),
            get_size,
            contains_point,
        }
    }

    pub fn insert(&mut self, toplevel: Kind<R>, location: (i32, i32)) {
        let mut window = Window {
            location,
            bbox: Rectangle::default(),
            toplevel,
        };
        window.self_update(self.ctoken, &self.get_size);
        self.windows.insert(0, window);
    }

    pub fn get_surface_under(&self, point: (f64, f64)) -> Option<(wl_surface::WlSurface, (f64, f64))> {
        for w in &self.windows {
            if let Some(surface) = w.matching(point, self.ctoken, &self.contains_point) {
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
            if let Some(surface) = w.matching(point, self.ctoken, &self.contains_point) {
                found = Some((i, surface));
                break;
            }
        }
        if let Some((i, surface)) = found {
            let winner = self.windows.remove(i);
            self.windows.insert(0, winner);
            Some(surface)
        } else {
            None
        }
    }

    pub fn with_windows_from_bottom_to_top<Func>(&self, mut f: Func)
    where
        Func: FnMut(&Kind<R>, (i32, i32)),
    {
        for w in self.windows.iter().rev() {
            f(&w.toplevel, w.location)
        }
    }

    pub fn refresh(&mut self) {
        self.windows.retain(|w| w.toplevel.alive());
        for w in &mut self.windows {
            w.self_update(self.ctoken, &self.get_size);
        }
    }

    pub fn clear(&mut self) {
        self.windows.clear();
    }

    /// Returns the location of the toplevel, if it exists.
    pub fn location(&self, toplevel: &Kind<R>) -> Option<(i32, i32)> {
        self.windows
            .iter()
            .find(|w| w.toplevel.equals(toplevel))
            .map(|w| w.location)
    }

    /// Sets the location of the toplevel, if it exists.
    pub fn set_location(&mut self, toplevel: &Kind<R>, location: (i32, i32)) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.toplevel.equals(toplevel)) {
            w.location = location;
            w.self_update(self.ctoken, &self.get_size);
        }
    }
}
