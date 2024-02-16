//! Implementation of the rendering traits using pixman
use std::{
    collections::HashSet,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use pixman::{Filter, FormatCode, Image, Operation, Repeat};
use tracing::warn;

use crate::{
    backend::allocator::{
        dmabuf::{Dmabuf, DmabufMapping, DmabufMappingMode, DmabufSyncFailed, DmabufSyncFlags, WeakDmabuf},
        format::has_alpha,
        Buffer,
    },
    utils::{Buffer as BufferCoords, Physical, Rectangle, Scale, Size, Transform},
};

#[cfg(feature = "wayland_frontend")]
use crate::{
    backend::renderer::{ImportDmaWl, ImportMemWl},
    wayland::{compositor::SurfaceData, shm},
};
#[cfg(feature = "wayland_frontend")]
use wayland_server::{protocol::wl_buffer, Resource, Weak};

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use super::ImportEgl;
use super::{
    sync::SyncPoint, Bind, DebugFlags, ExportMem, Frame, ImportDma, ImportMem, Offscreen, Renderer, Texture,
    TextureFilter, TextureMapping, Unbind,
};

mod error;

pub use error::*;

lazy_static::lazy_static! {
    static ref SUPPORTED_FORMATS: Vec<drm_fourcc::DrmFourcc> = {
        vec![
            #[cfg(target_endian = "little")]
            DrmFourcc::Rgb565,
            DrmFourcc::Xrgb8888,
            DrmFourcc::Argb8888,
            DrmFourcc::Xbgr8888,
            DrmFourcc::Abgr8888,
            DrmFourcc::Rgbx8888,
            DrmFourcc::Rgba8888,
            DrmFourcc::Bgrx8888,
            DrmFourcc::Bgra8888,
            #[cfg(target_endian = "little")]
            DrmFourcc::Xrgb2101010,
            #[cfg(target_endian = "little")]
            DrmFourcc::Argb2101010,
            #[cfg(target_endian = "little")]
            DrmFourcc::Xbgr2101010,
            #[cfg(target_endian = "little")]
            DrmFourcc::Abgr2101010,
        ]
    };
}

#[derive(Debug)]
enum PixmanTarget {
    Image { dmabuf: Dmabuf, image: PixmanImage },
    RenderBuffer(PixmanRenderBuffer),
}

impl PixmanTarget {
    fn image(&self) -> &pixman::Image<'static, 'static> {
        match self {
            PixmanTarget::Image { image, .. } => &image.0.image,
            PixmanTarget::RenderBuffer(render_buffer) => &render_buffer.0,
        }
    }
}

/// Offscreen render buffer
#[derive(Debug)]
pub struct PixmanRenderBuffer(pixman::Image<'static, 'static>);

impl From<pixman::Image<'static, 'static>> for PixmanRenderBuffer {
    fn from(value: pixman::Image<'static, 'static>) -> Self {
        Self(value)
    }
}

#[derive(Debug)]
struct PixmanDmabufMapping {
    dmabuf: WeakDmabuf,
    _mapping: DmabufMapping,
}

#[derive(Debug)]
struct PixmanImageInner {
    #[cfg(feature = "wayland_frontend")]
    buffer: Option<Weak<wl_buffer::WlBuffer>>,
    dmabuf: Option<PixmanDmabufMapping>,
    image: Image<'static, 'static>,
    _flipped: bool, /* TODO: What about flipped textures? */
}

#[derive(Debug, Clone)]
struct PixmanImage(Rc<PixmanImageInner>);

impl PixmanImage {
    #[profiling::function]
    fn accessor<'l>(&'l self) -> Result<TextureAccessor<'l>, PixmanError> {
        #[cfg(feature = "wayland_frontend")]
        let buffer = if let Some(buffer) = self.0.buffer.as_ref() {
            Some(buffer.upgrade().map_err(|_| PixmanError::BufferDestroyed)?)
        } else {
            None
        };

        let guard = if let Some(mapping) = self.0.dmabuf.as_ref() {
            let dmabuf = mapping.dmabuf.upgrade().ok_or(PixmanError::BufferDestroyed)?;
            Some(DmabufReadGuard::new(dmabuf)?)
        } else {
            None
        };

        Ok(TextureAccessor {
            #[cfg(feature = "wayland_frontend")]
            buffer,
            image: &self.0.image,
            _guard: guard,
        })
    }
}

/// A handle to a pixman texture
#[derive(Debug, Clone)]
pub struct PixmanTexture(PixmanImage);

impl From<pixman::Image<'static, 'static>> for PixmanTexture {
    fn from(image: pixman::Image<'static, 'static>) -> Self {
        Self(PixmanImage(Rc::new(PixmanImageInner {
            #[cfg(feature = "wayland_frontend")]
            buffer: None,
            dmabuf: None,
            _flipped: false,
            image,
        })))
    }
}

struct DmabufReadGuard {
    dmabuf: Dmabuf,
}

impl DmabufReadGuard {
    #[profiling::function]
    pub fn new(dmabuf: Dmabuf) -> Result<Self, DmabufSyncFailed> {
        dmabuf.sync_plane(0, DmabufSyncFlags::START | DmabufSyncFlags::READ)?;
        Ok(Self { dmabuf })
    }
}

impl Drop for DmabufReadGuard {
    #[profiling::function]
    fn drop(&mut self) {
        if let Err(err) = self
            .dmabuf
            .sync_plane(0, DmabufSyncFlags::END | DmabufSyncFlags::READ)
        {
            tracing::warn!(?err, "failed to end sync read");
        }
    }
}

struct TextureAccessor<'l> {
    #[cfg(feature = "wayland_frontend")]
    buffer: Option<wl_buffer::WlBuffer>,
    image: &'l Image<'static, 'static>,
    _guard: Option<DmabufReadGuard>,
}

impl<'l> TextureAccessor<'l> {
    fn with_image<F, R>(&self, f: F) -> Result<R, PixmanError>
    where
        F: FnOnce(&'l Image<'static, 'static>) -> R,
    {
        #[cfg(feature = "wayland_frontend")]
        if let Some(buffer) = self.buffer.as_ref() {
            // We only have a buffer in case the image was created from
            // a shm buffer. In this case we need to guard against SIGPIPE
            // when accessing the image
            return Ok(shm::with_buffer_contents(buffer, |_, _, _| f(self.image))?);
        }

        Ok(f(self.image))
    }
}

impl PixmanTexture {
    #[profiling::function]
    fn accessor<'l>(&'l self) -> Result<TextureAccessor<'l>, PixmanError> {
        self.0.accessor()
    }
}

impl Texture for PixmanTexture {
    fn width(&self) -> u32 {
        self.0 .0.image.width() as u32
    }

    fn height(&self) -> u32 {
        self.0 .0.image.height() as u32
    }

    fn format(&self) -> Option<DrmFourcc> {
        DrmFourcc::try_from(self.0 .0.image.format()).ok()
    }
}

/// Handle to the currently rendered frame during [`PixmanRenderer::render`](Renderer::render).
#[derive(Debug)]
pub struct PixmanFrame<'frame> {
    renderer: &'frame mut PixmanRenderer,

    transform: Transform,
    output_size: Size<i32, Physical>,
    size: Size<i32, Physical>,

    finished: AtomicBool,
}

impl<'frame> PixmanFrame<'frame> {
    fn draw_solid_color(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: [f32; 4],
        op: Operation,
        debug: DebugFlags,
    ) -> Result<(), PixmanError> {
        let target_image = self
            .renderer
            .target
            .as_ref()
            .ok_or(PixmanError::NoTargetBound)?
            .image();

        let solid = pixman::Solid::new(color).map_err(|_| PixmanError::Unsupported)?;

        let mut clip_region =
            pixman::Region32::init_rect(0, 0, self.output_size.w as u32, self.output_size.h as u32);

        let damage_boxes = damage
            .iter()
            .copied()
            .map(|mut rect| {
                rect.loc += dst.loc;

                let rect = self.transform.transform_rect_in(rect, &self.size);

                let p1 = rect.loc;
                let p2 = p1 + rect.size.to_point();
                pixman::Box32 {
                    x1: p1.x,
                    y1: p1.y,
                    x2: p2.x,
                    y2: p2.y,
                }
            })
            .collect::<Vec<_>>();
        let damage_region = pixman::Region32::init_rects(&damage_boxes);
        clip_region = clip_region.intersect(&damage_region);

        target_image.set_clip_region32(Some(&clip_region))?;

        target_image.composite32(
            op,
            &solid,
            None,
            (0, 0),
            (0, 0),
            (0, 0),
            (target_image.width() as i32, target_image.height() as i32),
        );

        if debug.contains(DebugFlags::TINT) {
            target_image.composite32(
                Operation::Over,
                &self.renderer.tint,
                None,
                (0, 0),
                (0, 0),
                (0, 0),
                (target_image.width() as i32, target_image.height() as i32),
            );
        }

        target_image.set_clip_region32(None)?;

        Ok(())
    }
}

impl<'frame> Frame for PixmanFrame<'frame> {
    type Error = PixmanError;

    type TextureId = PixmanTexture;

    fn id(&self) -> usize {
        0
    }

    #[profiling::function]
    fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        self.draw_solid_color(
            Rectangle::from_loc_and_size((0, 0), self.size),
            at,
            color,
            Operation::Src,
            DebugFlags::empty(),
        )
    }

    #[profiling::function]
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: [f32; 4],
    ) -> Result<(), Self::Error> {
        let op = if color[3] == 1f32 {
            Operation::Src
        } else {
            Operation::Over
        };
        self.draw_solid_color(dst, damage, color, op, self.renderer.debug_flags)
    }

    #[profiling::function]
    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        let target_image = self
            .renderer
            .target
            .as_ref()
            .ok_or(PixmanError::NoTargetBound)?
            .image();
        let src_image_accessor = texture.accessor()?;

        let dst_loc = dst.loc;
        let dst = self.transform.transform_rect_in(dst, &self.size);

        // Our renderer works with clock-wise rotation, but the scr_transform in contrast to
        // the output transform is specified counter-clock-wise.
        let src_transform = src_transform.invert();

        let src: Rectangle<i32, BufferCoords> = src.to_i32_up::<i32>();

        let image_transform = match (src_transform, self.transform) {
            (Transform::Normal, output_transform) => output_transform,

            (Transform::_90, Transform::Normal) => Transform::_270,
            (Transform::_90, Transform::_90) => Transform::Normal,
            (Transform::_90, Transform::_180) => Transform::_90,
            (Transform::_90, Transform::_270) => Transform::_180,
            (Transform::_90, Transform::Flipped) => Transform::Flipped90,
            (Transform::_90, Transform::Flipped90) => Transform::Flipped180,
            (Transform::_90, Transform::Flipped180) => Transform::Flipped270,
            (Transform::_90, Transform::Flipped270) => Transform::Flipped,

            (Transform::_180, Transform::Normal) => Transform::_180,
            (Transform::_180, Transform::_90) => Transform::_270,
            (Transform::_180, Transform::_180) => Transform::Normal,
            (Transform::_180, Transform::_270) => Transform::_90,
            (Transform::_180, Transform::Flipped) => Transform::Flipped180,
            (Transform::_180, Transform::Flipped90) => Transform::Flipped270,
            (Transform::_180, Transform::Flipped180) => Transform::Flipped,
            (Transform::_180, Transform::Flipped270) => Transform::Flipped90,

            (Transform::_270, Transform::Normal) => Transform::_90,
            (Transform::_270, Transform::_90) => Transform::_180,
            (Transform::_270, Transform::_180) => Transform::_270,
            (Transform::_270, Transform::_270) => Transform::Normal,
            (Transform::_270, Transform::Flipped) => Transform::Flipped270,
            (Transform::_270, Transform::Flipped90) => Transform::Flipped,
            (Transform::_270, Transform::Flipped180) => Transform::Flipped90,
            (Transform::_270, Transform::Flipped270) => Transform::Flipped180,

            (Transform::Flipped, Transform::Normal) => Transform::Flipped,
            (Transform::Flipped, Transform::_90) => Transform::Flipped90,
            (Transform::Flipped, Transform::_180) => Transform::Flipped180,
            (Transform::Flipped, Transform::_270) => Transform::Flipped270,
            (Transform::Flipped, Transform::Flipped) => Transform::Normal,
            (Transform::Flipped, Transform::Flipped90) => Transform::_90,
            (Transform::Flipped, Transform::Flipped180) => Transform::_180,
            (Transform::Flipped, Transform::Flipped270) => Transform::_270,

            (Transform::Flipped90, Transform::Normal) => Transform::Flipped90,
            (Transform::Flipped90, Transform::_90) => Transform::Flipped180,
            (Transform::Flipped90, Transform::_180) => Transform::Flipped270,
            (Transform::Flipped90, Transform::_270) => Transform::Flipped,
            (Transform::Flipped90, Transform::Flipped) => Transform::_270,
            (Transform::Flipped90, Transform::Flipped90) => Transform::Normal,
            (Transform::Flipped90, Transform::Flipped180) => Transform::_90,
            (Transform::Flipped90, Transform::Flipped270) => Transform::_180,

            (Transform::Flipped180, Transform::Normal) => Transform::Flipped180,
            (Transform::Flipped180, Transform::_90) => Transform::Flipped270,
            (Transform::Flipped180, Transform::_180) => Transform::Flipped,
            (Transform::Flipped180, Transform::_270) => Transform::Flipped90,
            (Transform::Flipped180, Transform::Flipped) => Transform::_180,
            (Transform::Flipped180, Transform::Flipped90) => Transform::_270,
            (Transform::Flipped180, Transform::Flipped180) => Transform::Normal,
            (Transform::Flipped180, Transform::Flipped270) => Transform::_90,

            (Transform::Flipped270, Transform::Normal) => Transform::Flipped270,
            (Transform::Flipped270, Transform::_90) => Transform::Flipped,
            (Transform::Flipped270, Transform::_180) => Transform::Flipped90,
            (Transform::Flipped270, Transform::_270) => Transform::Flipped180,
            (Transform::Flipped270, Transform::Flipped) => Transform::_90,
            (Transform::Flipped270, Transform::Flipped90) => Transform::_180,
            (Transform::Flipped270, Transform::Flipped180) => Transform::_270,
            (Transform::Flipped270, Transform::Flipped270) => Transform::Normal,
        };

        let dst_src_size = image_transform.transform_size(src.size);
        let scale = dst_src_size.to_f64() / dst.size.to_f64();

        let (src_x, src_y, dest_x, dest_y, width, height, transform) =
            if image_transform != Transform::Normal || scale != Scale::from(1f64) {
                let mut transform = pixman::Transform::identity();

                // compensate for offset
                transform = transform
                    .translate(-dst.loc.x, -dst.loc.y, false)
                    .ok_or(PixmanError::Unsupported)?;

                // scale to src image size
                transform = transform
                    .scale(scale.x, scale.y, false)
                    .ok_or(PixmanError::Unsupported)?;

                let (cos, sin, x, y) = match image_transform {
                    Transform::Normal => (1, 0, 0, 0),
                    Transform::_90 => (0, -1, 0, src.size.h),
                    Transform::_180 => (-1, 0, src.size.w, src.size.h),
                    Transform::_270 => (0, 1, src.size.w, 0),
                    Transform::Flipped => (1, 0, src.size.w, 0),
                    Transform::Flipped90 => (0, -1, src.size.w, src.size.h),
                    Transform::Flipped180 => (-1, 0, 0, src.size.h),
                    Transform::Flipped270 => (0, 1, 0, 0),
                };

                // rotation
                transform = transform
                    .rotate(cos, sin, false)
                    .ok_or(PixmanError::Unsupported)?;

                // flipped
                if image_transform.flipped() {
                    transform = transform.scale(-1, 1, false).ok_or(PixmanError::Unsupported)?;
                }

                // Compensate rotation and flipped
                transform = transform.translate(x, y, false).ok_or(PixmanError::Unsupported)?;

                // crop src
                transform = transform
                    .translate(src.loc.x, src.loc.y, false)
                    .ok_or(PixmanError::Unsupported)?;

                (
                    0,
                    0,
                    0,
                    0,
                    target_image.width() as i32,
                    target_image.height() as i32,
                    Some(transform),
                )
            } else {
                (
                    src.loc.x, src.loc.y, dst.loc.x, dst.loc.y, src.size.w, src.size.h, None,
                )
            };

        let mut clip_region =
            pixman::Region32::init_rect(0, 0, self.output_size.w as u32, self.output_size.h as u32)
                .intersect(&pixman::Region32::init_rect(
                    dst.loc.x,
                    dst.loc.y,
                    dst.size.w as u32,
                    dst.size.h as u32,
                ));

        let damage_boxes = damage
            .iter()
            .copied()
            .map(|mut rect| {
                rect.loc += dst_loc;

                let rect = self.transform.transform_rect_in(rect, &self.size);

                let p1 = rect.loc;
                let p2 = p1 + rect.size.to_point();
                pixman::Box32 {
                    x1: p1.x,
                    y1: p1.y,
                    x2: p2.x,
                    y2: p2.y,
                }
            })
            .collect::<Vec<_>>();
        let damage_region = pixman::Region32::init_rects(&damage_boxes);
        clip_region = clip_region.intersect(&damage_region);

        target_image.set_clip_region32(Some(&clip_region))?;

        src_image_accessor.with_image(|src_image| {
            if let Some(transform) = transform {
                src_image.set_transform(transform)?;
            } else {
                src_image.clear_transform()?;
            }

            let filter = match self.renderer.upscale_filter {
                TextureFilter::Linear => Filter::Bilinear,
                TextureFilter::Nearest => Filter::Nearest,
            };

            src_image.set_filter(filter, &[])?;
            src_image.set_repeat(Repeat::None);

            let has_alpha = DrmFourcc::try_from(src_image.format())
                .ok()
                .map(has_alpha)
                .unwrap_or(true);

            let op = if has_alpha {
                Operation::Over
            } else {
                Operation::Src
            };

            let mask = if alpha != 1f32 {
                Some(pixman::Solid::new([0f32, 0f32, 0f32, alpha]).map_err(|_| PixmanError::Unsupported)?)
            } else {
                None
            };

            target_image.composite32(
                op,
                src_image,
                mask.as_deref(),
                (src_x, src_y),
                (0, 0),
                (dest_x, dest_y),
                (width, height),
            );

            src_image.clear_transform()?;

            Result::<(), PixmanError>::Ok(())
        })??;

        if self.renderer.debug_flags.contains(DebugFlags::TINT) {
            target_image.composite32(
                Operation::Over,
                &self.renderer.tint,
                None,
                (0, 0),
                (0, 0),
                (0, 0),
                (target_image.width() as i32, target_image.height() as i32),
            );
        }

        target_image.set_clip_region32(None)?;

        Ok(())
    }

    fn transformation(&self) -> Transform {
        self.transform
    }

    #[profiling::function]
    fn finish(mut self) -> Result<super::sync::SyncPoint, Self::Error> {
        self.finish_internal()
    }
}

impl<'frame> PixmanFrame<'frame> {
    #[profiling::function]
    fn finish_internal(&mut self) -> Result<SyncPoint, PixmanError> {
        if self.finished.swap(true, Ordering::SeqCst) {
            return Ok(SyncPoint::signaled());
        }

        if let PixmanTarget::Image { dmabuf, .. } =
            self.renderer.target.as_ref().ok_or(PixmanError::NoTargetBound)?
        {
            dmabuf
                .sync_plane(
                    0,
                    DmabufSyncFlags::END | DmabufSyncFlags::READ | DmabufSyncFlags::WRITE,
                )
                .map_err(PixmanError::Sync)?;
        }

        Ok(SyncPoint::signaled())
    }
}

impl<'frame> Drop for PixmanFrame<'frame> {
    fn drop(&mut self) {
        match self.finish_internal() {
            Ok(sync) => {
                sync.wait();
            }
            Err(err) => {
                warn!("Ignored error finishing PixmanFrame on drop: {}", err);
            }
        }
    }
}

/// A renderer utilizing pixman
#[derive(Debug)]
pub struct PixmanRenderer {
    target: Option<PixmanTarget>,
    downscale_filter: TextureFilter,
    upscale_filter: TextureFilter,
    debug_flags: DebugFlags,
    tint: pixman::Solid<'static>,

    // caches
    buffers: Vec<PixmanImage>,
    dmabuf_cache: Vec<PixmanImage>,
}

impl PixmanRenderer {
    /// Creates a new pixman renderer
    pub fn new() -> Result<Self, PixmanError> {
        let tint = pixman::Solid::new([0.0, 0.3, 0.0, 0.2]).map_err(|_| PixmanError::Unsupported)?;
        Ok(Self {
            target: None,
            downscale_filter: TextureFilter::Linear,
            upscale_filter: TextureFilter::Linear,
            debug_flags: DebugFlags::empty(),
            tint,

            buffers: Default::default(),
            dmabuf_cache: Default::default(),
        })
    }
}

impl PixmanRenderer {
    fn existing_dmabuf(&self, dmabuf: &Dmabuf) -> Option<PixmanImage> {
        self.dmabuf_cache
            .iter()
            .find(|image| {
                image
                    .0
                    .dmabuf
                    .as_ref()
                    .and_then(|map| map.dmabuf.upgrade().map(|buf| &buf == dmabuf))
                    .unwrap_or(false)
            })
            .cloned()
    }

    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        mode: DmabufMappingMode,
    ) -> Result<PixmanImage, PixmanError> {
        if dmabuf.num_planes() != 1 {
            return Err(PixmanError::UnsupportedNumberOfPlanes);
        }

        let size = dmabuf.size();
        let format = dmabuf.format();

        if format.modifier != DrmModifier::Linear {
            return Err(PixmanError::UnsupportedModifier(format.modifier));
        }
        let format = pixman::FormatCode::try_from(format.code)
            .map_err(|_| PixmanError::UnsupportedPixelFormat(format.code))?;

        let dmabuf_mapping = dmabuf.map_plane(0, mode)?;
        let stride = dmabuf.strides().next().expect("already checked") as usize;
        let expected_len = stride * size.h as usize;

        if dmabuf_mapping.length() < expected_len {
            return Err(PixmanError::IncompleteBuffer {
                expected: expected_len,
                actual: dmabuf_mapping.length(),
            });
        }

        let image: Image<'_, '_> = unsafe {
            pixman::Image::from_raw_mut(
                format,
                size.w as usize,
                size.h as usize,
                dmabuf_mapping.ptr() as *mut u32,
                stride,
                false,
            )
        }
        .map_err(|_| PixmanError::ImportFailed)?;

        Ok(PixmanImage(Rc::new(PixmanImageInner {
            #[cfg(feature = "wayland_frontend")]
            buffer: None,
            dmabuf: Some(PixmanDmabufMapping {
                dmabuf: dmabuf.weak(),
                _mapping: dmabuf_mapping,
            }),
            image,
            _flipped: false,
        })))
    }

    fn cleanup(&mut self) {
        self.dmabuf_cache.retain(|image| {
            image
                .0
                .dmabuf
                .as_ref()
                .map(|map| map.dmabuf.upgrade().is_some())
                .unwrap_or(false)
        });
        self.buffers.retain(|image| {
            image
                .0
                .dmabuf
                .as_ref()
                .map(|map| map.dmabuf.upgrade().is_some())
                .unwrap_or(false)
        });
    }
}

impl Renderer for PixmanRenderer {
    type Error = PixmanError;

    type TextureId = PixmanTexture;

    type Frame<'frame> = PixmanFrame<'frame>;

    fn id(&self) -> usize {
        0
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.downscale_filter = filter;
        Ok(())
    }

    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.upscale_filter = filter;
        Ok(())
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.debug_flags = flags;
    }

    fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }

    #[profiling::function]
    fn render(
        &mut self,
        output_size: Size<i32, Physical>,
        dst_transform: Transform,
    ) -> Result<PixmanFrame<'_>, Self::Error> {
        self.cleanup();

        if let PixmanTarget::Image { dmabuf, .. } = self.target.as_ref().ok_or(PixmanError::NoTargetBound)? {
            dmabuf
                .sync_plane(
                    0,
                    DmabufSyncFlags::START | DmabufSyncFlags::READ | DmabufSyncFlags::WRITE,
                )
                .map_err(PixmanError::Sync)?;
        }
        Ok(PixmanFrame {
            renderer: self,

            transform: dst_transform,
            output_size,
            size: dst_transform.transform_size(output_size),

            finished: AtomicBool::new(false),
        })
    }
}

impl ImportMem for PixmanRenderer {
    #[profiling::function]
    fn import_memory(
        &mut self,
        data: &[u8],
        format: drm_fourcc::DrmFourcc,
        size: Size<i32, BufferCoords>,
        flipped: bool,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        let format =
            pixman::FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedPixelFormat(format))?;
        let image = pixman::Image::new(format, size.w as usize, size.h as usize, false)
            .map_err(|_| PixmanError::Unsupported)?;
        let expected_len = image.stride() * image.height();
        if data.len() < expected_len {
            return Err(PixmanError::IncompleteBuffer {
                expected: expected_len,
                actual: data.len(),
            });
        }
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), image.data() as *mut u8, expected_len);
        }
        Ok(PixmanTexture(PixmanImage(Rc::new(PixmanImageInner {
            #[cfg(feature = "wayland_frontend")]
            buffer: None,
            dmabuf: None,
            image,
            _flipped: flipped,
        }))))
    }

    #[profiling::function]
    fn update_memory(
        &mut self,
        texture: &<Self as Renderer>::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoords>,
    ) -> Result<(), <Self as Renderer>::Error> {
        #[cfg(feature = "wayland_frontend")]
        if texture.0 .0.buffer.is_some() {
            return Err(PixmanError::ImportFailed);
        }

        if texture.0 .0.dmabuf.is_some() {
            return Err(PixmanError::ImportFailed);
        }

        let stride = texture.0 .0.image.stride();
        let expected_len = stride * texture.0 .0.image.height();

        if data.len() < expected_len {
            return Err(PixmanError::IncompleteBuffer {
                expected: expected_len,
                actual: data.len(),
            });
        }

        let src_image = unsafe {
            // SAFETY: As we are never going to write to this image
            // it is safe to cast the passed slice to a mut pointer
            pixman::Image::from_raw_mut(
                texture.0 .0.image.format(),
                texture.0 .0.image.width(),
                texture.0 .0.image.height(),
                data.as_ptr() as *mut _,
                stride,
                false,
            )
        }
        .map_err(|_| PixmanError::ImportFailed)?;

        texture.0 .0.image.composite32(
            Operation::Src,
            &src_image,
            None,
            region.loc.into(),
            (0, 0),
            region.loc.into(),
            region.size.into(),
        );

        Ok(())
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = drm_fourcc::DrmFourcc>> {
        Box::new(SUPPORTED_FORMATS.iter().copied())
    }
}

/// Texture mapping of a pixman texture
#[derive(Debug)]
pub struct PixmanMapping(pixman::Image<'static, 'static>);

impl Texture for PixmanMapping {
    fn width(&self) -> u32 {
        self.0.width() as u32
    }

    fn height(&self) -> u32 {
        self.0.height() as u32
    }

    fn format(&self) -> Option<DrmFourcc> {
        DrmFourcc::try_from(self.0.format()).ok()
    }
}

impl TextureMapping for PixmanMapping {
    fn flipped(&self) -> bool {
        false
    }
}

impl ExportMem for PixmanRenderer {
    type TextureMapping = PixmanMapping;

    #[profiling::function]
    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, BufferCoords>,
        format: DrmFourcc,
    ) -> Result<Self::TextureMapping, <Self as Renderer>::Error> {
        let format_code =
            pixman::FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedPixelFormat(format))?;
        let copy_image =
            pixman::Image::new(format_code, region.size.w as usize, region.size.h as usize, false)
                .map_err(|_| PixmanError::Unsupported)?;

        if let Some(target) = self.target.as_ref() {
            if let PixmanTarget::Image { dmabuf, .. } = target {
                dmabuf.sync_plane(0, DmabufSyncFlags::START | DmabufSyncFlags::READ)?;
            };
            copy_image.composite32(
                Operation::Src,
                target.image(),
                None,
                region.loc.into(),
                (0, 0),
                (0, 0),
                region.size.into(),
            );
            if let PixmanTarget::Image { dmabuf, .. } = target {
                dmabuf.sync_plane(0, DmabufSyncFlags::END | DmabufSyncFlags::READ)?;
            };
        }

        Ok(PixmanMapping(copy_image))
    }

    #[profiling::function]
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoords>,
        format: DrmFourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        let accessor = texture.accessor()?;
        let format_code =
            pixman::FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedPixelFormat(format))?;
        let copy_image =
            pixman::Image::new(format_code, region.size.w as usize, region.size.h as usize, false)
                .map_err(|_| PixmanError::Unsupported)?;
        accessor.with_image(|image| {
            copy_image.composite32(
                Operation::Src,
                image,
                None,
                region.loc.into(),
                (0, 0),
                (0, 0),
                region.size.into(),
            );
        })?;
        Ok(PixmanMapping(copy_image))
    }

    fn can_read_texture(&mut self, _texture: &Self::TextureId) -> Result<bool, Self::Error> {
        Ok(true)
    }

    #[profiling::function]
    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], <Self as Renderer>::Error> {
        Ok(unsafe {
            std::slice::from_raw_parts(
                texture_mapping.0.data() as *const u8,
                texture_mapping.0.stride() * texture_mapping.0.height(),
            )
        })
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl ImportEgl for PixmanRenderer {
    fn bind_wl_display(
        &mut self,
        _display: &wayland_server::DisplayHandle,
    ) -> Result<(), crate::backend::egl::Error> {
        Err(crate::backend::egl::Error::NoEGLDisplayBound)
    }

    fn unbind_wl_display(&mut self) {}

    fn egl_reader(&self) -> Option<&crate::backend::egl::display::EGLBufferReader> {
        None
    }

    fn import_egl_buffer(
        &mut self,
        _buffer: &wl_buffer::WlBuffer,
        _surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, BufferCoords>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        Err(PixmanError::Unsupported)
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportMemWl for PixmanRenderer {
    #[profiling::function]
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        _surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, BufferCoords>],
    ) -> Result<PixmanTexture, PixmanError> {
        let image = shm::with_buffer_contents(buffer, |ptr, len, data| {
            let format = FormatCode::try_from(
                shm::shm_format_to_fourcc(data.format)
                    .ok_or(PixmanError::UnsupportedWlPixelFormat(data.format))?,
            )
            .map_err(|_| PixmanError::UnsupportedWlPixelFormat(data.format))?;

            let expected_len = (data.offset + data.stride * data.height) as usize;
            if len < expected_len {
                return Err(PixmanError::IncompleteBuffer {
                    expected: expected_len,
                    actual: len,
                });
            }

            let image = unsafe {
                // SAFETY: We guarantee that this image is only used for reading,
                // so it is safe to cast the ptr to *mut
                Image::from_raw_mut(
                    format,
                    data.width as usize,
                    data.height as usize,
                    ptr.offset(data.offset as isize) as *mut u32,
                    data.stride as usize,
                    false,
                )
            }
            .map_err(|_| PixmanError::ImportFailed)?;
            std::result::Result::<_, PixmanError>::Ok(image)
        })??;
        Ok(PixmanTexture(PixmanImage(Rc::new(PixmanImageInner {
            buffer: Some(buffer.downgrade()),
            dmabuf: None,
            image,
            _flipped: false,
        }))))
    }
}

impl ImportDma for PixmanRenderer {
    #[profiling::function]
    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, BufferCoords>]>,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        if let Some(image) = self.existing_dmabuf(dmabuf) {
            return Ok(PixmanTexture(image));
        };

        let image = self.import_dmabuf(dmabuf, DmabufMappingMode::READ)?;
        self.dmabuf_cache.push(image.clone());
        Ok(PixmanTexture(image))
    }

    fn dmabuf_formats(&self) -> Box<dyn Iterator<Item = drm_fourcc::DrmFormat>> {
        lazy_static::lazy_static! {
            static ref DMABUF_FORMATS: Vec<DrmFormat> = {
                SUPPORTED_FORMATS.iter().copied().map(|code| DrmFormat {
                    code,
                    modifier: drm_fourcc::DrmModifier::Linear,
                }).collect()
            };
        }
        Box::new(DMABUF_FORMATS.iter().copied())
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for PixmanRenderer {}

impl Unbind for PixmanRenderer {
    #[profiling::function]
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        self.target = None;
        Ok(())
    }
}

impl Bind<Dmabuf> for PixmanRenderer {
    #[profiling::function]
    fn bind(&mut self, target: Dmabuf) -> Result<(), <Self as Renderer>::Error> {
        let existing_image = self
            .buffers
            .iter()
            .find(|image| {
                image
                    .0
                    .dmabuf
                    .as_ref()
                    .and_then(|map| map.dmabuf.upgrade().map(|buf| buf == target))
                    .unwrap_or(false)
            })
            .cloned();

        let image = if let Some(image) = existing_image {
            image
        } else {
            let image = self.import_dmabuf(&target, DmabufMappingMode::READ | DmabufMappingMode::WRITE)?;
            self.buffers.push(image.clone());
            image
        };

        self.target = Some(PixmanTarget::Image {
            dmabuf: target,
            image,
        });
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<DrmFormat>> {
        lazy_static::lazy_static! {
            static ref DMABUF_FORMATS: HashSet<DrmFormat> = {
                SUPPORTED_FORMATS.iter().copied().map(|code| DrmFormat {
                    code,
                    modifier: DrmModifier::Linear,
                }).collect()
            };
        }
        Some(DMABUF_FORMATS.clone())
    }
}

impl Offscreen<PixmanRenderBuffer> for PixmanRenderer {
    #[profiling::function]
    fn create_buffer(
        &mut self,
        format: DrmFourcc,
        size: Size<i32, BufferCoords>,
    ) -> Result<PixmanRenderBuffer, <Self as Renderer>::Error> {
        let format_code =
            FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedPixelFormat(format))?;
        let image = pixman::Image::new(format_code, size.w as usize, size.h as usize, true)
            .map_err(|_| PixmanError::Unsupported)?;
        Ok(PixmanRenderBuffer::from(image))
    }
}

impl Bind<PixmanRenderBuffer> for PixmanRenderer {
    #[profiling::function]
    fn bind(&mut self, target: PixmanRenderBuffer) -> Result<(), <Self as Renderer>::Error> {
        self.target = Some(PixmanTarget::RenderBuffer(target));
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<DrmFormat>> {
        lazy_static::lazy_static! {
            static ref RENDER_BUFFER_FORMATS: HashSet<DrmFormat> = {
                SUPPORTED_FORMATS.iter().copied().map(|code| DrmFormat {
                    code,
                    modifier: DrmModifier::Linear,
                }).collect()
            };
        }
        Some(RENDER_BUFFER_FORMATS.clone())
    }
}
