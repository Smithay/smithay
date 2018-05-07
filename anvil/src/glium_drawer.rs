use glium;
use glium::{Frame, GlObject, Surface};
use glium::index::PrimitiveType;
use glium::texture::{MipmapsOption, Texture2d, UncompressedFloatFormat};
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::graphics::egl::error::Result as EGLResult;
use smithay::backend::graphics::egl::wayland::{EGLDisplay, EGLImages, EGLWaylandExtensions, Format};
use smithay::backend::graphics::glium::GliumGraphicsBackend;
use smithay::wayland_server::Display;

use std::cell::Ref;

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
    program: glium::Program,
}

impl<F: EGLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn borrow(&self) -> Ref<F> {
        self.display.borrow()
    }
}

impl<T: Into<GliumGraphicsBackend<T>> + EGLGraphicsBackend + 'static> From<T> for GliumDrawer<T> {
    fn from(backend: T) -> GliumDrawer<T> {
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

        // compiling shaders and linking them together
        let program = program!(&display,
			100 => {
				vertex: "
					#version 100
					uniform lowp mat4 matrix;
					attribute lowp vec2 position;
					attribute lowp vec2 tex_coords;
					varying lowp vec2 v_tex_coords;
					void main() {
						gl_Position = matrix * vec4(position, 0.0, 1.0);
						v_tex_coords = tex_coords;
					}
				",

				fragment: "
					#version 100
					uniform lowp sampler2D tex;
					varying lowp vec2 v_tex_coords;
					void main() {
	                    lowp vec4 color = texture2D(tex, v_tex_coords);
						gl_FragColor.r = color.z;
                        gl_FragColor.g = color.y;
                        gl_FragColor.b = color.x;
                        gl_FragColor.a = color.w;
					}
				",
			},
        ).unwrap();

        GliumDrawer {
            display,
            vertex_buffer,
            index_buffer,
            program,
        }
    }
}

impl<F: EGLGraphicsBackend + 'static> GliumDrawer<F> {
    pub fn texture_from_mem(&self, contents: &[u8], surface_dimensions: (u32, u32)) -> Texture2d {
        let image = glium::texture::RawImage2d {
            data: contents.into(),
            width: surface_dimensions.0,
            height: surface_dimensions.1,
            format: glium::texture::ClientFormat::U8U8U8U8,
        };
        Texture2d::new(&self.display, image).unwrap()
    }

    pub fn texture_from_egl(&self, images: &EGLImages) -> Option<Texture2d> {
        let format = match images.format {
            Format::RGB => UncompressedFloatFormat::U8U8U8,
            Format::RGBA => UncompressedFloatFormat::U8U8U8U8,
            _ => return None,
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
        Some(opengl_texture)
    }

    pub fn render_texture(
        &self,
        target: &mut glium::Frame,
        texture: &Texture2d,
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
                &self.program,
                &uniforms,
                &glium::DrawParameters {
                    blend: blending,
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

impl<G: EGLWaylandExtensions + EGLGraphicsBackend + 'static> EGLWaylandExtensions for GliumDrawer<G> {
    fn bind_wl_display(&self, display: &Display) -> EGLResult<EGLDisplay> {
        self.display.bind_wl_display(display)
    }
}
