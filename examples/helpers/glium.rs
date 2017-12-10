use glium;
use glium::{Frame, Surface};
use glium::index::PrimitiveType;
use smithay::backend::graphics::egl::EGLGraphicsBackend;
use smithay::backend::graphics::glium::GliumGraphicsBackend;
use std::borrow::Borrow;
use std::ops::Deref;

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

impl<F: EGLGraphicsBackend + 'static> Deref for GliumDrawer<F> {
    type Target = F;

    fn deref(&self) -> &F {
        self.borrow()
    }
}

impl<F: EGLGraphicsBackend + 'static> Borrow<F> for GliumDrawer<F> {
    fn borrow(&self) -> &F {
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
    pub fn render_shm(&self, target: &mut glium::Frame, contents: &[u8], surface_dimensions: (u32, u32),
                  surface_location: (i32, i32), screen_size: (u32, u32)) {
        let image = glium::texture::RawImage2d {
            data: contents.into(),
            width: surface_dimensions.0,
            height: surface_dimensions.1,
            format: glium::texture::ClientFormat::U8U8U8U8,
        };
        let opengl_texture = glium::texture::Texture2d::new(&self.display, image).unwrap();

        let xscale = 2.0 * (surface_dimensions.0 as f32) / (screen_size.0 as f32);
        let yscale = -2.0 * (surface_dimensions.1 as f32) / (screen_size.1 as f32);

        let x = 2.0 * (surface_location.0 as f32) / (screen_size.0 as f32) - 1.0;
        let y = 1.0 - 2.0 * (surface_location.1 as f32) / (screen_size.1 as f32);

        let uniforms = uniform! {
            matrix: [
                [xscale,   0.0  , 0.0, 0.0],
                [  0.0 , yscale , 0.0, 0.0],
                [  0.0 ,   0.0  , 1.0, 0.0],
                [   x  ,    y   , 0.0, 1.0]
            ],
            tex: &opengl_texture
        };

        target
            .draw(
                &self.vertex_buffer,
                &self.index_buffer,
                &self.program,
                &uniforms,
                &Default::default(),
            )
            .unwrap();
    }

    pub fn render_egl(&self, target: &mut glium::Frame, images: Vec<EGLImage>,
                  format: UncompressedFloatFormat, y_inverted: bool, surface_dimensions: (u32, u32),
                  surface_location: (i32, i32), screen_size: (u32, u32))
    {
        let opengl_texture = glium::texture::Texture2d::empty_with_format(
            &self.display,
            MipmapsOption::NoMipmap,
            format,
            surface_dimensions.0,
            surface_dimensions.1,
        ).unwrap();
        self.display.exec_in_context(|| {
            self.display.borrow().egl_image_to_texture(images[0], opengl_texture.get_id());
        });

        let xscale = 2.0 * (surface_dimensions.0 as f32) / (screen_size.0 as f32);
        let mut yscale = -2.0 * (surface_dimensions.1 as f32) / (screen_size.1 as f32);
        if y_inverted {
            yscale = -yscale;
        }

        let x = 2.0 * (surface_location.0 as f32) / (screen_size.0 as f32) - 1.0;
        let y = 1.0 - 2.0 * (surface_location.1 as f32) / (screen_size.1 as f32);

        let uniforms = uniform! {
            matrix: [
                [xscale,   0.0  , 0.0, 0.0],
                [  0.0 , yscale , 0.0, 0.0],
                [  0.0 ,   0.0  , 1.0, 0.0],
                [   x  ,    y   , 0.0, 1.0]
            ],
            tex: &opengl_texture
        };

        target
            .draw(
                &self.vertex_buffer,
                &self.index_buffer,
                &self.program,
                &uniforms,
                &Default::default(),
            )
            .unwrap();
    }

    #[inline]
    pub fn draw(&self) -> Frame {
        self.display.draw()
    }
}
