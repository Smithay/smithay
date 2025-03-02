use ffi::Gles2;

use super::*;
use std::sync::Arc;

/// A handle to a GLES texture
///
/// The texture can be used with the same [`GlesRenderer`] it was created with, or one using a
/// shared [`EGLContext`].
#[derive(Debug, Clone)]
pub struct GlesTexture(pub(super) Arc<GlesTextureInternal>);

impl GlesTexture {
    /// Create a GlesTexture from a raw gl texture id.
    ///
    /// It is expected to not be external or y_inverted.
    ///
    /// Ownership over the texture is taken by the renderer, you should not free the texture yourself.
    ///
    /// # Safety
    ///
    /// The renderer cannot make sure `tex` is a valid texture id.
    pub unsafe fn from_raw(
        renderer: &GlesRenderer,
        internal_format: Option<ffi::types::GLenum>,
        opaque: bool,
        tex: ffi::types::GLuint,
        size: Size<i32, BufferCoord>,
    ) -> GlesTexture {
        GlesTexture(Arc::new(GlesTextureInternal {
            texture: tex,
            sync: RwLock::default(),
            format: internal_format,
            has_alpha: !opaque,
            is_external: false,
            y_inverted: false,
            size,
            egl_images: None,
            destruction_callback_sender: renderer.destruction_callback_sender.clone(),
        }))
    }

    /// OpenGL texture id of this texture
    ///
    /// This id will become invalid, when the GlesTexture is dropped and does not transfer ownership.
    pub fn tex_id(&self) -> ffi::types::GLuint {
        self.0.texture
    }

    /// Whether the texture is upside down
    pub fn is_y_inverted(&self) -> bool {
        self.0.y_inverted
    }

    /// Whether this is the only reference to this texture (strong or weak)
    ///
    /// Note that this tracks only references to this Smithay object (that you can get by cloning
    /// it). If you make a reference via OpenGL directly somehow, you need to keep track of it on
    /// your own.
    pub fn is_unique_reference(&mut self) -> bool {
        Arc::get_mut(&mut self.0).is_some()
    }
}

#[derive(Debug, Default)]
pub(super) struct TextureSync {
    read_sync: Mutex<Option<ffi::types::GLsync>>,
    write_sync: Mutex<Option<ffi::types::GLsync>>,
}

unsafe fn wait_for_syncpoint(sync: &mut Option<ffi::types::GLsync>, gl: &Gles2) {
    if let Some(sync_obj) = *sync {
        match gl.ClientWaitSync(sync_obj, 0, 0) {
            ffi::ALREADY_SIGNALED | ffi::CONDITION_SATISFIED => {
                let _ = sync.take();
                gl.DeleteSync(sync_obj);
            }
            _ => {
                gl.WaitSync(sync_obj, 0, ffi::TIMEOUT_IGNORED);
            }
        };
    }
}

impl TextureSync {
    pub(super) fn wait_for_upload(&self, gl: &Gles2) {
        unsafe {
            wait_for_syncpoint(&mut self.write_sync.lock().unwrap(), gl);
        }
    }

    pub(super) fn update_read(&self, gl: &Gles2) {
        let mut read_sync = self.read_sync.lock().unwrap();
        if let Some(old) = read_sync.take() {
            unsafe {
                gl.WaitSync(old, 0, ffi::TIMEOUT_IGNORED);
                gl.DeleteSync(old);
            };
        }
        *read_sync = Some(unsafe { gl.FenceSync(ffi::SYNC_GPU_COMMANDS_COMPLETE, 0) });
    }

    pub(super) fn wait_for_all(&mut self, gl: &Gles2) {
        unsafe {
            wait_for_syncpoint(self.read_sync.get_mut().unwrap(), gl);
            wait_for_syncpoint(self.write_sync.get_mut().unwrap(), gl);
        }
    }

    pub(super) fn update_write(&mut self, gl: &Gles2) {
        let write_sync = self.write_sync.get_mut().unwrap();
        if let Some(old) = write_sync.take() {
            unsafe {
                gl.WaitSync(old, 0, ffi::TIMEOUT_IGNORED);
                gl.DeleteSync(old);
            };
        }

        *write_sync = Some(unsafe { gl.FenceSync(ffi::SYNC_GPU_COMMANDS_COMPLETE, 0) });
    }
}

#[derive(Debug)]
pub(super) struct GlesTextureInternal {
    pub(super) texture: ffi::types::GLuint,
    pub(super) sync: RwLock<TextureSync>,
    pub(super) format: Option<ffi::types::GLenum>,
    pub(super) has_alpha: bool,
    pub(super) is_external: bool,
    pub(super) y_inverted: bool,
    pub(super) size: Size<i32, BufferCoord>,
    pub(super) egl_images: Option<Vec<EGLImage>>,
    pub(super) destruction_callback_sender: Sender<CleanupResource>,
}
unsafe impl Send for GlesTextureInternal {}
unsafe impl Sync for GlesTextureInternal {}

impl Drop for GlesTextureInternal {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Texture(self.texture));
        let mut sync = self.sync.write().unwrap();
        if let Some(sync) = sync.read_sync.get_mut().unwrap().take() {
            let _ = self
                .destruction_callback_sender
                .send(CleanupResource::Sync(sync as *const _));
        }
        if let Some(sync) = sync.write_sync.get_mut().unwrap().take() {
            let _ = self
                .destruction_callback_sender
                .send(CleanupResource::Sync(sync as *const _));
        }
        if let Some(images) = self.egl_images.take() {
            for image in images {
                let _ = self
                    .destruction_callback_sender
                    .send(CleanupResource::EGLImage(image));
            }
        }
    }
}

impl Texture for GlesTexture {
    fn width(&self) -> u32 {
        self.0.size.w as u32
    }
    fn height(&self) -> u32 {
        self.0.size.h as u32
    }
    fn size(&self) -> Size<i32, BufferCoord> {
        self.0.size
    }
    fn format(&self) -> Option<Fourcc> {
        let fmt = gl_internal_format_to_fourcc(self.0.format?);
        if self.0.has_alpha {
            fmt
        } else {
            fmt.and_then(get_opaque)
        }
    }
}

/// Texture mapping of a GLES2 texture
#[derive(Debug)]
pub struct GlesMapping {
    pub(super) pbo: ffi::types::GLuint,
    pub(super) format: ffi::types::GLenum,
    pub(super) layout: ffi::types::GLenum,
    pub(super) has_alpha: bool,
    pub(super) size: Size<i32, BufferCoord>,
    pub(super) mapping: AtomicPtr<std::ffi::c_void>,
    pub(super) destruction_callback_sender: Sender<CleanupResource>,
}

impl Texture for GlesMapping {
    fn width(&self) -> u32 {
        self.size.w as u32
    }
    fn height(&self) -> u32 {
        self.size.h as u32
    }
    fn size(&self) -> Size<i32, BufferCoord> {
        self.size
    }
    fn format(&self) -> Option<Fourcc> {
        let fmt = gl_read_format_to_fourcc(self.format, self.layout);
        if self.has_alpha {
            fmt
        } else {
            fmt.and_then(get_opaque)
        }
    }
}
impl TextureMapping for GlesMapping {
    fn flipped(&self) -> bool {
        true
    }
    fn format(&self) -> Fourcc {
        Texture::format(self).expect("Should never happen")
    }
}

impl Drop for GlesMapping {
    fn drop(&mut self) {
        let _ = self.destruction_callback_sender.send(CleanupResource::Mapping(
            self.pbo,
            self.mapping.load(Ordering::SeqCst),
        ));
    }
}
