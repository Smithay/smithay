use smithay::compositor::{CompositorToken, SubsurfaceRole, SurfaceAttributes, TraversalAction};
use smithay::compositor::roles::Role;
use smithay::shell::{ShellSurfaceRole, ToplevelSurface};
use smithay::utils::Rectangle;
use wayland_server::Resource;
use wayland_server::protocol::wl_surface;

struct Window<U, R, CID, SD> {
    location: (i32, i32),
    surface: Rectangle,
    toplevel: ToplevelSurface<U, R, CID, SD>,
}

impl<U, R, CID, SD> Window<U, R, CID, SD>
where
    U: 'static,
    R: Role<SubsurfaceRole> + Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SD: 'static,
{
    // Find the topmost surface under this point if any and the location of this point in the surface
    fn matching<F>(&self, point: (f64, f64), ctoken: CompositorToken<U, R, CID>, get_size: F)
                   -> Option<(wl_surface::WlSurface, (f64, f64))>
    where
        F: Fn(&SurfaceAttributes<U>) -> Option<(i32, i32)>,
    {
        if !self.surface.contains((point.0 as i32, point.1 as i32)) {
            return None;
        }
        // need to check more carefully
        let mut found = None;
        if let Some(wl_surface) = self.toplevel.get_surface() {
            ctoken.with_surface_tree_downward(
                wl_surface,
                self.location,
                |wl_surface, attributes, role, &(mut x, mut y)| if let Some((w, h)) = get_size(attributes) {
                    if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                        x += subdata.x;
                        y += subdata.y;
                    }
                    let my_rect = Rectangle {
                        x,
                        y,
                        width: w,
                        height: h,
                    };
                    if my_rect.contains((point.0 as i32, point.1 as i32)) {
                        found = wl_surface.clone().map(|s| {
                            (s, (point.0 - my_rect.x as f64, point.1 - my_rect.y as f64))
                        });
                        TraversalAction::Break
                    } else {
                        TraversalAction::DoChildren((x, y))
                    }
                } else {
                    TraversalAction::SkipChildren
                },
            );
        }
        found
    }

    fn self_update<F>(&mut self, ctoken: CompositorToken<U, R, CID>, mut get_size: F)
    where
        F: Fn(&SurfaceAttributes<U>) -> Option<(i32, i32)>,
    {
        let (base_x, base_y) = self.location;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (base_x, base_y, base_x, base_y);
        if let Some(wl_surface) = self.toplevel.get_surface() {
            ctoken.with_surface_tree_downward(
                wl_surface,
                (base_x, base_y),
                |_, attributes, role, &(mut x, mut y)| {
                    if let Some((w, h)) = get_size(attributes) {
                        if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                            x += subdata.x;
                            y += subdata.y;
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
            );
        }
        self.surface = Rectangle {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        };
    }
}

pub struct WindowMap<U, R, CID, SD, F> {
    ctoken: CompositorToken<U, R, CID>,
    windows: Vec<Window<U, R, CID, SD>>,
    get_size: F,
}

impl<U, R, CID, SD, F> WindowMap<U, R, CID, SD, F>
where
    F: Fn(&SurfaceAttributes<U>) -> Option<(i32, i32)>,
    U: 'static,
    R: Role<SubsurfaceRole> + Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SD: 'static,
{
    pub fn new(ctoken: CompositorToken<U, R, CID>, get_size: F) -> WindowMap<U, R, CID, SD, F> {
        WindowMap {
            ctoken: ctoken,
            windows: Vec::new(),
            get_size: get_size,
        }
    }

    pub fn insert(&mut self, toplevel: ToplevelSurface<U, R, CID, SD>, location: (i32, i32)) {
        let mut window = Window {
            location: location,
            surface: Rectangle {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            toplevel: toplevel,
        };
        window.self_update(self.ctoken, &self.get_size);
        self.windows.insert(0, window);
    }

    pub fn get_surface_under(&self, point: (f64, f64)) -> Option<(wl_surface::WlSurface, (f64, f64))> {
        for w in &self.windows {
            if let Some(surface) = w.matching(point, self.ctoken, &self.get_size) {
                return Some(surface);
            }
        }
        None
    }

    pub fn get_surface_and_bring_to_top(&mut self, point: (f64, f64))
                                        -> Option<(wl_surface::WlSurface, (f64, f64))> {
        let mut found = None;
        for (i, w) in self.windows.iter().enumerate() {
            if let Some(surface) = w.matching(point, self.ctoken, &self.get_size) {
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
        Func: FnMut(&ToplevelSurface<U, R, CID, SD>, (i32, i32)),
    {
        for w in self.windows.iter().rev() {
            f(&w.toplevel, w.location)
        }
    }

    pub fn refresh(&mut self) {
        self.windows.retain(|w| w.toplevel.alive());
        for w in self.windows.iter_mut() {
            w.self_update(self.ctoken, &self.get_size);
        }
    }

    pub fn clear(&mut self) {
        self.windows.clear();
    }
}
