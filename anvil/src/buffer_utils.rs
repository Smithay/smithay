use glium::texture::Texture2d;
#[cfg(feature = "egl")]
use glium::{
    texture::{MipmapsOption, UncompressedFloatFormat},
    GlObject,
};
use slog::Logger;
use std::collections::HashMap;
#[cfg(feature = "egl")]
use std::{cell::RefCell, rc::Rc};

#[cfg(feature = "egl")]
use smithay::backend::egl::{
    display::EGLBufferReader, BufferAccessError as EGLBufferAccessError, EGLImages, Format,
};
use smithay::{
    backend::graphics::gl::GLGraphicsBackend,
    reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    wayland::shm::{with_buffer_contents as shm_buffer_contents, BufferAccessError},
};

use crate::glium_drawer::GliumDrawer;

/// Utilities for working with `WlBuffer`s.
#[derive(Clone)]
pub struct BufferUtils {
    #[cfg(feature = "egl")]
    egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>,
    log: Logger,
}

impl BufferUtils {
    /// Creates a new `BufferUtils`.
    #[cfg(feature = "egl")]
    pub fn new(egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>, log: Logger) -> Self {
        Self {
            egl_buffer_reader,
            log,
        }
    }

    /// Creates a new `BufferUtils`.
    #[cfg(not(feature = "egl"))]
    pub fn new(log: Logger) -> Self {
        Self { log }
    }

    /// Returns the dimensions of an image stored in the buffer.
    #[cfg(feature = "egl")]
    pub fn dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        // Try to retrieve the EGL dimensions of this buffer, and, if that fails, the shm dimensions.
        self.egl_buffer_reader
            .borrow()
            .as_ref()
            .and_then(|display| display.egl_buffer_dimensions(buffer))
            .or_else(|| self.shm_buffer_dimensions(buffer).ok())
    }

    /// Returns the dimensions of an image stored in the buffer.
    #[cfg(not(feature = "egl"))]
    pub fn dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        self.shm_buffer_dimensions(buffer).ok()
    }

    /// Returns the dimensions of an image stored in the shm buffer.
    fn shm_buffer_dimensions(&self, buffer: &WlBuffer) -> Result<(i32, i32), BufferAccessError> {
        shm_buffer_contents(buffer, |_, data| (data.width, data.height)).map_err(|err| {
            warn!(self.log, "Unable to load buffer contents"; "err" => format!("{:?}", err));
            err
        })
    }

    #[cfg(feature = "egl")]
    pub fn load_buffer(&self, buffer: WlBuffer) -> Result<BufferTextures, WlBuffer> {
        // try to retrieve the egl contents of this buffer
        let images = if let Some(display) = &self.egl_buffer_reader.borrow().as_ref() {
            display.egl_buffer_contents(&buffer)
        } else {
            return Err(buffer);
        };

        match images {
            Ok(images) => {
                // we have an EGL buffer
                Ok(BufferTextures {
                    buffer,
                    textures: HashMap::new(),
                    fragment: crate::shaders::BUFFER_RGBA,
                    y_inverted: images.y_inverted,
                    dimensions: (images.width, images.height),
                    images: Some(images), // I guess we need to keep this alive ?
                    logger: self.log.clone(),
                })
            }
            Err(EGLBufferAccessError::NotManaged(_)) => {
                // this is not an EGL buffer, try SHM
                self.load_shm_buffer(buffer)
            }
            Err(err) => {
                error!(self.log, "EGL error"; "err" => format!("{:?}", err));
                Err(buffer)
            }
        }
    }

    #[cfg(not(feature = "egl"))]
    pub fn load_buffer(&self, buffer: WlBuffer) -> Result<BufferTextures, WlBuffer> {
        self.load_shm_buffer(buffer)
    }

    fn load_shm_buffer(&self, buffer: WlBuffer) -> Result<BufferTextures, WlBuffer> {
        let (width, height, format) =
            match shm_buffer_contents(&buffer, |_, data| (data.width, data.height, data.format)) {
                Ok(x) => x,
                Err(err) => {
                    warn!(self.log, "Unable to load buffer contents"; "err" => format!("{:?}", err));
                    return Err(buffer);
                }
            };
        let shader = match crate::shm_load::load_format(format) {
            Ok(x) => x.1,
            Err(format) => {
                warn!(self.log, "Unable to load buffer format: {:?}", format);
                return Err(buffer);
            }
        };
        Ok(BufferTextures {
            buffer,
            textures: HashMap::new(),
            fragment: shader,
            y_inverted: false,
            dimensions: (width as u32, height as u32),
            #[cfg(feature = "egl")]
            images: None,
            logger: self.log.clone(),
        })
    }
}

pub struct BufferTextures {
    buffer: WlBuffer,
    pub textures: HashMap<usize, Texture2d>,
    pub fragment: usize,
    pub y_inverted: bool,
    pub dimensions: (u32, u32),
    #[cfg(feature = "egl")]
    images: Option<EGLImages>,
    logger: slog::Logger,
}

impl BufferTextures {
    #[cfg(feature = "egl")]
    pub fn load_texture<'a, F: GLGraphicsBackend + 'static>(
        &'a mut self,
        drawer: &GliumDrawer<F>,
    ) -> Result<&'a Texture2d, ()> {
        if self.textures.contains_key(&drawer.id) {
            return Ok(&self.textures[&drawer.id]);
        }

        if let Some(images) = self.images.as_ref() {
            //EGL buffer
            let format = match images.format {
                Format::RGB => UncompressedFloatFormat::U8U8U8,
                Format::RGBA => UncompressedFloatFormat::U8U8U8U8,
                _ => {
                    warn!(self.logger, "Unsupported EGL buffer format"; "format" => format!("{:?}", images.format));
                    return Err(());
                }
            };

            let opengl_texture = Texture2d::empty_with_format(
                &drawer.display,
                format,
                MipmapsOption::NoMipmap,
                images.width,
                images.height,
            )
            .unwrap();

            unsafe {
                images
                    .bind_to_texture(0, opengl_texture.get_id(), &*drawer.display.borrow())
                    .expect("Failed to bind to texture");
            }

            self.textures.insert(drawer.id, opengl_texture);
            Ok(&self.textures[&drawer.id])
        } else {
            self.load_shm_texture(drawer)
        }
    }

    #[cfg(not(feature = "egl"))]
    pub fn load_texture<'a, F: GLGraphicsBackend + 'static>(
        &'a mut self,
        drawer: &GliumDrawer<F>,
    ) -> Result<&'a Texture2d, ()> {
        if self.textures.contains_key(&drawer.id) {
            return Ok(&self.textures[&drawer.id]);
        }

        self.load_shm_texture(drawer)
    }

    fn load_shm_texture<'a, F: GLGraphicsBackend + 'static>(
        &'a mut self,
        drawer: &GliumDrawer<F>,
    ) -> Result<&'a Texture2d, ()> {
        match shm_buffer_contents(&self.buffer, |slice, data| {
            crate::shm_load::load_shm_buffer(data, slice)
                .map(|(image, _kind)| Texture2d::new(&drawer.display, image).unwrap())
        }) {
            Ok(Ok(texture)) => {
                self.textures.insert(drawer.id, texture);
                Ok(&self.textures[&drawer.id])
            }
            Ok(Err(format)) => {
                warn!(self.logger, "Unsupported SHM buffer format"; "format" => format!("{:?}", format));
                Err(())
            }
            Err(err) => {
                warn!(self.logger, "Unable to load buffer contents"; "err" => format!("{:?}", err));
                Err(())
            }
        }
    }
}

impl Drop for BufferTextures {
    fn drop(&mut self) {
        self.buffer.release()
    }
}
