#![allow(clippy::too_many_arguments)]

use std::cell::RefCell;

use slog::Logger;
use smithay::{
    backend::{
        renderer::{Frame, Renderer, Texture, Transform, ImportShm, BufferType, buffer_type},
        SwapBuffersError,
    },
    reexports::wayland_server::protocol::{wl_buffer, wl_surface},
    utils::Rectangle,
    wayland::{
        compositor::{roles::Role, Damage, SubsurfaceRole, TraversalAction},
        data_device::DnDIconRole,
        seat::CursorImageRole,
    },
};
#[cfg(feature = "egl")]
use smithay::backend::{
    egl::display::EGLBufferReader,
    renderer::ImportEgl
};
// hacky...
#[cfg(not(feature = "egl"))]
pub trait ImportEgl {}
#[cfg(not(feature = "egl"))]
impl<T> ImportEgl for T {}

use crate::shell::{MyCompositorToken, MyWindowMap, SurfaceData};

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
    #[cfg(feature = "egl")]
    egl_buffer_reader: Option<&EGLBufferReader>,
    (x, y): (i32, i32),
    token: MyCompositorToken,
    log: &Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportShm + ImportEgl,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error + Into<SwapBuffersError>,
    T: Texture + 'static,
{
    let (dx, dy) = match token.with_role_data::<CursorImageRole, _, _>(surface, |data| data.hotspot) {
        Ok(h) => h,
        Err(_) => {
            warn!(
                log,
                "Trying to display as a cursor a surface that does not have the CursorImage role."
            );
            (0, 0)
        }
    };
    draw_surface_tree(
        renderer,
        frame,
        surface,
        #[cfg(feature = "egl")]
        egl_buffer_reader,
        (x - dx, y - dy),
        token,
        log,
    )
}

fn draw_surface_tree<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    root: &wl_surface::WlSurface,
    #[cfg(feature = "egl")]
    egl_buffer_reader: Option<&EGLBufferReader>,
    location: (i32, i32),
    compositor_token: MyCompositorToken,
    log: &Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportShm + ImportEgl,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error + Into<SwapBuffersError>,
    T: Texture + 'static,
{
    let mut result = Ok(());

    compositor_token.with_surface_tree_upward(
        root,
        location,
        |_surface, attributes, role, &(mut x, mut y)| {
            // Pull a new buffer if available
            if let Some(data) = attributes.user_data.get::<RefCell<SurfaceData>>() {
                let mut data = data.borrow_mut();
                if data.texture.is_none() {
                    if let Some(buffer) = data.current_state.buffer.take() {
                        let texture = match buffer_type(
                            &buffer,
                            #[cfg(feature = "egl")]
                            egl_buffer_reader
                        ) {
                            Some(BufferType::Shm) => {
                                let damage = attributes
                                    .damage
                                    .iter()
                                    .map(|dmg| match dmg {
                                        Damage::Buffer(rect) => *rect,
                                        // TODO also apply transformations
                                        Damage::Surface(rect) => rect.scale(attributes.buffer_scale),
                                    })
                                    .collect::<Vec<_>>();
                                let result = renderer.import_shm_buffer(&buffer, Some(&attributes), &damage);
                                if result.is_ok() {
                                    buffer.release();
                                }
                                Some(result)
                            },
                            #[cfg(feature = "egl")]
                            Some(BufferType::Egl) => Some(renderer.import_egl_buffer(&buffer, egl_buffer_reader.unwrap())),
                            _ => {
                                error!(log, "Unknown buffer format for: {:?}", buffer);
                                None
                            }
                        };
                        match texture {
                            Some(Ok(m)) => {
                                data.texture = Some(Box::new(BufferTextures { buffer: Some(buffer), texture: m })
                                    as Box<dyn std::any::Any + 'static>)
                            }
                            // there was an error reading the buffer, release it.
                            Some(Err(err)) => {
                                warn!(log, "Error loading buffer: {:?}", err);
                                buffer.release();
                            },
                            None => buffer.release(),
                        };
                    }
                }
                // Now, should we be drawn ?
                if data.texture.is_some() {
                    // if yes, also process the children
                    if Role::<SubsurfaceRole>::has(role) {
                        x += data.current_state.sub_location.0;
                        y += data.current_state.sub_location.1;
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
        |_surface, attributes, role, &(mut x, mut y)| {
            if let Some(ref data) = attributes.user_data.get::<RefCell<SurfaceData>>() {
                let mut data = data.borrow_mut();
                let (sub_x, sub_y) = data.current_state.sub_location;
                if let Some(texture) = data
                    .texture
                    .as_mut()
                    .and_then(|x| x.downcast_mut::<BufferTextures<T>>())
                {
                    // we need to re-extract the subsurface offset, as the previous closure
                    // only passes it to our children
                    if Role::<SubsurfaceRole>::has(role) {
                        x += sub_x;
                        y += sub_y;
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
        |_, _, _, _| true,
    );

    result
}

pub fn draw_windows<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    #[cfg(feature = "egl")]
    egl_buffer_reader: Option<&EGLBufferReader>,
    window_map: &MyWindowMap,
    output_rect: Option<Rectangle>,
    compositor_token: MyCompositorToken,
    log: &::slog::Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportShm + ImportEgl,
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
            if let Err(err) = draw_surface_tree(
                renderer,
                frame,
                &wl_surface,
                #[cfg(feature = "egl")]
                egl_buffer_reader,
                initial_place,
                compositor_token,
                log,
            ) {
                result = Err(err);
            }
        }
    });

    result
}

pub fn draw_dnd_icon<R, E, F, T>(
    renderer: &mut R,
    frame: &mut F,
    surface: &wl_surface::WlSurface,
    #[cfg(feature = "egl")]
    egl_buffer_reader: Option<&EGLBufferReader>,
    (x, y): (i32, i32),
    token: MyCompositorToken,
    log: &::slog::Logger,
) -> Result<(), SwapBuffersError>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportShm + ImportEgl,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error + Into<SwapBuffersError>,
    T: Texture + 'static,
{
    if !token.has_role::<DnDIconRole>(surface) {
        warn!(
            log,
            "Trying to display as a dnd icon a surface that does not have the DndIcon role."
        );
    }
    draw_surface_tree(
        renderer,
        frame,
        surface,
        #[cfg(feature = "egl")]
        egl_buffer_reader,
        (x, y),
        token,
        log
    )
}
