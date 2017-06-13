use glium;
use glium::Surface;
use glium::index::PrimitiveType;

#[derive(Copy, Clone)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
}

implement_vertex!(Vertex, position, tex_coords);

pub struct GliumDrawer<'a, F: 'a> {
    display: &'a F,
    vertex_buffer: glium::VertexBuffer<Vertex>,
    index_buffer: glium::IndexBuffer<u16>,
    program: glium::Program,
}

impl<'a, F: glium::backend::Facade + 'a> GliumDrawer<'a, F> {
    pub fn new(display: &'a F) -> GliumDrawer<'a, F> {

        // building the vertex buffer, which contains all the vertices that we will draw
        let vertex_buffer = glium::VertexBuffer::new(display,
                                                     &[Vertex {
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
                                                       }]).unwrap();

        // building the index buffer
        let index_buffer =
            glium::IndexBuffer::new(display, PrimitiveType::TriangleStrip, &[1 as u16, 2, 0, 3]).unwrap();

        // compiling shaders and linking them together
        let program = program!(display,
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

    pub fn draw(&self, target: &mut glium::Frame, contents: &[u8], surface_dimensions: (u32, u32),
                surface_location: (i32, i32), screen_size: (u32, u32)) {

        let image = glium::texture::RawImage2d {
            data: contents.into(),
            width: surface_dimensions.0,
            height: surface_dimensions.1,
            format: glium::texture::ClientFormat::U8U8U8U8,
        };
        let opengl_texture = glium::texture::CompressedSrgbTexture2d::new(self.display, image).unwrap();

        let xscale = 2.0 * (surface_dimensions.0 as f32) / (screen_size.0 as f32);
        let yscale = -2.0 * (surface_dimensions.1 as f32) / (screen_size.1 as f32);

        let x = 2.0 * (surface_location.0 as f32) / (screen_size.0 as f32) - 1.0;
        let y = 1.0 - 2.0 * (surface_location.1 as f32) / (screen_size.1 as f32);

        let uniforms =
            uniform! {
            matrix: [
                [xscale,   0.0  , 0.0, 0.0],
                [  0.0 , yscale , 0.0, 0.0],
                [  0.0 ,   0.0  , 1.0, 0.0],
                [   x  ,    y   , 0.0, 1.0]
            ],
            tex: &opengl_texture
        };

        target
            .draw(&self.vertex_buffer,
                  &self.index_buffer,
                  &self.program,
                  &uniforms,
                  &Default::default())
            .unwrap();

    }
}
