//! Element to render from RGBA memory
//!
//! # Why use this implementation
//!
//! The [`MemoryRenderBuffer`] allows for easy integration of software-rendered elements
//! in the smithay rendering pipeline. As software-rendered elements eventually have to
//! upload to the GPU for rendering damage tracking is a crucial part. The [`MemoryRenderBuffer`]
//! allows for efficient damage tracking by providing a [`RenderContext`] which accumulates the
//! software-rendering damage. It automatically uploads the damaged parts to a [`RendererSuper::TextureId`](crate::backend::renderer::RendererSuper::TextureId)
//! during rendering.
//!
//! # Why **not** to use this implementation
//!
//! As described earlier the [`MemoryRenderBuffer`] is targeted at software rendering, if you have
//! some static content you may want to take a look at [`TextureBuffer`](super::texture::TextureBuffer)
//! or [`TextureRenderBuffer`](super::texture::TextureRenderBuffer) for hardware accelerated rendering.
//!
//! # How to use it
//!
//! The [`MemoryRenderBuffer`] represents a buffer of your data and holds the damage as reported by the [`RenderContext`].
//! To render the buffer you have to create a [`MemoryRenderBufferRenderElement`] in your render loop as
//! shown in the example.
//!
//! ```no_run
//! # use smithay::{
//! #     backend::renderer::{Color32F, DebugFlags, Frame, ImportMem, Renderer, Texture, TextureFilter, sync::SyncPoint, test::{DummyRenderer, DummyFramebuffer}},
//! #     utils::{Buffer, Physical},
//! # };
//! use std::time::{Duration, Instant};
//!
//! use smithay::{
//!     backend::{
//!         allocator::Fourcc,
//!         renderer::{
//!             damage::OutputDamageTracker,
//!             element::{
//!                 Kind,
//!                 memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
//!             },
//!         },
//!     },
//!     utils::{Point, Rectangle, Size, Transform},
//! };
//!
//! const WIDTH: i32 = 10;
//! const HEIGHT: i32 = 10;
//!
//! // Initialize a empty render buffer
//! let mut buffer = MemoryRenderBuffer::new(Fourcc::Argb8888, (WIDTH, HEIGHT), 1, Transform::Normal, None);
//!
//! // Create a rendering context
//! let mut render_context = buffer.render();
//!
//! // Draw to the buffer
//! render_context.draw(|buffer| {
//!     buffer.chunks_exact_mut(4).for_each(|chunk| {
//!         chunk.copy_from_slice(&[255, 231, 199, 255]);
//!     });
//!
//!     // Return the whole buffer as damage
//!     Result::<_, ()>::Ok(vec![Rectangle::from_size((WIDTH, HEIGHT).into())])
//! });
//!
//! // Optionally update the opaque regions
//! render_context.update_opaque_regions(Some(vec![Rectangle::from_size(
//!     Size::from((WIDTH, HEIGHT)),
//! )]));
//!
//! // We explicitly drop the context here to make the borrow checker happy
//! std::mem::drop(render_context);
//!
//! // Initialize a static damage tracker
//! let mut damage_tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
//! # let mut renderer = DummyRenderer::default();
//! # let mut framebuffer = DummyFramebuffer;
//!
//! let mut last_update = Instant::now();
//!
//! loop {
//!     let now = Instant::now();
//!     if now.duration_since(last_update) >= Duration::from_secs(3) {
//!         let mut render_context = buffer.render();
//!
//!         render_context.draw(|_buffer| {
//!             // Update the changed parts of the buffer
//!
//!             // Return the updated parts
//!             Result::<_, ()>::Ok(vec![Rectangle::from_size((WIDTH, HEIGHT).into())])
//!         });
//!
//!         last_update = now;
//!     }
//!
//!     // Create a render element from the buffer
//!     let location = Point::from((100.0, 100.0));
//!     let render_element = MemoryRenderBufferRenderElement::from_buffer(&mut renderer, location, &buffer, None, None, None, Kind::Unspecified)
//!         .expect("Failed to upload from memory to gpu");
//!
//!     // Render the element(s)
//!     damage_tracker
//!         .render_output(&mut renderer, &mut framebuffer, 0, &[&render_element], [0.8, 0.8, 0.9, 1.0])
//!         .expect("failed to render output");
//! }
//! ```

use std::{
    any::Any,
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex, MutexGuard},
};

use tracing::{instrument, trace, warn};

use crate::{
    backend::{
        allocator::{format::get_bpp, Fourcc},
        renderer::{
            utils::{CommitCounter, DamageBag, DamageSet, DamageSnapshot, OpaqueRegions},
            ErasedContextId, Frame, ImportMem, Renderer,
        },
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use super::{Element, Id, Kind, RenderElement, UnderlyingStorage};

/// A buffer storing pixel data backed by system memory
#[derive(Debug, Clone)]
pub struct MemoryBuffer {
    mem: Arc<Vec<u8>>,
    format: Fourcc,
    size: Size<i32, Buffer>,
    stride: i32,
}

impl Default for MemoryBuffer {
    fn default() -> Self {
        Self {
            mem: Default::default(),
            format: Fourcc::Abgr8888,
            size: Default::default(),
            stride: Default::default(),
        }
    }
}

impl MemoryBuffer {
    /// Create a new zeroed memory buffer with the specified format and size
    pub fn new(format: Fourcc, size: impl Into<Size<i32, Buffer>>) -> Self {
        let size = size.into();
        let stride = size.w * (get_bpp(format).expect("Format with unknown bits per pixel") / 8) as i32;
        let mem = vec![0; (stride * size.h) as usize];
        Self {
            mem: Arc::new(mem),
            format,
            size,
            stride,
        }
    }

    /// Create a new memory buffer from a slice with the specified format and size
    pub fn from_slice(mem: &[u8], format: Fourcc, size: impl Into<Size<i32, Buffer>>) -> Self {
        let size = size.into();
        let stride = size.w * (get_bpp(format).expect("Format with unknown bits per pixel") / 8) as i32;
        assert!(mem.len() >= (stride * size.h) as usize);
        Self {
            mem: Arc::new(mem.to_vec()),
            format,
            size,
            stride,
        }
    }

    /// Get the size of this buffer
    pub fn size(&self) -> Size<i32, Buffer> {
        self.size
    }

    /// Get the format of this buffer
    pub fn format(&self) -> Fourcc {
        self.format
    }

    /// Get the stride of this buffer
    pub fn stride(&self) -> i32 {
        self.stride
    }

    /// Resize this buffer to the size specified
    pub fn resize(&mut self, size: impl Into<Size<i32, Buffer>>) -> bool {
        self.size = size.into();
        self.stride =
            self.size.w * (get_bpp(self.format).expect("Format with unknown bits per pixel") / 8) as i32;
        let mem_size = (self.stride * self.size.h) as usize;
        if self.mem.len() != mem_size {
            let mem = Arc::make_mut(&mut self.mem);
            mem.resize(mem_size, 0);
            true
        } else {
            false
        }
    }
}

impl std::ops::Deref for MemoryBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.mem
    }
}

impl std::ops::DerefMut for MemoryBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *Arc::make_mut(&mut self.mem)
    }
}

#[derive(Debug)]
struct MemoryRenderBufferInner {
    mem: MemoryBuffer,
    scale: i32,
    transform: Transform,
    opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    damage_bag: DamageBag<i32, Buffer>,
    textures: HashMap<ErasedContextId, Box<dyn Any + Send>>,
    renderer_seen: HashMap<ErasedContextId, CommitCounter>,
}

impl Default for MemoryRenderBufferInner {
    fn default() -> Self {
        MemoryRenderBufferInner {
            mem: Default::default(),
            scale: 1,
            transform: Transform::Normal,
            opaque_regions: None,
            damage_bag: DamageBag::default(),
            textures: HashMap::default(),
            renderer_seen: HashMap::default(),
        }
    }
}

impl MemoryRenderBufferInner {
    fn new(
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        MemoryRenderBufferInner {
            mem: MemoryBuffer::new(format, size),
            scale,
            transform,
            opaque_regions,
            damage_bag: DamageBag::default(),
            textures: HashMap::default(),
            renderer_seen: HashMap::default(),
        }
    }

    fn from_memory(
        mem: MemoryBuffer,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        MemoryRenderBufferInner {
            mem,
            scale,
            transform,
            opaque_regions,
            damage_bag: DamageBag::default(),
            textures: HashMap::default(),
            renderer_seen: HashMap::default(),
        }
    }

    fn from_slice(
        mem: &[u8],
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        Self::from_memory(
            MemoryBuffer::from_slice(mem, format, size),
            scale,
            transform,
            opaque_regions,
        )
    }

    fn resize(&mut self, size: impl Into<Size<i32, Buffer>>) {
        if self.mem.resize(size) {
            self.renderer_seen.clear();
            self.textures.clear();
            self.damage_bag.reset();
            self.opaque_regions = None;
        }
    }

    #[instrument(level = "trace", skip(renderer))]
    #[profiling::function]
    fn import_texture<R>(&mut self, renderer: &mut R) -> Result<R::TextureId, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let context_id = renderer.context_id().erased();
        let current_commit = self.damage_bag.current_commit();
        let last_commit = self.renderer_seen.get(&context_id).copied();
        let buffer_damage = self
            .damage_bag
            .damage_since(last_commit)
            .map(|d| d.into_iter().reduce(|a, b| a.merge(b)).unwrap_or_default())
            .unwrap_or_else(|| Rectangle::from_size(self.mem.size()));

        let tex = match self.textures.entry(context_id.clone()) {
            Entry::Occupied(entry) => {
                let tex = entry.get().downcast_ref().unwrap();
                if !buffer_damage.is_empty() {
                    trace!("updating memory with damage {:#?}", &buffer_damage);
                    renderer.update_memory(tex, &self.mem, buffer_damage)?
                }
                tex.clone()
            }
            Entry::Vacant(entry) => {
                trace!("importing memory");
                let tex = renderer.import_memory(&self.mem, self.mem.format(), self.mem.size(), false)?;
                entry.insert(Box::new(tex.clone()));
                tex
            }
        };

        self.renderer_seen.insert(context_id, current_commit);
        Ok(tex)
    }
}

/// A memory backed render buffer
#[derive(Debug, Clone)]
pub struct MemoryRenderBuffer {
    id: Id,
    inner: Arc<Mutex<MemoryRenderBufferInner>>,
}

impl Default for MemoryRenderBuffer {
    fn default() -> Self {
        Self {
            id: Id::new(),
            inner: Default::default(),
        }
    }
}

impl MemoryRenderBuffer {
    /// Initialize a empty [`MemoryRenderBuffer`]
    pub fn new(
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        let inner = MemoryRenderBufferInner::new(format, size, scale, transform, opaque_regions);
        MemoryRenderBuffer {
            id: Id::new(),
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Initialize a [`MemoryRenderBuffer`] from an existing [`MemoryBuffer`]
    pub fn from_memory(
        mem: MemoryBuffer,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        let inner = MemoryRenderBufferInner::from_memory(mem, scale, transform, opaque_regions);
        MemoryRenderBuffer {
            id: Id::new(),
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Initialize a [`MemoryRenderBuffer`] from a slice
    pub fn from_slice(
        mem: &[u8],
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        let inner = MemoryRenderBufferInner::from_slice(mem, format, size, scale, transform, opaque_regions);
        MemoryRenderBuffer {
            id: Id::new(),
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Render to the memory buffer
    pub fn render(&mut self) -> RenderContext<'_> {
        let guard = self.inner.lock().unwrap();
        RenderContext {
            buffer: guard,
            damage: Vec::new(),
            opaque_regions: None,
        }
    }
}

/// A render context for [`MemoryRenderBuffer`]
#[derive(Debug)]
pub struct RenderContext<'a> {
    buffer: MutexGuard<'a, MemoryRenderBufferInner>,
    damage: Vec<Rectangle<i32, Buffer>>,
    opaque_regions: Option<Option<Vec<Rectangle<i32, Buffer>>>>,
}

impl RenderContext<'_> {
    /// Resize the buffer
    ///
    /// Note that this will also reset the opaque regions.
    /// If you previously set opaque regions you should update
    /// them with [`RenderContext::update_opaque_regions`]
    pub fn resize(&mut self, size: impl Into<Size<i32, Buffer>>) {
        self.buffer.resize(size);
    }

    /// Draw to the buffer
    ///
    /// Provided closure has to return updated regions.
    pub fn draw<F, E>(&mut self, f: F) -> Result<(), E>
    where
        F: FnOnce(&mut [u8]) -> Result<Vec<Rectangle<i32, Buffer>>, E>,
    {
        let draw_damage = f(&mut self.buffer.mem)?;
        self.damage.extend(draw_damage);
        Ok(())
    }

    /// Update the opaque regions
    pub fn update_opaque_regions(&mut self, opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>) {
        self.opaque_regions = Some(opaque_regions);
    }
}

impl Drop for RenderContext<'_> {
    fn drop(&mut self) {
        self.buffer.damage_bag.add(std::mem::take(&mut self.damage));
        if let Some(opaque_regions) = self.opaque_regions.take() {
            self.buffer.opaque_regions = opaque_regions;
        }
    }
}

/// A render element for [`MemoryRenderBuffer`]
#[derive(Debug)]
pub struct MemoryRenderBufferRenderElement<R: Renderer> {
    id: Id,
    location: Point<f64, Physical>,
    buffer: MemoryBuffer,
    alpha: f32,
    src: Rectangle<f64, Logical>,
    buffer_scale: i32,
    buffer_transform: Transform,
    size: Size<i32, Logical>,
    damage: DamageSnapshot<i32, Buffer>,
    opaque_regions: OpaqueRegions<i32, Buffer>,
    texture: R::TextureId,
    kind: Kind,
}

impl<R: Renderer> MemoryRenderBufferRenderElement<R> {
    /// Create a new [`MemoryRenderBufferRenderElement`] for
    /// a [`MemoryRenderBuffer`]
    pub fn from_buffer(
        renderer: &mut R,
        location: impl Into<Point<f64, Physical>>,
        buffer: &MemoryRenderBuffer,
        alpha: Option<f32>,
        src: Option<Rectangle<f64, Logical>>,
        size: Option<Size<i32, Logical>>,
        kind: Kind,
    ) -> Result<Self, R::Error>
    where
        R: ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut inner = buffer.inner.lock().unwrap();
        let texture = inner.import_texture(renderer)?;

        let size = size
            .or_else(|| src.map(|src| Size::from((src.size.w as i32, src.size.h as i32))))
            .unwrap_or_else(|| inner.mem.size().to_logical(inner.scale, inner.transform));

        let src = src.unwrap_or_else(|| Rectangle::from_size(size.to_f64()));

        Ok(MemoryRenderBufferRenderElement {
            id: buffer.id.clone(),
            buffer: inner.mem.clone(),
            location: location.into(),
            alpha: alpha.unwrap_or(1.0),
            src,
            buffer_scale: inner.scale,
            buffer_transform: inner.transform,
            size,
            opaque_regions: inner
                .opaque_regions
                .as_deref()
                .map(OpaqueRegions::from_slice)
                .unwrap_or_default(),
            damage: inner.damage_bag.snapshot(),
            texture,
            kind,
        })
    }

    fn physical_size(&self, scale: Scale<f64>) -> Size<i32, Physical> {
        ((self.size.to_f64().to_physical(scale).to_point() + self.location).to_i32_round()
            - self.location.to_i32_round())
        .to_size()
    }

    fn scale(&self) -> Scale<f64> {
        let src = self.src();
        Scale::from((self.size.w as f64 / src.size.w, self.size.h as f64 / src.size.h))
    }
}

impl<R: Renderer> Element for MemoryRenderBufferRenderElement<R> {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    fn transform(&self) -> Transform {
        self.buffer_transform
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.src.to_buffer(
            self.buffer_scale as f64,
            self.buffer_transform,
            &self.size.to_f64(),
        )
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::new(self.location.to_i32_round(), self.physical_size(scale))
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        let physical_size = self.physical_size(scale);
        let logical_scale = self.scale();

        self.damage
            .damage_since(commit)
            .map(|damage| {
                damage
                    .into_iter()
                    .filter_map(|rect| {
                        rect.to_f64()
                            .to_logical(
                                self.buffer_scale as f64,
                                self.buffer_transform,
                                &self.buffer.size.to_f64(),
                            )
                            .intersection(self.src)
                            .map(|mut rect| {
                                rect.loc -= self.src.loc;
                                rect.upscale(logical_scale)
                            })
                            .map(|rect| {
                                let surface_scale =
                                    physical_size.to_f64() / self.size.to_f64().to_physical(scale);
                                rect.to_physical_precise_up(surface_scale * scale)
                            })
                    })
                    .collect::<DamageSet<_, _>>()
            })
            .unwrap_or_else(|| DamageSet::from_slice(&[Rectangle::from_size(physical_size)]))
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.alpha < 1.0 {
            return OpaqueRegions::default();
        }

        let physical_size = self.physical_size(scale);
        let logical_scale = self.scale();

        self.opaque_regions
            .iter()
            .filter_map(|rect| {
                rect.to_f64()
                    .to_logical(
                        self.buffer_scale as f64,
                        self.buffer_transform,
                        &self.buffer.size.to_f64(),
                    )
                    .intersection(self.src)
                    .map(|mut rect| {
                        rect.loc -= self.src.loc;
                        rect.upscale(logical_scale)
                    })
                    .map(|rect| {
                        let surface_scale = physical_size.to_f64() / self.size.to_f64().to_physical(scale);
                        rect.to_physical_precise_up(surface_scale * scale)
                    })
            })
            .collect::<OpaqueRegions<_, _>>()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl<R> RenderElement<R> for MemoryRenderBufferRenderElement<R>
where
    R: Renderer + ImportMem,
    R::TextureId: 'static,
{
    #[instrument(level = "trace", skip(self, frame))]
    #[profiling::function]
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            self.buffer_transform,
            self.alpha,
        )
    }

    #[inline]
    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        Some(UnderlyingStorage::Memory(&self.buffer))
    }
}
