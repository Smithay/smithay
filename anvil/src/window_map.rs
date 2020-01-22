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
}

struct Window<R> {
    location: (i32, i32),
    /// A bounding box over the input areas of this window and its children.
    ///
    /// Used for the fast path of the check in `matching`.
    input_bbox: Rectangle,
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
        if !self.input_bbox.contains((point.0 as i32, point.1 as i32)) {
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
                        // update the bounding box
                        if x < min_x {
                            min_x = x;
                        }
                        if y < min_y {
                            min_y = y;
                        }
                        if x + w > max_x {
                            max_x = x + w;
                        }
                        if y + h > max_y {
                            max_y = y + w;
                        }
                        TraversalAction::DoChildren((x, y))
                    } else {
                        TraversalAction::SkipChildren
                    }
                },
                |_, _, _, _| {},
                |_, _, _, _| true,
            );
        }
        self.input_bbox = Rectangle {
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
            input_bbox: Rectangle::default(),
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
}
