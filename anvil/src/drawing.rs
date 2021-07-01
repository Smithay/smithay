#![allow(clippy::too_many_arguments)]

use std::{cell::RefCell, sync::Mutex};

use slog::Logger;
use smithay::{
    backend::{
        renderer::{buffer_type, BufferType, Frame, ImportAll, Renderer, Texture, Transform},
        SwapBuffersError,
    },
    reexports::wayland_server::protocol::{wl_buffer, wl_surface},
    utils::Rectangle,
    wayland::{
        compositor::{
            get_role, with_states, with_surface_tree_upward, Damage, SubsurfaceCachedState,
            SurfaceAttributes, TraversalAction,
        },
        seat::CursorImageAttributes,
    },
};

use crate::{shell::SurfaceData, window_map::WindowMap};

struct BufferTextures<T> {
    buffer: Option<wl_buffer::WlBuffer>,
    texture: T,
}

impl<T> Drop for BufferTextures<T> {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            buffer.release();
        }
    }
}

pub fn draw_cursor<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    surface: &wl_surface::WlSurface,
    (x, y): (i32, i32),
    log: &Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error + Into<SwapBuffersError>,
    T: Texture + 'static,
{
    let ret = with_states(surface, |states| {
        Some(
            states
                .data_map
                .get::<Mutex<CursorImageAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .hotspot,
        )
    })
    .unwrap_or(None);
    let (dx, dy) = match ret {
        Some(h) => h,
        None => {
            warn!(
                log,
                "Trying to display as a cursor a surface that does not have the CursorImage role."
            );
            (0, 0)
        }
    };
    draw_surface_tree(renderer, frame, surface, (x - dx, y - dy), log)
}

fn draw_surface_tree<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    root: &wl_surface::WlSurface,
    location: (i32, i32),
    log: &Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error + Into<SwapBuffersError>,
    T: Texture + 'static,
{
    let mut result = Ok(());

    with_surface_tree_upward(
        root,
        location,
        |_surface, states, &(mut x, mut y)| {
            // Pull a new buffer if available
            if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>() {
                let mut data = data.borrow_mut();
                let attributes = states.cached_state.current::<SurfaceAttributes>();
                if data.texture.is_none() {
                    if let Some(buffer) = data.buffer.take() {
                        let damage = attributes
                            .damage
                            .iter()
                            .map(|dmg| match dmg {
                                Damage::Buffer(rect) => *rect,
                                // TODO also apply transformations
                                Damage::Surface(rect) => rect.scale(attributes.buffer_scale),
                            })
                            .collect::<Vec<_>>();

                        match renderer.import_buffer(&buffer, Some(states), &damage) {
                            Some(Ok(m)) => {
                                let texture_buffer = if let Some(BufferType::Shm) = buffer_type(&buffer) {
                                    buffer.release();
                                    None
                                } else {
                                    Some(buffer)
                                };
                                data.texture = Some(Box::new(BufferTextures {
                                    buffer: texture_buffer,
                                    texture: m,
                                }))
                            }
                            Some(Err(err)) => {
                                warn!(log, "Error loading buffer: {:?}", err);
                                buffer.release();
                            }
                            None => {
                                error!(log, "Unknown buffer format for: {:?}", buffer);
                                buffer.release();
                            }
                        }
                    }
                }
                // Now, should we be drawn ?
                if data.texture.is_some() {
                    // if yes, also process the children
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        x += current.location.0;
                        y += current.location.1;
                    }
                    TraversalAction::DoChildren((x, y))
                } else {
                    // we are not displayed, so our children are neither
                    TraversalAction::SkipChildren
                }
            } else {
                // we are not displayed, so our children are neither
                TraversalAction::SkipChildren
            }
        },
        |_surface, states, &(mut x, mut y)| {
            if let Some(ref data) = states.data_map.get::<RefCell<SurfaceData>>() {
                let mut data = data.borrow_mut();
                if let Some(texture) = data
                    .texture
                    .as_mut()
                    .and_then(|x| x.downcast_mut::<BufferTextures<T>>())
                {
                    // we need to re-extract the subsurface offset, as the previous closure
                    // only passes it to our children
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        x += current.location.0;
                        y += current.location.1;
                    }
                    if let Err(err) = frame.render_texture_at(
                        &texture.texture,
                        (x, y),
                        Transform::Normal, /* TODO */
                        1.0,
                    ) {
                        result = Err(err.into());
                    }
                }
            }
        },
        |_, _, _| true,
    );

    result
}

pub fn draw_windows<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    window_map: &WindowMap,
    output_rect: Option<Rectangle>,
    log: &::slog::Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error + Into<SwapBuffersError>,
    T: Texture + 'static,
{
    let mut result = Ok(());

    // redraw the frame, in a simple but inneficient way
    window_map.with_windows_from_bottom_to_top(|toplevel_surface, mut initial_place, bounding_box| {
        // skip windows that do not overlap with a given output
        if let Some(output) = output_rect {
            if !output.overlaps(bounding_box) {
                return;
            }
            initial_place.0 -= output.x;
        }
        if let Some(wl_surface) = toplevel_surface.get_surface() {
            // this surface is a root of a subsurface tree that needs to be drawn
            if let Err(err) = draw_surface_tree(renderer, frame, &wl_surface, initial_place, log) {
                result = Err(err);
            }
            // furthermore, draw its popups
            let toplevel_geometry_offset = window_map
                .geometry(toplevel_surface)
                .map(|g| (g.x, g.y))
                .unwrap_or_default();
            window_map.with_child_popups(&wl_surface, |popup| {
                let location = popup.location();
                let draw_location = (
                    initial_place.0 + location.0 + toplevel_geometry_offset.0,
                    initial_place.1 + location.1 + toplevel_geometry_offset.1,
                );
                if let Some(wl_surface) = popup.get_surface() {
                    if let Err(err) = draw_surface_tree(renderer, frame, &wl_surface, draw_location, log) {
                        result = Err(err);
                    }
                }
            });
        }
    });

    result
}

pub fn draw_dnd_icon<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    surface: &wl_surface::WlSurface,
    (x, y): (i32, i32),
    log: &::slog::Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error + Into<SwapBuffersError>,
    T: Texture + 'static,
{
    if get_role(surface) != Some("dnd_icon") {
        warn!(
            log,
            "Trying to display as a dnd icon a surface that does not have the DndIcon role."
        );
    }
    draw_surface_tree(renderer, frame, surface, (x, y), log)
}
