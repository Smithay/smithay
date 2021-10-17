use std::cell::RefCell;

use smithay::{
    reexports::wayland_server::protocol::wl_surface,
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{with_states, with_surface_tree_downward, SubsurfaceCachedState, TraversalAction},
        shell::wlr_layer::{self, Anchor, LayerSurfaceCachedState},
    },
};

use crate::{output_map::Output, shell::SurfaceData};

#[derive(Debug)]
pub struct LayerSurface {
    pub surface: wlr_layer::LayerSurface,
    pub location: Point<i32, Logical>,
    pub bbox: Rectangle<i32, Logical>,
    pub layer: wlr_layer::Layer,
}

impl LayerSurface {
    /// Finds the topmost surface under this point if any and returns it together with the location of this
    /// surface.
    fn matching(&self, point: Point<f64, Logical>) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)> {
        if !self.bbox.to_f64().contains(point) {
            return None;
        }
        // need to check more carefully
        let found = RefCell::new(None);
        if let Some(wl_surface) = self.surface.get_surface() {
            with_surface_tree_downward(
                wl_surface,
                self.location,
                |wl_surface, states, location| {
                    let mut location = *location;
                    let data = states.data_map.get::<RefCell<SurfaceData>>();

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
        }
        found.into_inner()
    }

    fn self_update(&mut self) {
        let mut bounding_box = Rectangle::from_loc_and_size(self.location, (0, 0));
        if let Some(wl_surface) = self.surface.get_surface() {
            with_surface_tree_downward(
                wl_surface,
                self.location,
                |_, states, &loc| {
                    let mut loc = loc;
                    let data = states.data_map.get::<RefCell<SurfaceData>>();

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
        }
        self.bbox = bounding_box;

        if let Some(surface) = self.surface.get_surface() {
            self.layer = with_states(surface, |states| {
                let current = states.cached_state.current::<LayerSurfaceCachedState>();
                current.layer
            })
            .unwrap();
        }
    }

    /// Sends the frame callback to all the subsurfaces in this
    /// window that requested it
    fn send_frame(&self, time: u32) {
        if let Some(wl_surface) = self.surface.get_surface() {
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

#[derive(Debug, Default)]
pub struct LayerMap {
    surfaces: Vec<LayerSurface>,
}

impl LayerMap {
    pub fn insert(&mut self, surface: wlr_layer::LayerSurface, layer: wlr_layer::Layer) {
        let mut layer = LayerSurface {
            location: Default::default(),
            bbox: Rectangle::default(),
            surface,
            layer,
        };
        layer.self_update();
        self.surfaces.insert(0, layer);
    }

    pub fn get_surface_under(
        &self,
        layer: &wlr_layer::Layer,
        point: Point<f64, Logical>,
    ) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)> {
        for l in self.surfaces.iter().filter(|s| &s.layer == layer) {
            if let Some(surface) = l.matching(point) {
                return Some(surface);
            }
        }
        None
    }

    pub fn with_layers_from_bottom_to_top<Func>(&self, layer: &wlr_layer::Layer, mut f: Func)
    where
        Func: FnMut(&LayerSurface),
    {
        for l in self.surfaces.iter().filter(|s| &s.layer == layer).rev() {
            f(l)
        }
    }

    pub fn refresh(&mut self) {
        self.surfaces.retain(|l| l.surface.alive());

        for l in self.surfaces.iter_mut() {
            l.self_update();
        }
    }

    /// Finds the layer corresponding to the given `WlSurface`.
    pub fn find(&self, surface: &wl_surface::WlSurface) -> Option<&LayerSurface> {
        self.surfaces.iter().find_map(|l| {
            if l.surface
                .get_surface()
                .map(|s| s.as_ref().equals(surface.as_ref()))
                .unwrap_or(false)
            {
                Some(l)
            } else {
                None
            }
        })
    }

    pub fn arange_layers(&mut self, output: &Output) {
        let output_rect = output.geometry();

        // Get all layer surfaces assigned to this output
        let surfaces: Vec<_> = output
            .layer_surfaces()
            .into_iter()
            .map(|s| s.as_ref().clone())
            .collect();

        // Find layers for this output
        let filtered_layers = self.surfaces.iter_mut().filter(|l| {
            l.surface
                .get_surface()
                .map(|s| surfaces.contains(s.as_ref()))
                .unwrap_or(false)
        });

        for layer in filtered_layers {
            let surface = if let Some(surface) = layer.surface.get_surface() {
                surface
            } else {
                continue;
            };

            let data = with_states(surface, |states| {
                *states.cached_state.current::<LayerSurfaceCachedState>()
            })
            .unwrap();

            let x = if data.size.w == 0 || data.anchor.contains(Anchor::LEFT) {
                output_rect.loc.x
            } else if data.anchor.contains(Anchor::RIGHT) {
                output_rect.loc.x + (output_rect.size.w - data.size.w)
            } else {
                output_rect.loc.x + ((output_rect.size.w / 2) - (data.size.w / 2))
            };

            let y = if data.size.h == 0 || data.anchor.contains(Anchor::TOP) {
                output_rect.loc.y
            } else if data.anchor.contains(Anchor::BOTTOM) {
                output_rect.loc.y + (output_rect.size.h - data.size.h)
            } else {
                output_rect.loc.y + ((output_rect.size.h / 2) - (data.size.h / 2))
            };

            let location: Point<i32, Logical> = (x, y).into();

            layer
                .surface
                .with_pending_state(|state| {
                    state.size = Some(output_rect.size);
                })
                .unwrap();

            layer.surface.send_configure();

            layer.location = location;
        }
    }

    pub fn send_frames(&self, time: u32) {
        for layer in &self.surfaces {
            layer.send_frame(time);
        }
    }
}
