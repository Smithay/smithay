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
use std::fmt;

use crate::utils::{Buffer as BufferCoord, Physical, Point, Rectangle, Scale, Size, Transform};
use cgmath::Matrix3;

#[cfg(feature = "wayland_frontend")]
use crate::wayland::{compositor::SurfaceData, shm::fourcc_to_shm_format};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

#[cfg(feature = "renderer_gl")]
pub mod gles;

#[cfg(feature = "renderer_glow")]
pub mod glow;

#[cfg(feature = "renderer_pixman")]
pub mod pixman;

use crate::backend::allocator::{dmabuf::Dmabuf, Format, Fourcc};
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

pub mod utils;

pub mod element;

pub mod damage;

pub mod sync;

#[cfg(feature = "renderer_test")]
pub mod test;

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
    #[inline]
    pub fn matrix(&self) -> Matrix3<f32> {
        match self {
            Transform::Normal => Matrix3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_90 => Matrix3::new(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_180 => Matrix3::new(-1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_270 => Matrix3::new(0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped => Matrix3::new(-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped90 => Matrix3::new(0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped180 => Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped270 => Matrix3::new(0.0, -1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl From<wayland_server::protocol::wl_output::Transform> for Transform {
    #[inline]
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
pub trait Texture: fmt::Debug {
    /// Size of the texture plane
    fn size(&self) -> Size<i32, BufferCoord> {
        Size::from((self.width() as i32, self.height() as i32))
    }

    /// Width of the texture plane
    fn width(&self) -> u32;
    /// Height of the texture plane
    fn height(&self) -> u32;

    /// Format of the texture, if available.
    ///
    /// In case the format is hidden by the implementation,
    /// it should be assumed, that the pixel representation cannot be read.
    ///
    /// Thus [`ExportMem::copy_texture`], if implemented, will not succeed for this texture.
    /// Note that this does **not** mean every texture with a format is guaranteed to be copyable.
    fn format(&self) -> Option<Fourcc>;
}

/// A downloaded texture buffer
pub trait TextureMapping: Texture {
    /// Returns if the mapped buffer is flipped on the y-axis
    /// (compared to the lower left being (0, 0))
    fn flipped(&self) -> bool;

    /// Format of the texture
    fn format(&self) -> Fourcc {
        Texture::format(self).expect("Texture Mappings need to have a format")
    }
}

/// Helper trait for [`Renderer`], which defines a rendering api for a currently in-progress frame during [`Renderer::render`].
pub trait Frame {
    /// Error type returned by the rendering operations of this renderer.
    type Error: Error;
    /// Texture Handle type used by this renderer.
    type TextureId: Texture;

    /// Returns an id, that is unique to all renderers, that can use
    /// `TextureId`s originating from any of these renderers.
    fn id(&self) -> usize;

    /// Clear the complete current target with a single given color.
    ///
    /// The `at` parameter specifies a set of rectangles to clear in the current target. This allows partially
    /// clearing the target which may be useful for damaged rendering.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error>;

    /// Draw a solid color to the current target at the specified destination with the specified color.
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: [f32; 4],
    ) -> Result<(), Self::Error>;

    /// Render a texture to the current target as a flat 2d-plane at a given
    /// position and applying the given transformation with the given alpha value.
    /// (Meaning `src_transform` should match the orientation of surface being rendered).
    #[allow(clippy::too_many_arguments)]
    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: Point<i32, Physical>,
        texture_scale: i32,
        output_scale: impl Into<Scale<f64>>,
        src_transform: Transform,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        alpha: f32,
    ) -> Result<(), Self::Error> {
        self.render_texture_from_to(
            texture,
            Rectangle::from_loc_and_size(Point::<i32, BufferCoord>::from((0, 0)), texture.size()).to_f64(),
            Rectangle::from_loc_and_size(
                pos,
                texture
                    .size()
                    .to_logical(texture_scale, src_transform)
                    .to_physical_precise_round(output_scale),
            ),
            damage,
            opaque_regions,
            src_transform,
            alpha,
        )
    }

    /// Render part of a texture as given by src to the current target into the rectangle described by dst
    /// as a flat 2d-plane after applying the inverse of the given transformation.
    /// (Meaning `src_transform` should match the orientation of surface being rendered).
    #[allow(clippy::too_many_arguments)]
    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error>;

    /// Output transformation that is applied to this frame
    fn transformation(&self) -> Transform;

    /// Wait for a [`SyncPoint`](sync::SyncPoint) to be signaled
    fn wait(&mut self, sync: &sync::SyncPoint) -> Result<(), Self::Error>;

    /// Finish this [`Frame`] returning any error that may happen during any cleanup.
    ///
    /// Dropping the frame instead may result in any of the following and is implementation dependent:
    /// - All actions done to the frame vanish and are never executed
    /// - A partial renderer with undefined framebuffer contents occurs
    /// - All actions are performed as normal without errors being returned.
    ///
    /// Leaking the frame instead will leak resources and can cause any of the previous effects.
    /// Leaking might make the renderer return Errors and force it's recreation.
    /// Leaking may not cause otherwise undefined behavior and program execution will always continue normally.
    fn finish(self) -> Result<sync::SyncPoint, Self::Error>;
}

bitflags::bitflags! {
    /// Debug flags that can be enabled at runtime
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct DebugFlags: u32 {
        /// Tint all rendered textures
        const TINT = 0b00000001;
    }
}
/// Abstraction of commonly used rendering operations for compositors.
pub trait Renderer: fmt::Debug {
    /// Error type returned by the rendering operations of this renderer.
    type Error: Error;
    /// Texture Handle type used by this renderer.
    type TextureId: Texture;
    /// Type representing a currently in-progress frame during the [`Renderer::render`]-call
    type Frame<'frame>: Frame<Error = Self::Error, TextureId = Self::TextureId> + 'frame
    where
        Self: 'frame;

    /// Returns an id, that is unique to all renderers, that can use
    /// `TextureId`s originating from any of these renderers.
    fn id(&self) -> usize;

    /// Set the filter method to be used when rendering a texture into a smaller area than its size
    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error>;
    /// Set the filter method to be used when rendering a texture into a larger area than its size
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error>;

    /// Set the enabled [`DebugFlags`]
    fn set_debug_flags(&mut self, flags: DebugFlags);
    /// Returns the current enabled [`DebugFlags`]
    fn debug_flags(&self) -> DebugFlags;

    /// Initialize a rendering context on the current rendering target with given dimensions and transformation.
    ///
    /// The `output_size` specifies the dimensions of the display **before** the `dst_transform` is
    /// applied.
    ///
    /// This function *may* error, if:
    /// - The given dimensions are unsupported (too large) for this renderer
    /// - The given Transformation is not supported by the renderer (`Transform::Normal` is always supported).
    /// - This renderer implements `Bind`, no target was bound *and* has no default target.
    /// - (Renderers not implementing `Bind` always have a default target.)
    fn render(
        &mut self,
        output_size: Size<i32, Physical>,
        dst_transform: Transform,
    ) -> Result<Self::Frame<'_>, Self::Error>;

    /// Wait for a [`SyncPoint`](sync::SyncPoint) to be signaled
    fn wait(&mut self, sync: &sync::SyncPoint) -> Result<(), Self::Error>;
}

/// Trait for renderers that support creating offscreen framebuffers to render into.
///
/// Usually also implement [`ExportMem`] to receive the framebuffers contents.
pub trait Offscreen<Target>: Renderer + Bind<Target> {
    /// Create a new instance of a framebuffer.
    ///
    /// This call *may* fail, if (but not limited to):
    /// - The maximum amount of framebuffers for this renderer would be exceeded
    /// - The format is not supported to be rendered into
    /// - The size is too large for a framebuffer
    fn create_buffer(
        &mut self,
        format: Fourcc,
        size: Size<i32, BufferCoord>,
    ) -> Result<Target, <Self as Renderer>::Error>;
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
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;

    /// Returns supported formats for shared memory buffers.
    ///
    /// Will always contain At least `Argb8888` and `Xrgb8888`.
    fn shm_formats(&self) -> Box<dyn Iterator<Item = wl_shm::Format>> {
        Box::new(self.mem_formats().flat_map(fourcc_to_shm_format))
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
    /// The provided data slice needs to be in a format supported as indicated by [`ImportMem::mem_formats`].
    /// Its length should thus be `size.w * size.h * bits_per_pixel`.
    /// Anything beyond will be truncated, if the buffer is too small an error will be returned.
    fn import_memory(
        &mut self,
        data: &[u8],
        format: Fourcc,
        size: Size<i32, BufferCoord>,
        flipped: bool,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;

    /// Update a portion of a given chunk of memory into an existing texture.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The provided data slice needs to be in the same format used to create the texture and the same size of the texture.
    /// Its length should this be `texture.size().w * texture.size().h * bits_per_pixel`.
    /// Anything beyond will be ignored, if the buffer is too small an error will be returned.
    ///
    /// This function *may* error, if (but not limited to):
    /// - The texture was not created using either [`ImportMemWl::import_shm_buffer`] or [`ImportMem::import_memory`].
    ///   External textures imported by other means (e.g. via ImportDma) may not be writable. This property is defined
    ///   by the implementation.
    /// - The region is out of bounds of the initial size the texture was created with. Implementations are not required
    ///   to support resizing the original texture.
    fn update_memory(
        &mut self,
        texture: &<Self as Renderer>::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<(), <Self as Renderer>::Error>;

    /// Returns supported formats for memory imports.
    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>>;
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
    fn bind_wl_display(&mut self, display: &wayland_server::DisplayHandle) -> Result<(), EglError>;

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
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;
}

#[cfg(feature = "wayland_frontend")]
/// Trait for Renderers supporting importing dmabuf-based wl_buffers
pub trait ImportDmaWl: ImportDma {
    /// Import a given dmabuf-based buffer into the renderer (see [`buffer_type`]).
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
    fn import_dma_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        _surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        let dmabuf = crate::wayland::dmabuf::get_dmabuf(buffer)
            .expect("import_dma_buffer without checking buffer type?");
        self.import_dmabuf(dmabuf, Some(damage))
    }
}

/// Trait for Renderers supporting importing dmabufs.
pub trait ImportDma: Renderer {
    /// Returns supported formats for dmabufs.
    fn dmabuf_formats(&self) -> Box<dyn Iterator<Item = Format>> {
        Box::new(std::iter::empty())
    }

    /// Test if a specific dmabuf [`Format`] is supported
    fn has_dmabuf_format(&self, format: Format) -> bool {
        self.dmabuf_formats().any(|f| f == format)
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
        damage: Option<&[Rectangle<i32, BufferCoord>]>,
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
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Option<Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>>;
}

// TODO: Do this with specialization, when possible and do default implementations
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl<R: Renderer + ImportMemWl + ImportEgl + ImportDmaWl> ImportAll for R {
    #[profiling::function]
    fn import_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
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
        damage: &[Rectangle<i32, BufferCoord>],
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
    /// - It is not possible to convert the framebuffer into the provided format.
    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, <Self as Renderer>::Error>;

    /// Copies the contents of the passed texture.
    /// *Note*: This function may change or invalidate the current bind.
    ///
    /// Renderers are not required to support any format other than what was returned by `Texture::format`.
    /// This operation is not destructive, the contents of the texture keep being valid.
    ///
    /// This function *may* fail, if:
    /// - There is not enough space to create the mapping
    /// - The texture does no allow copying for implementation-specfic reasons
    /// - It is not possible to convert the texture into the provided format.
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error>;

    /// Returns whether the renderer should be able to read-back from the given texture.
    ///
    /// No actual copying shall be performed by this function nor is a format specified,
    /// so it is still legal for [`ExportMem::copy_texture`] to return an error, if this
    /// method returns `true`.
    ///
    /// This function *may* fail, if:
    /// - A readability test did successfully complete (not that it returned `unreadble`!)
    /// - Any of the state of the renderer is irrevesibly changed
    fn can_read_texture(&mut self, texture: &Self::TextureId) -> Result<bool, Self::Error>;

    /// Returns a read-only pointer to a previously created texture mapping.
    ///
    /// The format of the returned slice is given by [`Texture::format`] of the texture mapping.
    ///
    /// This function *may* fail, if (but not limited to):
    /// - There is not enough space in memory
    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], <Self as Renderer>::Error>;
}

/// Trait for renderers supporting blitting contents from one framebuffer to another.
pub trait Blit<Target>
where
    Self: Renderer + Bind<Target>,
{
    /// Copies the contents of `src` in the current bound framebuffer to `dst` in Target,
    /// applying `filter` if necessary.
    ///
    /// This operation is non destructive, the contents of the source framebuffer
    /// are kept intact as is any region not in `dst` for the target framebuffer.
    ///
    /// This operation needs a bound or default rendering target.
    /// The currently bound target is guaranteed to still be active after this operation.
    ///
    /// This function *may* fail, if (but not limited to):
    /// - The source framebuffer is not readable / unset
    /// - The destination framebuffer is not writable
    /// - `src` is out of bounds for the source framebuffer
    /// - `dst` is out of bounds for the destination framebuffer
    /// - `src` and `dst` sizes are different and interpolation id not supported by this renderer.
    /// - source and target framebuffer are the same, and `src` and `dst` overlap
    fn blit_to(
        &mut self,
        to: Target,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), <Self as Renderer>::Error>;

    /// Copies the contents of `src` in Target to `dst` of the current bound framebuffer,
    /// applying `filter` if necessary.
    ///
    /// This operation is non destructive, the contents of the source framebuffer
    /// are kept intact as is any region not in `dst` for the target framebuffer.
    ///
    /// This operation needs a bound or default rendering target.
    /// The currently bound target is guaranteed to still be active after this operation.
    ///
    /// This function *may* fail, if (but not limited to):
    /// - The source framebuffer is not readable
    /// - The destination framebuffer is not writable / unset
    /// - `src` is out of bounds for the source framebuffer
    /// - `dst` is out of bounds for the destination framebuffer
    /// - `src` and `dst` sizes are different and interpolation id not supported by this renderer.
    /// - source and target framebuffer are the same, and `src` and `dst` overlap
    fn blit_from(
        &mut self,
        from: Target,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), <Self as Renderer>::Error>;
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
    use crate::wayland::shm::BufferAccessError;

    if crate::wayland::dmabuf::get_dmabuf(buffer).is_ok() {
        return Some(BufferType::Dma);
    }

    if !matches!(
        crate::wayland::shm::with_buffer_contents(buffer, |_, _, _| ()),
        Err(BufferAccessError::NotManaged)
    ) {
        return Some(BufferType::Shm);
    }

    // Not managed, check if this is an EGLBuffer
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

    None
}

/// Returns if the buffer has an alpha channel
///
/// Returns `None` if the type is not known to smithay
/// or otherwise not supported (e.g. not initialized using one of smithays [`crate::wayland`]-handlers).
///
/// Note: This is on a best-effort, but will never return false for a buffer
/// with a format that supports alpha.
#[cfg(feature = "wayland_frontend")]
pub fn buffer_has_alpha(buffer: &wl_buffer::WlBuffer) -> Option<bool> {
    use super::allocator::format::has_alpha;
    use crate::wayland::shm::shm_format_to_fourcc;

    if let Ok(dmabuf) = crate::wayland::dmabuf::get_dmabuf(buffer) {
        return Some(crate::backend::allocator::format::has_alpha(dmabuf.0.format));
    }

    if let Ok(has_alpha) = crate::wayland::shm::with_buffer_contents(buffer, |_, _, data| {
        shm_format_to_fourcc(data.format).map_or(false, has_alpha)
    }) {
        return Some(has_alpha);
    }

    // Not managed, check if this is an EGLBuffer
    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    if let Some(format) = BUFFER_READER
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|x| x.upgrade())
        .and_then(|x| x.egl_buffer_contents(buffer).ok())
        .map(|b| b.format)
    {
        return Some(crate::backend::egl::display::EGLBufferReader::egl_buffer_has_alpha(format));
    }

    None
}

/// Returns the dimensions of a wl_buffer
///
/// *Note*: This will only return dimensions for buffer types known to smithay (see [`buffer_type`])
#[cfg(feature = "wayland_frontend")]
pub fn buffer_dimensions(buffer: &wl_buffer::WlBuffer) -> Option<Size<i32, BufferCoord>> {
    use crate::{
        backend::allocator::Buffer,
        wayland::shm::{self, BufferAccessError},
    };

    if let Ok(buf) = crate::wayland::dmabuf::get_dmabuf(buffer) {
        return Some((buf.width() as i32, buf.height() as i32).into());
    }

    match shm::with_buffer_contents(buffer, |_, _, data| (data.width, data.height).into()) {
        Ok(data) => Some(data),

        Err(BufferAccessError::NotManaged) => {
            // Not managed, check if this is an EGLBuffer
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

            None
        }

        Err(_) => None,
    }
}

/// Returns if the underlying buffer is y-inverted
///
/// *Note*: This will only return y-inverted for buffer types known to smithay (see [`buffer_type`])
#[cfg(feature = "wayland_frontend")]
#[profiling::function]
pub fn buffer_y_inverted(buffer: &wl_buffer::WlBuffer) -> Option<bool> {
    if let Ok(dmabuf) = crate::wayland::dmabuf::get_dmabuf(buffer) {
        return Some(dmabuf.y_inverted());
    }

    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    if let Some(Ok(egl_buffer)) = BUFFER_READER
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|x| x.upgrade())
        .map(|x| x.egl_buffer_contents(buffer))
    {
        return Some(egl_buffer.y_inverted);
    }

    None
}
