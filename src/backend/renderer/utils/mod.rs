use crate::{
    backend::renderer::{buffer_dimensions, Frame, ImportAll, Renderer, Texture},
    utils::{Logical, Physical, Point, Size},
    wayland::compositor::{
        is_sync_subsurface, with_surface_tree_upward, BufferAssignment, Damage, SubsurfaceCachedState,
        SurfaceAttributes, TraversalAction,
    },
};
use std::cell::RefCell;
use wayland_server::protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface};

#[derive(Default)]
pub(crate) struct SurfaceState {
    pub(crate) buffer_dimensions: Option<Size<i32, Physical>>,
    pub(crate) buffer_scale: i32,
    pub(crate) buffer: Option<WlBuffer>,
    pub(crate) texture: Option<Box<dyn std::any::Any + 'static>>,
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
}

pub fn on_commit_buffer_handler(surface: &WlSurface) {
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

pub fn draw_surface_tree<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    surface: &WlSurface,
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
        |surface, states, location| {
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

    result
}
