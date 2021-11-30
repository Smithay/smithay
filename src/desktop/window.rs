use crate::{
    backend::renderer::{buffer_dimensions, Frame, ImportAll, Renderer, Texture},
    utils::{Logical, Physical, Point, Rectangle, Size},
    wayland::{
        compositor::{
            add_commit_hook, is_sync_subsurface, with_states, with_surface_tree_downward,
            with_surface_tree_upward, BufferAssignment, Damage, SubsurfaceCachedState, SurfaceAttributes,
            TraversalAction,
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

#[derive(Default)]
pub(super) struct SurfaceState {
    buffer_dimensions: Option<Size<i32, Physical>>,
    buffer_scale: i32,
    buffer: Option<wl_buffer::WlBuffer>,
    texture: Option<Box<dyn std::any::Any + 'static>>,
}

fn surface_commit(surface: &wl_surface::WlSurface) {
    if !is_sync_subsurface(surface) {
        with_surface_tree_upward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_surf, states, _| {
                states
                    .data_map
                    .insert_if_missing(|| RefCell::new(SurfaceState::default()));
                let mut data = states
                    .data_map
                    .get::<RefCell<SurfaceState>>()
                    .unwrap()
                    .borrow_mut();
                data.update_buffer(&mut *states.cached_state.current::<SurfaceAttributes>());
            },
            |_, _, _| true,
        );
    }
}

impl SurfaceState {
    pub fn update_buffer(&mut self, attrs: &mut SurfaceAttributes) {
        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer { buffer, .. }) => {
                // new contents
                self.buffer_dimensions = buffer_dimensions(&buffer);
                self.buffer_scale = attrs.buffer_scale;
                if let Some(old_buffer) = std::mem::replace(&mut self.buffer, Some(buffer)) {
                    if &old_buffer != self.buffer.as_ref().unwrap() {
                        old_buffer.release();
                    }
                }
                self.texture = None;
            }
            Some(BufferAssignment::Removed) => {
                // remove the contents
                self.buffer_dimensions = None;
                if let Some(buffer) = self.buffer.take() {
                    buffer.release();
                };
                self.texture = None;
            }
            None => {}
        }
    }

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
        let mut bounding_box = Rectangle::from_loc_and_size((0, 0), (0, 0));
        if let Some(wl_surface) = self.0.toplevel.get_surface() {
            with_surface_tree_downward(
                wl_surface,
                (0, 0).into(),
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
        if let Some(wl_surface) = self.0.toplevel.get_surface() {
            with_surface_tree_downward(
                wl_surface,
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
    }

    /// Finds the topmost surface under this point if any and returns it together with the location of this
    /// surface.
    pub fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)> {
        let found = RefCell::new(None);
        if let Some(wl_surface) = self.0.toplevel.get_surface() {
            with_surface_tree_downward(
                wl_surface,
                (0, 0).into(),
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
        }
        found.into_inner()
    }

    /// Damage of all the surfaces of this window
    pub(super) fn accumulated_damage(&self) -> Vec<Rectangle<i32, Logical>> {
        let mut damage = Vec::new();
        let location = (0, 0).into();
        if let Some(surface) = self.0.toplevel.get_surface() {
            with_surface_tree_upward(
                surface,
                location,
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
            )
        }
        damage
    }

    pub fn toplevel(&self) -> &Kind {
        &self.0.toplevel
    }

    pub fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }

    /// Has to be called on commit - Window handles the buffer for you
    pub fn commit(surface: &wl_surface::WlSurface) {
        surface_commit(surface)
    }
}

// TODO: This is basically `draw_surface_tree` from anvil.
// Can we move this somewhere, where it is also usable for other things then windows?
// Maybe add this as a helper function for surfaces to `backend::renderer`?
// How do we handle SurfaceState in that case? Like we need a closure to
// store and retrieve textures for arbitrary surface trees? Or leave this to the
// compositor, but that feels like a lot of unnecessary code dublication.

// TODO: This does not handle ImportAll errors properly and uses only one texture slot.
// This means it is *not* compatible with MultiGPU setups at all.
// Current plan is to make sure it does not crash at least in that case and later add
// A `MultiGpuManager` that opens gpus automatically, creates renderers for them,
// implements `Renderer` and `ImportAll` itself and dispatches everything accordingly,
// even copying buffers if necessary. This abstraction will likely also handle dmabuf-
// protocol(s) (and maybe explicit sync?). Then this function will be fine and all the
// gore of handling multiple gpus will be hidden away for most if not all cases.

// TODO: This function does not crop or scale windows to fit into a space.
// How do we want to handle this? Add an additional size property to a window?
// Let the user specify the max size and the method to handle it?

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
    let mut result = Ok(());
    if let Some(surface) = window.0.toplevel.get_surface() {
        with_surface_tree_upward(
            surface,
            location,
            |_surface, states, location| {
                let mut location = *location;
                if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                    let mut data = data.borrow_mut();
                    let attributes = states.cached_state.current::<SurfaceAttributes>();
                    // Import a new buffer if necessary
                    if data.texture.is_none() {
                        if let Some(buffer) = data.buffer.as_ref() {
                            let damage = attributes
                                .damage
                                .iter()
                                .map(|dmg| match dmg {
                                    Damage::Buffer(rect) => *rect,
                                    // TODO also apply transformations
                                    Damage::Surface(rect) => rect.to_buffer(attributes.buffer_scale),
                                })
                                .collect::<Vec<_>>();

                            match renderer.import_buffer(buffer, Some(states), &damage) {
                                Some(Ok(m)) => {
                                    data.texture = Some(Box::new(m));
                                }
                                Some(Err(err)) => {
                                    slog::warn!(log, "Error loading buffer: {}", err);
                                }
                                None => {
                                    slog::error!(log, "Unknown buffer format for: {:?}", buffer);
                                }
                            }
                        }
                    }
                    // Now, should we be drawn ?
                    if data.texture.is_some() {
                        // if yes, also process the children
                        if states.role == Some("subsurface") {
                            let current = states.cached_state.current::<SubsurfaceCachedState>();
                            location += current.location;
                        }
                        TraversalAction::DoChildren(location)
                    } else {
                        // we are not displayed, so our children are neither
                        TraversalAction::SkipChildren
                    }
                } else {
                    // we are not displayed, so our children are neither
                    TraversalAction::SkipChildren
                }
            },
            |_surface, states, location| {
                let mut location = *location;
                if let Some(data) = states.data_map.get::<RefCell<SurfaceState>>() {
                    let mut data = data.borrow_mut();
                    let buffer_scale = data.buffer_scale;
                    let attributes = states.cached_state.current::<SurfaceAttributes>();
                    if let Some(texture) = data.texture.as_mut().and_then(|x| x.downcast_mut::<T>()) {
                        // we need to re-extract the subsurface offset, as the previous closure
                        // only passes it to our children
                        if states.role == Some("subsurface") {
                            let current = states.cached_state.current::<SubsurfaceCachedState>();
                            location += current.location;
                        }
                        // TODO: Take wp_viewporter into account
                        if let Err(err) = frame.render_texture_at(
                            texture,
                            location.to_f64().to_physical(scale).to_i32_round(),
                            buffer_scale,
                            scale,
                            attributes.buffer_transform.into(),
                            1.0,
                        ) {
                            result = Err(err);
                        }
                    }
                }
            },
            |_, _, _| true,
        );
    }

    result
}
