use std::{
    cell::{Ref, RefCell},
    rc::Rc,
};

use glium::{
    self,
    index::PrimitiveType,
    texture::{MipmapsOption, Texture2d, UncompressedFloatFormat},
    GlObject, Surface,
};
use slog::Logger;

#[cfg(feature = "egl")]
use smithay::backend::egl::display::EGLBufferReader;
use smithay::{
    backend::{
        egl::{BufferAccessError, EGLImages, Format},
        graphics::{
            gl::GLGraphicsBackend,
            glium::{Frame, GliumGraphicsBackend},
            SwapBuffersError,
        },
    },
    reexports::{
        calloop::LoopHandle,
        wayland_server::protocol::{wl_buffer, wl_surface},
    },
    wayland::{
        compositor::{roles::Role, SubsurfaceRole, TraversalAction},
        data_device::DnDIconRole,
        seat::CursorImageRole,
        shm::with_buffer_contents as shm_buffer_contents,
    },
};

use crate::shaders;
use crate::shell::{MyCompositorToken, MyWindowMap, SurfaceData};

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
    display: GliumGraphicsBackend<F>,
    vertex_buffer: glium::VertexBuffer<Vertex>,
    index_buffer: glium::IndexBuffer<u16>,
    programs: [glium::Program; shaders::FRAGMENT_COUNT],
    #[cfg(feature = "egl")]
    egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>,
    log: Logger,
}

impl<F: GLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn borrow(&self) -> Ref<'_, F> {
        self.display.borrow()
    }
}

impl<T: Into<GliumGraphicsBackend<T>> + GLGraphicsBackend + 'static> GliumDrawer<T> {
    #[cfg(feature = "egl")]
    pub fn init(
        backend: T,
        egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>,
        log: Logger,
    ) -> GliumDrawer<T> {
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
            display,
            vertex_buffer,
            index_buffer,
            programs,
            egl_buffer_reader,
            log,
        }
    }

    #[cfg(not(feature = "egl"))]
    pub fn init(backend: T, log: Logger) -> GliumDrawer<T> {
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
            display,
            vertex_buffer,
            index_buffer,
            programs,
            log,
        }
    }
}

impl<F: GLGraphicsBackend + 'static> GliumDrawer<F> {
    #[cfg(feature = "egl")]
    pub fn texture_from_buffer(&self, buffer: wl_buffer::WlBuffer) -> Result<TextureMetadata, ()> {
        // try to retrieve the egl contents of this buffer
        let images = if let Some(display) = &self.egl_buffer_reader.borrow().as_ref() {
            display.egl_buffer_contents(buffer)
        } else {
            Err(BufferAccessError::NotManaged(
                buffer,
                smithay::backend::egl::EGLError::BadDisplay,
            ))
        };
        match images {
            Ok(images) => {
                // we have an EGL buffer
                let format = match images.format {
                    Format::RGB => UncompressedFloatFormat::U8U8U8,
                    Format::RGBA => UncompressedFloatFormat::U8U8U8U8,
                    _ => {
                        warn!(self.log, "Unsupported EGL buffer format"; "format" => format!("{:?}", images.format));
                        return Err(());
                    }
                };
                let opengl_texture = Texture2d::empty_with_format(
                    &self.display,
                    format,
                    MipmapsOption::NoMipmap,
                    images.width,
                    images.height,
                )
                .unwrap();
                unsafe {
                    images
                        .bind_to_texture(0, opengl_texture.get_id(), &*self.display.borrow())
                        .expect("Failed to bind to texture");
                }
                Ok(TextureMetadata {
                    texture: opengl_texture,
                    fragment: crate::shaders::BUFFER_RGBA,
                    y_inverted: images.y_inverted,
                    dimensions: (images.width, images.height),
                    images: Some(images), // I guess we need to keep this alive ?
                })
            }
            Err(BufferAccessError::NotManaged(buffer, _)) => {
                // this is not an EGL buffer, try SHM
                self.texture_from_shm_buffer(buffer)
            }
            Err(err) => {
                error!(self.log, "EGL error"; "err" => format!("{:?}", err));
                Err(())
            }
        }
    }

    #[cfg(not(feature = "egl"))]
    pub fn texture_from_buffer(&self, buffer: wl_buffer::WlBuffer) -> Result<TextureMetadata, ()> {
        self.texture_from_shm_buffer(buffer)
    }

    fn texture_from_shm_buffer(&self, buffer: wl_buffer::WlBuffer) -> Result<TextureMetadata, ()> {
        match shm_buffer_contents(&buffer, |slice, data| {
            crate::shm_load::load_shm_buffer(data, slice)
                .map(|(image, kind)| (Texture2d::new(&self.display, image).unwrap(), kind, data))
        }) {
            Ok(Ok((texture, kind, data))) => Ok(TextureMetadata {
                texture,
                fragment: kind,
                y_inverted: false,
                dimensions: (data.width as u32, data.height as u32),
                #[cfg(feature = "egl")]
                images: None,
            }),
            Ok(Err(format)) => {
                warn!(self.log, "Unsupported SHM buffer format"; "format" => format!("{:?}", format));
                Err(())
            }
            Err(err) => {
                warn!(self.log, "Unable to load buffer contents"; "err" => format!("{:?}", err));
                Err(())
            }
        }
    }

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

pub struct TextureMetadata {
    pub texture: Texture2d,
    pub fragment: usize,
    pub y_inverted: bool,
    pub dimensions: (u32, u32),
    #[cfg(feature = "egl")]
    images: Option<EGLImages>,
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
                            if let Ok(m) = self.texture_from_buffer(buffer.clone()) {
                                // release the buffer if it was an SHM buffer
                                #[cfg(feature = "egl")]
                                {
                                    if m.images.is_none() {
                                        buffer.release();
                                    }
                                }
                                #[cfg(not(feature = "egl"))]
                                {
                                    buffer.release();
                                }

                                data.texture = Some(m);
                            } else {
                                // there was an error reading the buffer, release it, we
                                // already logged the error
                                buffer.release();
                            }
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
                    let data = data.borrow();
                    if let Some(ref metadata) = data.texture {
                        // we need to re-extract the subsurface offset, as the previous closure
                        // only passes it to our children
                        if Role::<SubsurfaceRole>::has(role) {
                            x += data.current_state.sub_location.0;
                            y += data.current_state.sub_location.1;
                        }
                        self.render_texture(
                            frame,
                            RenderTextureSpec {
                                texture: &metadata.texture,
                                texture_kind: metadata.fragment,
                                y_inverted: metadata.y_inverted,
                                surface_dimensions: metadata.dimensions,
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
            },
            |_, _, _, _| true,
        );
    }

    pub fn draw_windows(
        &self,
        frame: &mut Frame,
        window_map: &MyWindowMap,
        compositor_token: MyCompositorToken,
    ) {
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = self.borrow().get_framebuffer_dimensions();
            window_map.with_windows_from_bottom_to_top(|toplevel_surface, initial_place| {
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
            });
        }
    }

    pub fn draw_cursor(
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
                error!(renderer.log, "Failed to submit page_flip: {}", err);
                let handle = evt_handle.clone();
                evt_handle.insert_idle(move |_| schedule_initial_render(renderer, &handle));
            }
            SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
        }
    }
}
