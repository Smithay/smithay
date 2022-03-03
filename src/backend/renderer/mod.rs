//! Rendering functionality and abstractions
//!
//! Collection of common traits and implementations
//! to facilitate (possible hardware-accelerated) rendering.
//!
//! Supported rendering apis:
//!
//! - Raw OpenGL ES 2

use std::collections::HashSet;
use std::error::Error;

use crate::utils::{Buffer, Physical, Point, Rectangle, Size, Transform};

#[cfg(feature = "wayland_frontend")]
use crate::wayland::compositor::SurfaceData;
use cgmath::Matrix3;
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

#[cfg(feature = "renderer_gl")]
pub mod gles2;

use crate::backend::allocator::{dmabuf::Dmabuf, Format};
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use crate::backend::egl::{
    display::{EGLBufferReader, BUFFER_READER},
    Error as EglError,
};
#[cfg(feature = "renderer_multi")]
pub mod multigpu;

#[cfg(feature = "wayland_frontend")]
pub mod utils;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// Texture filtering methods
pub enum TextureFilter {
    /// Returns the value of the texture element that is nearest (in Manhattan distance) to the center of the pixel being textured.
    Linear,
    /// Returns the weighted average of the four texture elements that are closest to the center of the pixel being textured.
    Nearest,
}

impl Transform {
    /// A projection matrix to apply this transformation
    pub fn matrix(&self) -> Matrix3<f32> {
        match self {
            Transform::Normal => Matrix3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_90 => Matrix3::new(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_180 => Matrix3::new(-1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_270 => Matrix3::new(0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped => Matrix3::new(-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped90 => Matrix3::new(0.0, -1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped180 => Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped270 => Matrix3::new(0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl From<wayland_server::protocol::wl_output::Transform> for Transform {
    fn from(transform: wayland_server::protocol::wl_output::Transform) -> Transform {
        use wayland_server::protocol::wl_output::Transform as WlTransform;
        match transform {
            WlTransform::Normal => Transform::Normal,
            WlTransform::_90 => Transform::_90,
            WlTransform::_180 => Transform::_180,
            WlTransform::_270 => Transform::_270,
            WlTransform::Flipped => Transform::Flipped,
            WlTransform::Flipped90 => Transform::Flipped90,
            WlTransform::Flipped180 => Transform::Flipped180,
            WlTransform::Flipped270 => Transform::Flipped270,
            _ => Transform::Normal,
        }
    }
}

/// Abstraction for Renderers, that can render into different targets
pub trait Bind<Target>: Unbind {
    /// Bind a given rendering target, which will contain the rendering results until `unbind` is called.
    ///
    /// Binding to target, while another one is already bound, is rendering defined.
    /// Some renderers might happily replace the current target, while other might drop the call
    /// or throw an error.
    fn bind(&mut self, target: Target) -> Result<(), <Self as Renderer>::Error>;
    /// Supported pixel formats for given targets, if applicable.
    fn supported_formats(&self) -> Option<HashSet<crate::backend::allocator::Format>> {
        None
    }
}

/// Functionality to unbind the current rendering target
pub trait Unbind: Renderer {
    /// Unbind the current rendering target.
    ///
    /// May fall back to a default target, if defined by the implementation.
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error>;
}

/// A two dimensional texture
pub trait Texture {
    /// Size of the texture plane
    fn size(&self) -> Size<i32, Buffer> {
        Size::from((self.width() as i32, self.height() as i32))
    }

    /// Width of the texture plane
    fn width(&self) -> u32;
    /// Height of the texture plane
    fn height(&self) -> u32;
}

/// A downloaded texture buffer
pub trait TextureMapping: Texture {
    /// Returns if the mapped buffer is flipped on the y-axis
    /// (compared to the lower left being (0, 0))
    fn flipped(&self) -> bool;
}

/// Helper trait for [`Renderer`], which defines a rendering api for a currently in-progress frame during [`Renderer::render`].
pub trait Frame {
    /// Error type returned by the rendering operations of this renderer.
    type Error: Error;
    /// Texture Handle type used by this renderer.
    type TextureId: Texture;

    /// Clear the complete current target with a single given color.
    ///
    /// The `at` parameter specifies a set of rectangles to clear in the current target. This allows partially
    /// clearing the target which may be useful for damaged rendering.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error>;

    /// Render a texture to the current target as a flat 2d-plane at a given
    /// position and applying the given transformation with the given alpha value.
    /// (Meaning `src_transform` should match the orientation of surface being rendered).
    #[allow(clippy::too_many_arguments)]
    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: Point<f64, Physical>,
        texture_scale: i32,
        output_scale: f64,
        src_transform: Transform,
        damage: &[Rectangle<i32, Buffer>],
        alpha: f32,
    ) -> Result<(), Self::Error> {
        self.render_texture_from_to(
            texture,
            Rectangle::from_loc_and_size(Point::<i32, Buffer>::from((0, 0)), texture.size()),
            Rectangle::from_loc_and_size(
                pos,
                texture
                    .size()
                    .to_logical(texture_scale, src_transform)
                    .to_f64()
                    .to_physical(output_scale),
            ),
            damage,
            src_transform,
            alpha,
        )
    }

    /// Render part of a texture as given by src to the current target into the rectangle described by dst
    /// as a flat 2d-plane after applying the inverse of the given transformation.
    /// (Meaning `src_transform` should match the orientation of surface being rendered).
    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<i32, Buffer>,
        dst: Rectangle<f64, Physical>,
        damage: &[Rectangle<i32, Buffer>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error>;

    /// Output transformation that is applied to this frame
    fn transformation(&self) -> Transform;
}

/// Abstraction of commonly used rendering operations for compositors.
pub trait Renderer {
    /// Error type returned by the rendering operations of this renderer.
    type Error: Error;
    /// Texture Handle type used by this renderer.
    type TextureId: Texture;
    /// Type representing a currently in-progress frame during the [`Renderer::render`]-call
    type Frame: Frame<Error = Self::Error, TextureId = Self::TextureId>;

    /// Returns an id, that is unique to all renderers, that can use
    /// `TextureId`s originating from any of these renderers.
    fn id(&self) -> usize;

    /// Set the filter method to be used when rendering a texture into a smaller area than its size
    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error>;
    /// Set the filter method to be used when rendering a texture into a larger area than its size
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error>;

    /// Initialize a rendering context on the current rendering target with given dimensions and transformation.
    ///
    /// This function *may* error, if:
    /// - The given dimensions are unsupported (too large) for this renderer
    /// - The given Transformation is not supported by the renderer (`Transform::Normal` is always supported).
    /// - This renderer implements `Bind`, no target was bound *and* has no default target.
    /// - (Renderers not implementing `Bind` always have a default target.)
    fn render<F, R>(
        &mut self,
        size: Size<i32, Physical>,
        dst_transform: Transform,
        rendering: F,
    ) -> Result<R, Self::Error>
    where
        F: FnOnce(&mut Self, &mut Self::Frame) -> R;
}

/// Trait for renderers that support creating offscreen framebuffers to render into.
///
/// Usually also implement either [`ExportMem`] and/or [`ExportDma`] to receive the framebuffers contents.
pub trait Offscreen<Target>: Renderer + Bind<Target> {
    /// Create a new instance of a framebuffer.
    ///
    /// This call *may* fail, if (but not limited to):
    /// - The maximum amount of framebuffers for this renderer would be exceeded
    /// - The size is too large for a framebuffer
    fn create_buffer(&mut self, size: Size<i32, Buffer>) -> Result<Target, <Self as Renderer>::Error>;
}

/// Trait for Renderers supporting importing wl_buffers using shared memory.
#[cfg(feature = "wayland_frontend")]
pub trait ImportMemWl: ImportMem {
    /// Import a given shm-based buffer into the renderer (see [`buffer_type`]).
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture_from_to`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    ///
    /// If provided the `SurfaceAttributes` can be used to do caching of rendering resources and is generally recommended.
    ///
    /// The `damage` argument provides a list of rectangle locating parts of the buffer that need to be updated. When provided
    /// with an empty list `&[]`, the renderer is allowed to not update the texture at all.
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;

    /// Returns supported formats for shared memory buffers.
    ///
    /// Will always contain At least `Argb8888` and `Xrgb8888`.
    fn shm_formats(&self) -> &[wl_shm::Format] {
        // Mandatory
        &[wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]
    }
}

/// Trait for Renderers supporting importing bitmaps from memory.
pub trait ImportMem: Renderer {
    /// Import a given chunk of memory into the renderer.
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture_from_to`] (or [`Frame::render_texture_at`])
    ///  or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    /// This operation needs no bound or default rendering target.
    ///
    /// Settings flipped to true will cause the buffer to be interpreted like the y-axis is flipped
    /// (opposed to the lower left begin (0, 0)).
    /// This is a texture specific property, so future uploads to the same texture via [`ImportMem::update_memory`]
    /// will also be interpreted as flipped.
    ///
    /// The provided data slice needs to be in RGBA8 format, its length should thus be `size.w * size.h * 4`.
    /// Anything beyond will be truncated, if the buffer is too small an error will be returned.
    fn import_memory(
        &mut self,
        data: &[u8],
        size: Size<i32, Buffer>,
        flipped: bool,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;

    /// Import a given chunk of memory into an existing texture.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The provided data slice needs to be in RGBA8 format, its length should thus be `region.size.w * region.size.h * 4`.
    /// Anything beyond will be truncated, if the buffer is too small an error will be returned.
    ///
    /// This function *may* error, if (but not limited to):
    /// - The texture was not created using either [`ImportMem::import_shm_buffer`] or [`ImportMem::import_memory`].
    ///   External textures imported by other means (e.g. via ImportDma) may not be writable. This property is defined
    ///   by the implementation.
    /// - The region is out of bounds of the initial size the texture was created with. Implementations are not required
    ///   to support resizing the original texture.
    fn update_memory(
        &mut self,
        texture: &<Self as Renderer>::TextureId,
        data: &[u8],
        region: Rectangle<i32, Buffer>,
    ) -> Result<(), <Self as Renderer>::Error>;
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
/// Trait for Renderers supporting importing wl_drm-based buffers.
pub trait ImportEgl: Renderer {
    /// Binds the underlying EGL display to the given Wayland display.
    ///
    /// This will allow clients to utilize EGL to create hardware-accelerated
    /// surfaces. This renderer will thus be able to handle wl_drm-based buffers.
    ///
    /// ## Errors
    ///
    /// This might return [`EglExtensionNotSupported`](super::egl::Error::EglExtensionNotSupported)
    /// if binding is not supported by the EGL implementation.
    ///
    /// This might return [`OtherEGLDisplayAlreadyBound`](super::egl::Error::OtherEGLDisplayAlreadyBound)
    /// if called for the same [`Display`](wayland_server::Display) multiple times, as only one egl
    /// display may be bound at any given time.
    fn bind_wl_display(&mut self, display: &wayland_server::Display) -> Result<(), EglError>;

    /// Unbinds a previously bound egl display, if existing.
    ///
    /// *Note*: As a result any previously created egl-based WlBuffers will not be readable anymore.
    /// Your compositor will have to deal with existing buffers of *unknown* type.
    fn unbind_wl_display(&mut self);

    /// Returns the underlying [`EGLBufferReader`].
    ///
    /// The primary use for this is calling [`buffer_dimensions`] or [`buffer_type`].
    ///
    /// Returns `None` if no [`Display`](wayland_server::Display) was previously bound to the underlying
    /// [`EGLDisplay`](super::egl::EGLDisplay) (see [`ImportEgl::bind_wl_display`]).
    fn egl_reader(&self) -> Option<&EGLBufferReader>;

    /// Import a given wl_drm-based buffer into the renderer (see [`buffer_type`]).
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture_from_to`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;
}

#[cfg(feature = "wayland_frontend")]
/// Trait for Renderers supporting importing dmabuf-based wl_buffers
pub trait ImportDmaWl: ImportDma {
    /// Import a given dmabuf-based buffer into the renderer (see [`buffer_type`]).
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    fn import_dma_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        let _ = surface;
        let dmabuf = buffer
            .as_ref()
            .user_data()
            .get::<Dmabuf>()
            .expect("import_dma_buffer without checking buffer type?");
        self.import_dmabuf(dmabuf, Some(damage))
    }
}

/// Trait for Renderers supporting importing dmabufs.
pub trait ImportDma: Renderer {
    /// Returns supported formats for dmabufs.
    fn dmabuf_formats<'a>(&'a self) -> Box<dyn Iterator<Item = &'a Format> + 'a> {
        Box::new([].iter())
    }

    /// Import a given raw dmabuf into the renderer.
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture_from_to`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        damage: Option<&[Rectangle<i32, Buffer>]>,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;
}

// TODO: Replace this with a trait_alias, once that is stabilized.
// pub type ImportAll = Renderer + ImportShm + ImportEgl;

/// Common trait for renderers of any wayland buffer type
#[cfg(feature = "wayland_frontend")]
pub trait ImportAll: Renderer {
    /// Import a given buffer into the renderer.
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture_from_to`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    ///
    /// If provided the `SurfaceAttributes` can be used to do caching of rendering resources and is generally recommended.
    ///
    /// The `damage` argument provides a list of rectangle locating parts of the buffer that need to be updated. When provided
    /// with an empty list `&[]`, the renderer is allowed to not update the texture at all.
    ///
    /// Returns `None`, if the buffer type cannot be determined.
    fn import_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Option<Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>>;
}

// TODO: Do this with specialization, when possible and do default implementations
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl<R: Renderer + ImportMemWl + ImportEgl + ImportDmaWl> ImportAll for R {
    fn import_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Option<Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>> {
        match buffer_type(buffer) {
            Some(BufferType::Shm) => Some(self.import_shm_buffer(buffer, surface, damage)),
            Some(BufferType::Egl) => Some(self.import_egl_buffer(buffer, surface, damage)),
            Some(BufferType::Dma) => Some(self.import_dma_buffer(buffer, surface, damage)),
            _ => None,
        }
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    not(all(feature = "backend_egl", feature = "use_system_lib"))
))]
impl<R: Renderer + ImportMemWl + ImportDmaWl> ImportAll for R {
    fn import_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Option<Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>> {
        match buffer_type(buffer) {
            Some(BufferType::Shm) => Some(self.import_shm_buffer(buffer, surface, damage)),
            Some(BufferType::Dma) => Some(self.import_dma_buffer(buffer, surface, damage)),
            _ => None,
        }
    }
}

/// Trait for renderers supporting exporting contents of framebuffers or textures into memory.
pub trait ExportMem: Renderer {
    /// Texture type representing a downloaded pixel buffer.
    type TextureMapping: TextureMapping;

    /// Copies the contents of the currently bound framebuffer.
    ///
    /// This operation is not destructive, the contents of the framebuffer keep being valid.
    ///
    /// This function *may* fail, if (but not limited to):
    /// - The framebuffer is not readable
    /// - The region is out of bounds of the framebuffer
    /// - There is not enough space to create the mapping
    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, Buffer>,
    ) -> Result<Self::TextureMapping, <Self as Renderer>::Error>;
    /// Copies the contents of the currently bound framebuffer.
    /// *Note*: This function may change or invalidate the current bind.
    ///
    /// This operation is not destructive, the contents of the texture keep being valid.
    ///
    /// This function *may* fail, if:
    /// - There is not enough space to create the mapping
    /// - The texture does no allow copying for implementation-specfic reasons
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
    ) -> Result<Self::TextureMapping, Self::Error>;
    /// Returns a read-only pointer to a previously created texture mapping.
    ///
    /// The format of the returned slice is RGBA8.
    ///
    /// This function *may* fail, if (but not limited to):
    /// - There is not enough space in memory
    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], <Self as Renderer>::Error>;
}

/// Trait for renderers supporting exporting contents of framebuffers or textures as dmabufs.
pub trait ExportDma: Renderer {
    /// Exports the currently bound framebuffer as a dmabuf.
    ///
    /// This operation is not destructive, the contents of the framebuffer keep being valid.
    ///
    /// This function *may* fail, if (but not limited to):
    /// - The framebuffer is not readable
    /// - The size is larger than the framebuffer
    /// - There is not enough space to create a copy
    fn export_framebuffer(&mut self, size: Size<i32, Buffer>) -> Result<Dmabuf, <Self as Renderer>::Error>;
    /// Exports the given texture as a dmabuf.
    ///
    /// This operation is not destructive, the contents of the texture keep being valid.
    ///
    /// This function *may* fail, if (but not limited to):
    /// - The texture cannot be exported
    fn export_texture(
        &mut self,
        texture: &<Self as Renderer>::TextureId,
    ) -> Result<Dmabuf, <Self as Renderer>::Error>;
}

#[cfg(feature = "wayland_frontend")]
#[non_exhaustive]
/// Buffer type of a given wl_buffer, if managed by smithay
#[derive(Debug)]
pub enum BufferType {
    /// Buffer is managed by the [`crate::wayland::shm`] global
    Shm,
    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    /// Buffer is managed by a currently initialized [`crate::backend::egl::display::EGLBufferReader`]
    Egl,
    /// Buffer is managed by the [`crate::wayland::dmabuf`] global
    Dma,
}

/// Returns the *type* of a wl_buffer
///
/// Returns `None` if the type is not known to smithay
/// or otherwise not supported (e.g. not initialized using one of smithays [`crate::wayland`]-handlers).
#[cfg(feature = "wayland_frontend")]
pub fn buffer_type(buffer: &wl_buffer::WlBuffer) -> Option<BufferType> {
    if buffer.as_ref().user_data().get::<Dmabuf>().is_some() {
        return Some(BufferType::Dma);
    }

    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    if BUFFER_READER
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|x| x.upgrade())
        .and_then(|x| x.egl_buffer_dimensions(buffer))
        .is_some()
    {
        return Some(BufferType::Egl);
    }

    if crate::wayland::shm::with_buffer_contents(buffer, |_, _| ()).is_ok() {
        return Some(BufferType::Shm);
    }

    None
}

/// Returns the dimensions of a wl_buffer
///
/// *Note*: This will only return dimensions for buffer types known to smithay (see [`buffer_type`])
#[cfg(feature = "wayland_frontend")]
pub fn buffer_dimensions(buffer: &wl_buffer::WlBuffer) -> Option<Size<i32, Buffer>> {
    use crate::backend::allocator::Buffer;

    if let Some(buf) = buffer.as_ref().user_data().get::<Dmabuf>() {
        return Some((buf.width() as i32, buf.height() as i32).into());
    }

    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    if let Some(dim) = BUFFER_READER
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|x| x.upgrade())
        .and_then(|x| x.egl_buffer_dimensions(buffer))
    {
        return Some(dim);
    }

    crate::wayland::shm::with_buffer_contents(buffer, |_, data| (data.width, data.height).into()).ok()
}
