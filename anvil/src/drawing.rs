use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::Sender,
};

use slog::Logger;
use smithay::{
    backend::SwapBuffersError,
    backend::renderer::{Renderer, Transform, Texture},
    reexports::{
        calloop::LoopHandle,
        wayland_server::protocol::wl_surface,
    },
    utils::Rectangle,
    wayland::{
        compositor::{roles::Role, SubsurfaceRole, TraversalAction},
        data_device::DnDIconRole,
        seat::CursorImageRole,
    },
};

use crate::shell::{MyCompositorToken, MyWindowMap, SurfaceData};
use crate::buffer_utils::{BufferUtils, BufferTextures};

pub fn draw_cursor<R, E, T>(
    renderer: &mut R,
    renderer_id: u64,
    texture_destruction_callback: &Sender<T>,
    buffer_utils: &BufferUtils,
    surface: &wl_surface::WlSurface,
    (x, y): (i32, i32),
    token: MyCompositorToken,
    log: &Logger,
)
    where
        R: Renderer<Error=E, TextureId=T>,
        E: std::error::Error,
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
    draw_surface_tree(renderer, renderer_id, texture_destruction_callback, buffer_utils, surface, (x - dx, y - dy), token, log);
}

fn draw_surface_tree<R, E, T>(
    renderer: &mut R,
    renderer_id: u64,
    texture_destruction_callback: &Sender<T>,
    buffer_utils: &BufferUtils,
    root: &wl_surface::WlSurface,
    location: (i32, i32),
    compositor_token: MyCompositorToken,
    log: &Logger,
)
    where
        R: Renderer<Error=E, TextureId=T>,
        E: std::error::Error,
        T: Texture + 'static,
{
    compositor_token.with_surface_tree_upward(
        root,
        (),
        |_surface, attributes, _, _| {
            // Pull a new buffer if available
            if let Some(data) = attributes.user_data.get::<RefCell<SurfaceData>>() {
                let mut data = data.borrow_mut();
                if data.texture.is_none() {
                    if let Some(buffer) = data.current_state.buffer.take() {
                        match buffer_utils.load_buffer::<R::TextureId>(buffer) {
                            Ok(m) => data.texture = Some(Box::new(m) as Box<dyn std::any::Any + 'static>),
                            // there was an error reading the buffer, release it, we
                            // already logged the error
                            Err(err) => {
                                warn!(log, "Error loading buffer: {:?}", err);
                            },
                        };
                    }
                }
                // Now, should we be drawn ?
                if data.texture.is_some() {
                    TraversalAction::DoChildren(())
                } else {
                    // we are not displayed, so our children are neither
                    TraversalAction::SkipChildren
                }
            } else {
                // we are not displayed, so our children are neither
                TraversalAction::SkipChildren
            }
        },
        |_, _, _, _| {},
        |_, _, _, _| true,
    );

    compositor_token.with_surface_tree_upward(
        root,
        location,
        |_surface, attributes, role, &(mut x, mut y)| {
            // Pull a new buffer if available
            if let Some(data) = attributes.user_data.get::<RefCell<SurfaceData>>() {
                let data = data.borrow();
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
                if let Some(buffer_textures) = data.texture.as_mut().and_then(|x| x.downcast_mut::<BufferTextures<T>>()) {
                    // we need to re-extract the subsurface offset, as the previous closure
                    // only passes it to our children
                    if Role::<SubsurfaceRole>::has(role) {
                        x += sub_x;
                        y += sub_y;
                    }
                    let texture = buffer_textures.load_texture(renderer_id, renderer, texture_destruction_callback).unwrap();
                    renderer.render_texture_at(texture, (x, y), Transform::Normal /* TODO */, 1.0);
                }
            }
        },
        |_, _, _, _| true,
    );
}

pub fn draw_windows<R, E, T>(
    renderer: &mut R,
    renderer_id: u64,
    texture_destruction_callback: &Sender<T>,
    buffer_utils: &BufferUtils,
    window_map: &MyWindowMap,
    output_rect: Option<Rectangle>,
    compositor_token: MyCompositorToken,
    log: &::slog::Logger,
)
    where
        R: Renderer<Error=E, TextureId=T>,
        E: std::error::Error,
        T: Texture + 'static,
{
    // redraw the frame, in a simple but inneficient way
    {
        window_map.with_windows_from_bottom_to_top(
            |toplevel_surface, mut initial_place, bounding_box| {
                // skip windows that do not overlap with a given output
                if let Some(output) = output_rect {
                    if !output.overlaps(bounding_box) {
                        return;
                    }
                    initial_place.0 -= output.x;
                }
                if let Some(wl_surface) = toplevel_surface.get_surface() {
                    // this surface is a root of a subsurface tree that needs to be drawn
                    draw_surface_tree(
                        renderer,
                        renderer_id,
                        texture_destruction_callback,
                        buffer_utils,
                        &wl_surface,
                        initial_place,
                        compositor_token,
                        log,
                    );
                }
            },
        );
    }
}

pub fn draw_dnd_icon<R, E, T>(
    renderer: &mut R,
    renderer_id: u64,
    texture_destruction_callback: &Sender<T>,
    buffer_utils: &BufferUtils,
    surface: &wl_surface::WlSurface,
    (x, y): (i32, i32),
    token: MyCompositorToken,
    log: &::slog::Logger,
)
    where
        R: Renderer<Error=E, TextureId=T>,
        E: std::error::Error,
        T: Texture + 'static,
{
    if !token.has_role::<DnDIconRole>(surface) {
        warn!(
            log,
            "Trying to display as a dnd icon a surface that does not have the DndIcon role."
        );
    }
    draw_surface_tree(renderer, renderer_id, texture_destruction_callback, buffer_utils, surface, (x, y), token, log);
}

pub fn schedule_initial_render<R: Renderer + 'static, Data: 'static>(
    renderer: Rc<RefCell<R>>,
    evt_handle: &LoopHandle<Data>,
    logger: ::slog::Logger,
)
where
    <R as Renderer>::Error: Into<SwapBuffersError>
{
    let result = {
        let mut renderer = renderer.borrow_mut();
        // Does not matter if we render an empty frame
        renderer.begin(1, 1, Transform::Normal).map_err(Into::<SwapBuffersError>::into)
        .and_then(|_| renderer.clear([0.8, 0.8, 0.9, 1.0]).map_err(Into::<SwapBuffersError>::into))
        .and_then(|_| renderer.finish())
    };
    if let Err(err) = result {
        match err {
            SwapBuffersError::AlreadySwapped => {}
            SwapBuffersError::TemporaryFailure(err) => {
                // TODO dont reschedule after 3(?) retries
                warn!(logger, "Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |_| schedule_initial_render(renderer, &handle, logger));
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}
