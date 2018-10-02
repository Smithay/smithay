use std::{
    cell::{Ref, RefCell},
    rc::Rc,
};

use glium::{
    self,
    index::PrimitiveType,
    texture::{MipmapsOption, Texture2d, UncompressedFloatFormat},
    Frame, GlObject, Surface,
};
use slog::Logger;

use smithay::{
    backend::graphics::{
        egl::{
            error::Result as EGLResult,
            wayland::{BufferAccessError, EGLDisplay, EGLImages, EGLWaylandExtensions, Format},
            EGLGraphicsBackend,
        },
        glium::GliumGraphicsBackend,
    },
    wayland::{
        compositor::{roles::Role, SubsurfaceRole, TraversalAction},
        shm::with_buffer_contents as shm_buffer_contents,
    },
    wayland_server::{protocol::wl_buffer, Display, Resource},
};

use shaders;
use shell::{MyCompositorToken, MyWindowMap};

#[derive(Copy, Clone)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
}

implement_vertex!(Vertex, position, tex_coords);

pub struct GliumDrawer<F: EGLGraphicsBackend + 'static> {
    display: GliumGraphicsBackend<F>,
    vertex_buffer: glium::VertexBuffer<Vertex>,
    index_buffer: glium::IndexBuffer<u16>,
    programs: [glium::Program; shaders::FRAGMENT_COUNT],
    egl_display: Rc<RefCell<Option<EGLDisplay>>>,
    log: Logger,
}

impl<F: EGLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn borrow(&self) -> Ref<F> {
        self.display.borrow()
    }
}

impl<T: Into<GliumGraphicsBackend<T>> + EGLGraphicsBackend + 'static> GliumDrawer<T> {
    pub fn init(backend: T, egl_display: Rc<RefCell<Option<EGLDisplay>>>, log: Logger) -> GliumDrawer<T> {
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
        ).unwrap();

        // building the index buffer
        let index_buffer =
            glium::IndexBuffer::new(&display, PrimitiveType::TriangleStrip, &[1 as u16, 2, 0, 3]).unwrap();

        let programs = opengl_programs!(&display);

        GliumDrawer {
            display,
            vertex_buffer,
            index_buffer,
            programs,
            egl_display,
            log,
        }
    }
}

impl<F: EGLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn texture_from_buffer(&self, buffer: Resource<wl_buffer::WlBuffer>) -> Result<TextureMetadata, ()> {
        // try to retrieve the egl contents of this buffer
        let images = if let Some(display) = &self.egl_display.borrow().as_ref() {
            display.egl_buffer_contents(buffer)
        } else {
            Err(BufferAccessError::NotManaged(buffer))
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
                ).unwrap();
                unsafe {
                    images
                        .bind_to_texture(0, opengl_texture.get_id())
                        .expect("Failed to bind to texture");
                }
                Ok(TextureMetadata {
                    texture: opengl_texture,
                    fragment: ::shaders::BUFFER_RGBA,
                    y_inverted: images.y_inverted,
                    dimensions: (images.width, images.height),
                    images: Some(images), // I guess we need to keep this alive ?
                })
            }
            Err(BufferAccessError::NotManaged(buffer)) => {
                // this is not an EGL buffer, try SHM
                match shm_buffer_contents(&buffer, |slice, data| {
                    ::shm_load::load_shm_buffer(data, slice)
                        .map(|(image, kind)| (Texture2d::new(&self.display, image).unwrap(), kind, data))
                }) {
                    Ok(Ok((texture, kind, data))) => Ok(TextureMetadata {
                        texture,
                        fragment: kind,
                        y_inverted: false,
                        dimensions: (data.width as u32, data.height as u32),
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
            Err(err) => {
                error!(self.log, "EGL error"; "err" => format!("{:?}", err));
                Err(())
            }
        }
    }

    pub fn render_texture(
        &self,
        target: &mut glium::Frame,
        texture: &Texture2d,
        texture_kind: usize,
        y_inverted: bool,
        surface_dimensions: (u32, u32),
        surface_location: (i32, i32),
        screen_size: (u32, u32),
        blending: glium::Blend,
    ) {
        let xscale = 2.0 * (surface_dimensions.0 as f32) / (screen_size.0 as f32);
        let mut yscale = -2.0 * (surface_dimensions.1 as f32) / (screen_size.1 as f32);

        let x = 2.0 * (surface_location.0 as f32) / (screen_size.0 as f32) - 1.0;
        let mut y = 1.0 - 2.0 * (surface_location.1 as f32) / (screen_size.1 as f32);

        if y_inverted {
            yscale = -yscale;
            y -= surface_dimensions.1 as f32;
        }

        let uniforms = uniform! {
            matrix: [
                [xscale,   0.0  , 0.0, 0.0],
                [  0.0 , yscale , 0.0, 0.0],
                [  0.0 ,   0.0  , 1.0, 0.0],
                [   x  ,    y   , 0.0, 1.0]
            ],
            tex: texture,
        };

        target
            .draw(
                &self.vertex_buffer,
                &self.index_buffer,
                &self.programs[texture_kind],
                &uniforms,
                &glium::DrawParameters {
                    blend: blending,
                    ..Default::default()
                },
            ).unwrap();
    }

    #[inline]
    pub fn draw(&self) -> Frame {
        self.display.draw()
    }
}

impl<G: EGLWaylandExtensions + EGLGraphicsBackend + 'static> EGLWaylandExtensions for GliumDrawer<G> {
    fn bind_wl_display(&self, display: &Display) -> EGLResult<EGLDisplay> {
        self.display.bind_wl_display(display)
    }
}

pub struct TextureMetadata {
    pub texture: Texture2d,
    pub fragment: usize,
    pub y_inverted: bool,
    pub dimensions: (u32, u32),
    images: Option<EGLImages>,
}

impl<F: EGLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn draw_windows(&self, window_map: &MyWindowMap, compositor_token: MyCompositorToken, log: &Logger) {
        let mut frame = self.draw();
        frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);
        // redraw the frame, in a simple but inneficient way
        {
            let screen_dimensions = self.borrow().get_framebuffer_dimensions();
            window_map.with_windows_from_bottom_to_top(|toplevel_surface, initial_place| {
                if let Some(wl_surface) = toplevel_surface.get_surface() {
                    // this surface is a root of a subsurface tree that needs to be drawn
                    compositor_token
                        .with_surface_tree_upward(
                            wl_surface,
                            initial_place,
                            |_surface, attributes, role, &(mut x, mut y)| {
                                // there is actually something to draw !
                                if attributes.user_data.texture.is_none() {
                                    if let Some(buffer) = attributes.user_data.buffer.take() {
                                        if let Ok(m) = self.texture_from_buffer(buffer.clone()) {
                                            attributes.user_data.texture = Some(m);
                                        }
                                        // notify the client that we have finished reading the
                                        // buffer
                                        buffer.send(wl_buffer::Event::Release);
                                    }
                                }
                                if let Some(ref metadata) = attributes.user_data.texture {
                                    if let Ok(subdata) = Role::<SubsurfaceRole>::data(role) {
                                        x += subdata.location.0;
                                        y += subdata.location.1;
                                    }
                                    self.render_texture(
                                        &mut frame,
                                        &metadata.texture,
                                        metadata.fragment,
                                        metadata.y_inverted,
                                        metadata.dimensions,
                                        (x, y),
                                        screen_dimensions,
                                        ::glium::Blend {
                                            color: ::glium::BlendingFunction::Addition {
                                                source: ::glium::LinearBlendingFactor::One,
                                                destination:
                                                    ::glium::LinearBlendingFactor::OneMinusSourceAlpha,
                                            },
                                            alpha: ::glium::BlendingFunction::Addition {
                                                source: ::glium::LinearBlendingFactor::One,
                                                destination:
                                                    ::glium::LinearBlendingFactor::OneMinusSourceAlpha,
                                            },
                                            ..Default::default()
                                        },
                                    );
                                    TraversalAction::DoChildren((x, y))
                                } else {
                                    // we are not display, so our children are neither
                                    TraversalAction::SkipChildren
                                }
                            },
                        ).unwrap();
                }
            });
        }
        if let Err(err) = frame.finish() {
            error!(log, "Error during rendering: {:?}", err);
        }
    }
}
