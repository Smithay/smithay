use super::*;

/// A handle to a GLES texture
#[derive(Debug, Clone)]
pub struct GlesTexture(pub(super) Rc<GlesTextureInternal>);

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
        GlesTexture(Rc::new(GlesTextureInternal {
            texture: tex,
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
}

#[derive(Debug)]
pub(super) struct GlesTextureInternal {
    pub(super) texture: ffi::types::GLuint,
    pub(super) format: Option<ffi::types::GLenum>,
    pub(super) has_alpha: bool,
    pub(super) is_external: bool,
    pub(super) y_inverted: bool,
    pub(super) size: Size<i32, BufferCoord>,
    pub(super) egl_images: Option<Vec<EGLImage>>,
    pub(super) destruction_callback_sender: Sender<CleanupResource>,
}

impl Drop for GlesTextureInternal {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Texture(self.texture));
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
