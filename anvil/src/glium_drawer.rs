use std::{
    cell::{Ref, RefCell},
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use glium::{self, index::PrimitiveType, texture::Texture2d, Surface};
use slog::Logger;

use smithay::{
    backend::graphics::{
        gl::GLGraphicsBackend,
        glium::{Frame, GliumGraphicsBackend},
        CursorBackend, SwapBuffersError,
    },
    reexports::{calloop::LoopHandle, wayland_server::protocol::wl_surface},
    utils::Rectangle,
    wayland::{
        compositor::{roles::Role, SubsurfaceRole, TraversalAction},
        data_device::DnDIconRole,
        seat::CursorImageRole,
    },
};

use crate::buffer_utils::BufferUtils;
use crate::shaders;
use crate::shell::{MyCompositorToken, MyWindowMap, SurfaceData};

pub static BACKEND_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Copy, Clone)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
}

mod implement_vertex {
    #![allow(clippy::unneeded_field_pattern)]
    // Module to scope the clippy lint disabling
    use super::Vertex;
    implement_vertex!(Vertex, position, tex_coords);
}

pub struct GliumDrawer<F: GLGraphicsBackend + 'static> {
    pub id: usize,
    pub display: GliumGraphicsBackend<F>,
    vertex_buffer: glium::VertexBuffer<Vertex>,
    index_buffer: glium::IndexBuffer<u16>,
    programs: [glium::Program; shaders::FRAGMENT_COUNT],
    buffer_loader: BufferUtils,
    pub hardware_cursor: AtomicBool,
    log: Logger,
}

impl<F: GLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn borrow(&self) -> Ref<'_, F> {
        self.display.borrow()
    }
}

impl<T: Into<GliumGraphicsBackend<T>> + GLGraphicsBackend + 'static> GliumDrawer<T> {
    pub fn init(backend: T, buffer_loader: BufferUtils, log: Logger) -> GliumDrawer<T> {
        let display = backend.into();

        // building the vertex buffer, which contains all the vertices that we will draw
        let vertex_buffer = glium::VertexBuffer::new(
            &display,
            &[
                Vertex {
                    position: [0.0, 0.0],
                    tex_coords: [0.0, 0.0],
                },
                Vertex {
                    position: [0.0, 1.0],
                    tex_coords: [0.0, 1.0],
                },
                Vertex {
                    position: [1.0, 1.0],
                    tex_coords: [1.0, 1.0],
                },
                Vertex {
                    position: [1.0, 0.0],
                    tex_coords: [1.0, 0.0],
                },
            ],
        )
        .unwrap();

        // building the index buffer
        let index_buffer =
            glium::IndexBuffer::new(&display, PrimitiveType::TriangleStrip, &[1 as u16, 2, 0, 3]).unwrap();

        let programs = opengl_programs!(&display);

        GliumDrawer {
            id: BACKEND_COUNTER.fetch_add(1, Ordering::AcqRel),
            display,
            vertex_buffer,
            index_buffer,
            programs,
            buffer_loader,
            hardware_cursor: AtomicBool::new(false),
            log,
        }
    }
}

impl<F: GLGraphicsBackend + CursorBackend + 'static> GliumDrawer<F> {
    pub fn draw_hardware_cursor(
        &self,
        cursor: &<F as CursorBackend>::CursorFormat,
        hotspot: (u32, u32),
        position: (i32, i32),
    ) {
        let (x, y) = position;
        let _ = self.display.borrow().set_cursor_position(x as u32, y as u32);
        if !self.hardware_cursor.swap(true, Ordering::SeqCst)
            && self
                .display
                .borrow()
                .set_cursor_representation(cursor, hotspot)
                .is_err()
        {
            warn!(self.log, "Failed to upload hardware cursor",);
        }
    }

    pub fn draw_software_cursor(
        &self,
        frame: &mut Frame,
        surface: &wl_surface::WlSurface,
        (x, y): (i32, i32),
        token: MyCompositorToken,
    ) {
        let (dx, dy) = match token.with_role_data::<CursorImageRole, _, _>(surface, |data| data.hotspot) {
            Ok(h) => h,
            Err(_) => {
                warn!(
                    self.log,
                    "Trying to display as a cursor a surface that does not have the CursorImage role."
                );
                (0, 0)
            }
        };
        let screen_dimensions = self.borrow().get_framebuffer_dimensions();
        self.draw_surface_tree(frame, surface, (x - dx, y - dy), token, screen_dimensions);
        self.clear_cursor()
    }

    pub fn clear_cursor(&self) {
        if self.hardware_cursor.swap(false, Ordering::SeqCst)
            && self.display.borrow().clear_cursor_representation().is_err()
        {
            warn!(self.log, "Failed to clear cursor");
        }
    }
}

// I would love to do this (check on !CursorBackend), but this is essentially specialization...
// And since this is just an example compositor, it seems we require now,
// that for the use of software cursors we need the hardware cursor trait (to do automatic cleanup..)
/*
impl<F: GLGraphicsBackend + !CursorBackend + 'static> GliumDrawer<F> {
    pub fn draw_software_cursor(
        &self,
        frame: &mut Frame,
        surface: &wl_surface::WlSurface,
        (x, y): (i32, i32),
        token: MyCompositorToken,
    ) {
        let (dx, dy) = match token.with_role_data::<CursorImageRole, _, _>(surface, |data| data.hotspot) {
            Ok(h) => h,
            Err(_) => {
                warn!(
                    self.log,
                    "Trying to display as a cursor a surface that does not have the CursorImage role."
                );
                (0, 0)
            }
        };
        let screen_dimensions = self.borrow().get_framebuffer_dimensions();
        self.draw_surface_tree(frame, surface, (x - dx, y - dy), token, screen_dimensions);
    }
}
*/

impl<F: GLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn render_texture(&self, target: &mut Frame, spec: RenderTextureSpec<'_>) {
        let xscale = 2.0 * (spec.surface_dimensions.0 as f32) / (spec.screen_size.0 as f32);
        let mut yscale = -2.0 * (spec.surface_dimensions.1 as f32) / (spec.screen_size.1 as f32);

        let x = 2.0 * (spec.surface_location.0 as f32) / (spec.screen_size.0 as f32) - 1.0;
        let mut y = 1.0 - 2.0 * (spec.surface_location.1 as f32) / (spec.screen_size.1 as f32);

        if spec.y_inverted {
            yscale = -yscale;
            y -= spec.surface_dimensions.1 as f32;
        }

        let uniforms = uniform! {
            matrix: [
                [xscale,   0.0  , 0.0, 0.0],
                [  0.0 , yscale , 0.0, 0.0],
                [  0.0 ,   0.0  , 1.0, 0.0],
                [   x  ,    y   , 0.0, 1.0]
            ],
            tex: spec.texture,
        };

        target
            .draw(
                &self.vertex_buffer,
                &self.index_buffer,
                &self.programs[spec.texture_kind],
                &uniforms,
                &glium::DrawParameters {
                    blend: spec.blending,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    #[inline]
    pub fn draw(&self) -> Frame {
        self.display.draw()
    }
}

pub struct RenderTextureSpec<'a> {
    texture: &'a Texture2d,
    texture_kind: usize,
    y_inverted: bool,
    surface_dimensions: (u32, u32),
    surface_location: (i32, i32),
    screen_size: (u32, u32),
    blending: glium::Blend,
}

impl<F: GLGraphicsBackend + 'static> GliumDrawer<F> {
    fn draw_surface_tree(
        &self,
        frame: &mut Frame,
        root: &wl_surface::WlSurface,
        location: (i32, i32),
        compositor_token: MyCompositorToken,
        screen_dimensions: (u32, u32),
    ) {
        compositor_token.with_surface_tree_upward(
            root,
            location,
            |_surface, attributes, role, &(mut x, mut y)| {
                // Pull a new buffer if available
                if let Some(data) = attributes.user_data.get::<RefCell<SurfaceData>>() {
                    let mut data = data.borrow_mut();
                    if data.texture.is_none() {
                        if let Some(buffer) = data.current_state.buffer.take() {
                            match self.buffer_loader.load_buffer(buffer) {
                                Ok(m) => data.texture = Some(m),
                                // there was an error reading the buffer, release it, we
                                // already logged the error
                                Err(buffer) => buffer.release(),
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
                    if let Some(buffer_textures) = data.texture.as_mut() {
                        let texture_kind = buffer_textures.fragment;
                        let y_inverted = buffer_textures.y_inverted;
                        let surface_dimensions = buffer_textures.dimensions;
                        if let Ok(ref texture) = buffer_textures.load_texture(&self) {
                            // we need to re-extract the subsurface offset, as the previous closure
                            // only passes it to our children
                            if Role::<SubsurfaceRole>::has(role) {
                                x += sub_x;
                                y += sub_y;
                            }
                            self.render_texture(
                                frame,
                                RenderTextureSpec {
                                    texture: &texture,
                                    texture_kind,
                                    y_inverted,
                                    surface_dimensions,
                                    surface_location: (x, y),
                                    screen_size: screen_dimensions,
                                    blending: ::glium::Blend {
                                        color: ::glium::BlendingFunction::Addition {
                                            source: ::glium::LinearBlendingFactor::One,
                                            destination: ::glium::LinearBlendingFactor::OneMinusSourceAlpha,
                                        },
                                        alpha: ::glium::BlendingFunction::Addition {
                                            source: ::glium::LinearBlendingFactor::One,
                                            destination: ::glium::LinearBlendingFactor::OneMinusSourceAlpha,
                                        },
                                        ..Default::default()
                                    },
                                },
                            );
                        }
                    }
                }
            },
            |_, _, _, _| true,
        );
    }

    pub fn draw_windows(
        &self,
        frame: &mut Frame,
        window_map: &MyWindowMap,
        output_rect: Option<Rectangle>,
        compositor_token: MyCompositorToken,
    ) {
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = self.borrow().get_framebuffer_dimensions();
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
                        self.draw_surface_tree(
                            frame,
                            &wl_surface,
                            initial_place,
                            compositor_token,
                            screen_dimensions,
                        );
                    }
                },
            );
        }
    }

    pub fn draw_dnd_icon(
        &self,
        frame: &mut Frame,
        surface: &wl_surface::WlSurface,
        (x, y): (i32, i32),
        token: MyCompositorToken,
    ) {
        if !token.has_role::<DnDIconRole>(surface) {
            warn!(
                self.log,
                "Trying to display as a dnd icon a surface that does not have the DndIcon role."
            );
        }
        let screen_dimensions = self.borrow().get_framebuffer_dimensions();
        self.draw_surface_tree(frame, surface, (x, y), token, screen_dimensions);
    }
}

pub fn schedule_initial_render<F: GLGraphicsBackend + 'static, Data: 'static>(
    renderer: Rc<GliumDrawer<F>>,
    evt_handle: &LoopHandle<Data>,
) {
    let mut frame = renderer.draw();
    frame.clear_color(0.8, 0.8, 0.9, 1.0);
    if let Err(err) = frame.set_finish() {
        match err {
            SwapBuffersError::AlreadySwapped => {}
            SwapBuffersError::TemporaryFailure(err) => {
                // TODO dont reschedule after 3(?) retries
                warn!(renderer.log, "Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |_| schedule_initial_render(renderer, &handle));
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}
