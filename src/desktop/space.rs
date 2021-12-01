use super::{draw_window, SurfaceState, Window};
use crate::{
    backend::renderer::{Frame, ImportAll, Renderer, Transform},
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{with_surface_tree_downward, SubsurfaceCachedState, TraversalAction},
        output::Output,
    },
};
use indexmap::{IndexMap, IndexSet};
use std::{
    cell::{RefCell, RefMut},
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
use wayland_server::protocol::wl_surface::WlSurface;

static SPACE_ID: AtomicUsize = AtomicUsize::new(0);
lazy_static::lazy_static! {
    static ref SPACE_IDS: Mutex<HashSet<usize>> = Mutex::new(HashSet::new());
}
fn next_space_id() -> usize {
    let mut ids = SPACE_IDS.lock().unwrap();
    if ids.len() == usize::MAX {
        // Theoretically the code below wraps around correctly,
        // but that is hard to detect and might deadlock.
        // Maybe make this a debug_assert instead?
        panic!("Out of space ids");
    }

    let mut id = SPACE_ID.fetch_add(1, Ordering::SeqCst);
    while ids.iter().any(|k| *k == id) {
        id = SPACE_ID.fetch_add(1, Ordering::SeqCst);
    }

    ids.insert(id);
    id
}

#[derive(Default)]
struct WindowState {
    location: Point<i32, Logical>,
    drawn: bool,
}

type WindowUserdata = RefCell<HashMap<usize, WindowState>>;
fn window_state(space: usize, w: &Window) -> RefMut<'_, WindowState> {
    let userdata = w.user_data();
    userdata.insert_if_missing(WindowUserdata::default);
    RefMut::map(userdata.get::<WindowUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}

#[derive(Clone, Default)]
struct OutputState {
    location: Point<i32, Logical>,
    render_scale: f64,

    // damage and last_state are in space coordinate space
    old_damage: VecDeque<Vec<Rectangle<i32, Logical>>>,
    last_state: IndexMap<usize, Rectangle<i32, Logical>>,

    // surfaces for tracking enter and leave events
    surfaces: Vec<WlSurface>,
}

type OutputUserdata = RefCell<HashMap<usize, OutputState>>;
fn output_state(space: usize, o: &Output) -> RefMut<'_, OutputState> {
    let userdata = o.user_data();
    userdata.insert_if_missing(OutputUserdata::default);
    RefMut::map(userdata.get::<OutputUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}

// TODO: Maybe replace UnmanagedResource if nothing else comes up?
#[derive(Debug, thiserror::Error)]
pub enum SpaceError {
    #[error("Window is not mapped to this space")]
    UnknownWindow,
}

#[derive(Debug)]
pub struct Space {
    id: usize,
    // in z-order, back to front
    windows: IndexSet<Window>,
    outputs: Vec<Output>,
    // TODO:
    //layers: Vec<Layer>,
    logger: ::slog::Logger,
}

impl Drop for Space {
    fn drop(&mut self) {
        SPACE_IDS.lock().unwrap().remove(&self.id);
    }
}

impl Space {
    pub fn new<L>(log: L) -> Space
    where
        L: Into<slog::Logger>,
    {
        Space {
            id: next_space_id(),
            windows: IndexSet::new(),
            outputs: Vec::new(),
            logger: log.into(),
        }
    }

    /// Map window and moves it to top of the stack
    ///
    /// This can safely be called on an already mapped window
    pub fn map_window(&mut self, window: &Window, location: Point<i32, Logical>) {
        window_state(self.id, window).location = location - window.geometry().loc;
        self.windows.shift_remove(window);
        self.windows.insert(window.clone());

        // TODO: should this be handled by us?
        window.set_activated(true);
        for w in self.windows.iter() {
            if w != window {
                w.set_activated(false);
            }
        }
    }

    pub fn raise_window(&mut self, window: &Window) {
        let loc = window_geo(window, &self.id).loc;
        self.map_window(window, loc);
    }

    /// Unmap a window from this space by its id
    pub fn unmap_window(&mut self, window: &Window) {
        if let Some(map) = window.user_data().get::<WindowUserdata>() {
            map.borrow_mut().remove(&self.id);
        }
        self.windows.shift_remove(window);
    }

    /// Iterate window in z-order back to front
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.iter()
    }

    /// Get a reference to the window under a given point, if any
    pub fn window_under(&self, point: Point<f64, Logical>) -> Option<&Window> {
        self.windows.iter().find(|w| {
            let bbox = window_rect(w, &self.id);
            bbox.to_f64().contains(point)
        })
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<&Window> {
        if !surface.as_ref().is_alive() {
            return None;
        }

        self.windows
            .iter()
            .find(|w| w.toplevel().get_surface().map(|x| x == surface).unwrap_or(false))
    }

    pub fn window_geometry(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.windows.contains(w) {
            return None;
        }

        Some(window_geo(w, &self.id))
    }

    pub fn window_bbox(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.windows.contains(w) {
            return None;
        }

        Some(window_rect(w, &self.id))
    }

    pub fn map_output(&mut self, output: &Output, scale: f64, location: Point<i32, Logical>) {
        let mut state = output_state(self.id, output);
        *state = OutputState {
            location,
            render_scale: scale,
            ..Default::default()
        };
        if !self.outputs.contains(output) {
            self.outputs.push(output.clone());
        }
    }

    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.outputs.iter()
    }

    pub fn unmap_output(&mut self, output: &Output) {
        if let Some(map) = output.user_data().get::<OutputUserdata>() {
            map.borrow_mut().remove(&self.id);
        }
        self.outputs.retain(|o| o != output);
    }

    pub fn output_geometry(&self, o: &Output) -> Option<Rectangle<i32, Logical>> {
        if !self.outputs.contains(o) {
            return None;
        }

        let state = output_state(self.id, o);
        o.current_mode().map(|mode| {
            Rectangle::from_loc_and_size(
                state.location,
                mode.size.to_f64().to_logical(state.render_scale).to_i32_round(),
            )
        })
    }

    pub fn output_scale(&self, o: &Output) -> Option<f64> {
        if !self.outputs.contains(o) {
            return None;
        }

        let state = output_state(self.id, o);
        Some(state.render_scale)
    }

    pub fn output_for_window(&self, w: &Window) -> Option<Output> {
        if !self.windows.contains(w) {
            return None;
        }

        let w_geo = self.window_geometry(w).unwrap();
        for o in &self.outputs {
            let o_geo = self.output_geometry(o).unwrap();
            if w_geo.overlaps(o_geo) {
                return Some(o.clone());
            }
        }

        // TODO primary output
        self.outputs.get(0).cloned()
    }

    pub fn refresh(&mut self) {
        self.windows.retain(|w| w.toplevel().alive());

        for output in &mut self.outputs {
            output_state(self.id, output)
                .surfaces
                .retain(|s| s.as_ref().is_alive());
        }

        for window in &self.windows {
            let bbox = window_rect(window, &self.id);
            let kind = window.toplevel();

            for output in &self.outputs {
                let output_geometry = self
                    .output_geometry(output)
                    .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (0, 0)));
                let mut output_state = output_state(self.id, output);

                // Check if the bounding box of the toplevel intersects with
                // the output, if not no surface in the tree can intersect with
                // the output.
                if !output_geometry.overlaps(bbox) {
                    if let Some(surface) = kind.get_surface() {
                        with_surface_tree_downward(
                            surface,
                            (),
                            |_, _, _| TraversalAction::DoChildren(()),
                            |wl_surface, _, _| {
                                if output_state.surfaces.contains(wl_surface) {
                                    slog::trace!(
                                        self.logger,
                                        "surface ({:?}) leaving output {:?}",
                                        wl_surface,
                                        output.name()
                                    );
                                    output.leave(wl_surface);
                                    output_state.surfaces.retain(|s| s != wl_surface);
                                }
                            },
                            |_, _, _| true,
                        )
                    }
                    continue;
                }

                if let Some(surface) = kind.get_surface() {
                    with_surface_tree_downward(
                        surface,
                        window_loc(window, &self.id),
                        |_, states, location| {
                            let mut location = *location;
                            let data = states.data_map.get::<RefCell<SurfaceState>>();

                            if data.is_some() {
                                if states.role == Some("subsurface") {
                                    let current = states.cached_state.current::<SubsurfaceCachedState>();
                                    location += current.location;
                                }

                                TraversalAction::DoChildren(location)
                            } else {
                                // If the parent surface is unmapped, then the child surfaces are hidden as
                                // well, no need to consider them here.
                                TraversalAction::SkipChildren
                            }
                        },
                        |wl_surface, states, &loc| {
                            let data = states.data_map.get::<RefCell<SurfaceState>>();

                            if let Some(size) = data.and_then(|d| d.borrow().size()) {
                                let surface_rectangle = Rectangle { loc, size };

                                if output_geometry.overlaps(surface_rectangle) {
                                    // We found a matching output, check if we already sent enter
                                    if !output_state.surfaces.contains(wl_surface) {
                                        slog::trace!(
                                            self.logger,
                                            "surface ({:?}) entering output {:?}",
                                            wl_surface,
                                            output.name()
                                        );
                                        output.enter(wl_surface);
                                        output_state.surfaces.push(wl_surface.clone());
                                    }
                                } else {
                                    // Surface does not match output, if we sent enter earlier
                                    // we should now send leave
                                    if output_state.surfaces.contains(wl_surface) {
                                        slog::trace!(
                                            self.logger,
                                            "surface ({:?}) leaving output {:?}",
                                            wl_surface,
                                            output.name()
                                        );
                                        output.leave(wl_surface);
                                        output_state.surfaces.retain(|s| s != wl_surface);
                                    }
                                }
                            } else {
                                // Maybe the the surface got unmapped, send leave on output
                                if output_state.surfaces.contains(wl_surface) {
                                    slog::trace!(
                                        self.logger,
                                        "surface ({:?}) leaving output {:?}",
                                        wl_surface,
                                        output.name()
                                    );
                                    output.leave(wl_surface);
                                    output_state.surfaces.retain(|s| s != wl_surface);
                                }
                            }
                        },
                        |_, _, _| true,
                    )
                }
            }
        }
    }

    pub fn render_output<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        age: usize,
        clear_color: [f32; 4],
    ) -> Result<bool, RenderError<R>>
    where
        R: Renderer + ImportAll,
        R::TextureId: 'static,
    {
        let mut state = output_state(self.id, output);
        let output_size = output
            .current_mode()
            .ok_or(RenderError::OutputNoMode)?
            .size
            .to_f64()
            .to_logical(state.render_scale)
            .to_i32_round();
        let output_geo = Rectangle::from_loc_and_size(state.location, output_size);

        // This will hold all the damage we need for this rendering step
        let mut damage = Vec::<Rectangle<i32, Logical>>::new();
        // First add damage for windows gone
        for old_window in state
            .last_state
            .iter()
            .filter_map(|(id, w)| {
                if !self.windows.iter().any(|w| w.0.id == *id) {
                    Some(*w)
                } else {
                    None
                }
            })
            .collect::<Vec<Rectangle<i32, Logical>>>()
        {
            slog::trace!(self.logger, "Removing window at: {:?}", old_window);
            damage.push(old_window);
        }

        // lets iterate front to back and figure out, what new windows or unmoved windows we have
        for window in self.windows.iter().rev() {
            let geo = window_rect(window, &self.id);
            let old_geo = state.last_state.get(&window.0.id).cloned();

            // window was moved or resized
            if old_geo.map(|old_geo| old_geo != geo).unwrap_or(false) {
                // Add damage for the old position of the window
                damage.push(old_geo.unwrap());
                damage.push(geo);
            } else {
                // window stayed at its place
                let loc = window_loc(window, &self.id);
                damage.extend(window.accumulated_damage().into_iter().map(|mut rect| {
                    rect.loc += loc;
                    rect
                }));
            }
        }

        // That is all completely new damage, which we need to store for subsequent renders
        let new_damage = damage.clone();
        // We now add old damage states, if we have an age value
        if age > 0 && state.old_damage.len() >= age {
            // We do not need older states anymore
            state.old_damage.truncate(age);
            damage.extend(state.old_damage.iter().flatten().copied());
        } else {
            // just damage everything, if we have no damage
            damage = vec![output_geo];
        }

        // Optimize the damage for rendering
        damage.retain(|rect| rect.overlaps(output_geo));
        damage.retain(|rect| rect.size.h > 0 && rect.size.w > 0);
        for rect in damage.clone().iter() {
            // if this rect was already removed, because it was smaller as another one,
            // there is no reason to evaluate this.
            if damage.contains(rect) {
                // remove every rectangle that is contained in this rectangle
                damage.retain(|other| !rect.contains_rect(*other));
            }
        }

        let output_transform: Transform = output.current_transform().into();
        if let Err(err) = renderer.render(
            output_transform
                .transform_size(output_size)
                .to_f64()
                .to_physical(state.render_scale)
                .to_i32_round(),
            output_transform,
            |renderer, frame| {
                // First clear all damaged regions
                for geo in &damage {
                    slog::trace!(self.logger, "Clearing at {:?}", geo);
                    frame.clear(
                        clear_color,
                        Some(geo.to_f64().to_physical(state.render_scale).to_i32_ceil()),
                    )?;
                }

                // Then re-draw all window overlapping with a damage rect.
                for window in self.windows.iter() {
                    let wgeo = window_rect(window, &self.id);
                    let mut loc = window_loc(window, &self.id);
                    if damage.iter().any(|geo| wgeo.overlaps(*geo)) {
                        loc -= output_geo.loc;
                        slog::trace!(self.logger, "Rendering window at {:?}", wgeo);
                        draw_window(renderer, frame, window, state.render_scale, loc, &self.logger)?;
                        window_state(self.id, window).drawn = true;
                    }
                }

                Result::<(), R::Error>::Ok(())
            },
        ) {
            // if the rendering errors on us, we need to be prepared, that this whole buffer was partially updated and thus now unusable.
            // thus clean our old states before returning
            state.old_damage = VecDeque::new();
            state.last_state = IndexMap::new();
            return Err(RenderError::Rendering(err));
        }

        // If rendering was successful capture the state and add the damage
        state.last_state = self
            .windows
            .iter()
            .map(|window| {
                let wgeo = window_rect(window, &self.id);
                (window.0.id, wgeo)
            })
            .collect();
        state.old_damage.push_front(new_damage);

        // Return if we actually rendered something
        Ok(!damage.is_empty())
    }

    pub fn send_frames(&self, all: bool, time: u32) {
        for window in self.windows.iter().filter(|w| {
            all || {
                let mut state = window_state(self.id, w);
                std::mem::replace(&mut state.drawn, false)
            }
        }) {
            window.send_frame(time);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError<R: Renderer> {
    #[error(transparent)]
    Rendering(R::Error),
    #[error("Output has no active mode")]
    OutputNoMode,
}

fn window_geo(window: &Window, space_id: &usize) -> Rectangle<i32, Logical> {
    let loc = window_loc(window, space_id);
    let mut wgeo = window.geometry();
    wgeo.loc += loc;
    wgeo
}

fn window_rect(window: &Window, space_id: &usize) -> Rectangle<i32, Logical> {
    let loc = window_loc(window, space_id);
    let mut wgeo = window.bbox();
    wgeo.loc += loc;
    wgeo
}

fn window_loc(window: &Window, space_id: &usize) -> Point<i32, Logical> {
    window
        .user_data()
        .get::<RefCell<HashMap<usize, WindowState>>>()
        .unwrap()
        .borrow()
        .get(space_id)
        .unwrap()
        .location
}
