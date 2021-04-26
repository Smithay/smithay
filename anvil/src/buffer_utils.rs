#[cfg(feature = "egl")]
use std::{cell::RefCell, rc::Rc};

#[cfg(feature = "udev")]
use smithay::backend::renderer::{Renderer, Texture};
#[cfg(feature = "udev")]
use smithay::reexports::nix::libc::dev_t;
#[cfg(feature = "udev")]
use std::collections::HashMap;

#[cfg(feature = "egl")]
use smithay::backend::egl::{
    display::EGLBufferReader, BufferAccessError as EGLBufferAccessError, EGLBuffer,
};
use smithay::{
    reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    wayland::shm::{with_buffer_contents as shm_buffer_contents, BufferAccessError},
};

/// Utilities for working with `WlBuffer`s.
#[derive(Clone)]
pub struct BufferUtils {
    #[cfg(feature = "egl")]
    egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>,
    log: ::slog::Logger,
}

impl BufferUtils {
    /// Creates a new `BufferUtils`.
    #[cfg(feature = "egl")]
    pub fn new(egl_buffer_reader: Rc<RefCell<Option<EGLBufferReader>>>, log: ::slog::Logger) -> Self {
        Self {
            egl_buffer_reader,
            log,
        }
    }

    /// Creates a new `BufferUtils`.
    #[cfg(not(feature = "egl"))]
    pub fn new(log: ::slog::Logger) -> Self {
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
    pub fn load_buffer<T>(&self, buffer: WlBuffer) -> Result<BufferTextures<T>, WlBuffer> {
        let result = if let Some(reader) = &self.egl_buffer_reader.borrow().as_ref() {
            reader.egl_buffer_contents(&buffer)
        } else {
            return Err(buffer);
        };

        let egl_buffer = match result {
            Ok(egl) => Some(egl),
            Err(EGLBufferAccessError::NotManaged(_)) => { None },
            Err(err) => {
                error!(self.log, "EGL error"; "err" => format!("{:?}", err));
                return Err(buffer);
            }
        };

        Ok(BufferTextures {
            buffer,
            textures: HashMap::new(),
            egl: egl_buffer, // I guess we need to keep this alive ?
        })
    }

    #[cfg(not(feature = "egl"))]
    pub fn load_buffer<T>(&self, buffer: WlBuffer) -> Result<BufferTextures<T>, WlBuffer> {
        Ok(BufferTextures {
            buffer,
            textures: HashMap::new(),
        })
    }
}

#[cfg(feature = "udev")]
pub struct BufferTextures<T> {
    buffer: WlBuffer,
    pub textures: HashMap<dev_t, T>,
    #[cfg(feature = "egl")]
    egl: Option<EGLBuffer>,
}

#[cfg(feature = "udev")]
impl<T: Texture> BufferTextures<T> {
    #[cfg(feature = "egl")]
    pub fn load_texture<'a, R: Renderer<Texture=T>>(
        &'a mut self,
        id: u64,
        renderer: &mut R,
    ) -> Result<&'a T, R::Error> {
        if self.textures.contains_key(&id) {
            return Ok(&self.textures[&id]);
        }

        if let Some(buffer) = self.egl.as_ref() {
            //EGL buffer
            let texture = renderer.import_egl(&buffer)?;
            self.textures.insert(id, texture);
            Ok(&self.textures[&id])
        } else {
            self.load_shm_texture(id, renderer)
        }
    }

    #[cfg(not(feature = "egl"))]
    pub fn load_texture<'a, R: Renderer<Texture=T>>(
        &'a mut self,
        id: u64,
        renderer: &mut R,
    ) -> Result<&'a T, R::Error> {
        if self.textures.contains_key(&id) {
            return Ok(&self.textures[&id]);
        }

        self.load_shm_texture(id, renderer)
    }

    fn load_shm_texture<'a, R: Renderer<Texture=T>>(
        &'a mut self,
        id: u64,
        renderer: &mut R,
    ) -> Result<&'a T, R::Error> {
        let texture = renderer.import_shm(&self.buffer)?;
        
        self.textures.insert(id, texture);
        Ok(&self.textures[&id])
    }
}

#[cfg(feature = "udev")]
impl<T> Drop for BufferTextures<T> {
    fn drop(&mut self) {
        self.buffer.release()
    }
}