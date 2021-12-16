use crate::{
    backend::renderer::utils::SurfaceState,
    desktop::Space,
    utils::{Logical, Point, Rectangle, Size},
    wayland::{
        compositor::{
            with_surface_tree_downward, with_surface_tree_upward, Damage, SubsurfaceCachedState,
            SurfaceAttributes, TraversalAction,
        },
        output::Output,
    },
};
use wayland_server::protocol::wl_surface;

use std::{cell::RefCell, sync::Arc};

impl SurfaceState {
    /// Returns the size of the surface.
    pub fn size(&self) -> Option<Size<i32, Logical>> {
        self.buffer_dimensions
            .map(|dims| dims.to_logical(self.buffer_scale))
    }

    fn contains_point<P: Into<Point<f64, Logical>>>(&self, attrs: &SurfaceAttributes, point: P) -> bool {
        let point = point.into();
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

pub fn bbox_from_surface_tree<P>(surface: &wl_surface::WlSurface, location: P) -> Rectangle<i32, Logical>
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

pub fn damage_from_surface_tree<P>(
    surface: &wl_surface::WlSurface,
    location: P,
    key: Option<(&Space, &Output)>,
) -> Vec<Rectangle<i32, Logical>>
where
    P: Into<Point<i32, Logical>>,
{
    let mut damage = Vec::new();
    let key = key.map(|(space, output)| (space.id, Arc::as_ptr(&output.inner) as *const ()));
    with_surface_tree_upward(
        surface,
        location.into(),
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                let data = data.borrow();
                if key
                    .as_ref()
                    .map(|key| !data.damage_seen.contains(key))
                    .unwrap_or(true)
                {
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        location += current.location;
                    }
                }
            }
            TraversalAction::DoChildren(location)
        },
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                let mut data = data.borrow_mut();
                let attributes = states.cached_state.current::<SurfaceAttributes>();

                if key
                    .as_ref()
                    .map(|key| !data.damage_seen.contains(key))
                    .unwrap_or(true)
                {
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

                    if let Some(key) = key {
                        data.damage_seen.insert(key);
                    }
                }
            }
        },
        |_, _, _| true,
    );
    damage
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

pub fn send_frames_surface_tree(surface: &wl_surface::WlSurface, time: u32) {
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
