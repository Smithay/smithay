//! Element to render from RGBA memory
//!
//! # Why use this implementation
//!
//! The [`MemoryRenderBuffer`] allows for easy integration of software-rendered elements
//! in the smithay rendering pipeline. As software-rendered elements eventually have to
//! upload to the GPU for rendering damage tracking is a crucial part. The [`MemoryRenderBuffer`]
//! allows for efficient damage tracking by providing a [`RenderContext`] which accumulates the
//! software-rendering damage. It automatically uploads the damaged parts to a [`Renderer::TextureId`]
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
//! #     backend::renderer::{DebugFlags, Frame, ImportMem, Renderer, Texture, TextureFilter, sync::SyncPoint},
//! #     utils::{Buffer, Physical},
//! # };
//! #
//! # #[derive(Clone, Debug)]
//! # struct FakeTexture;
//! #
//! # impl Texture for FakeTexture {
//! #     fn width(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn height(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn format(&self) -> Option<Fourcc> {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # struct FakeFrame;
//! #
//! # impl Frame for FakeFrame {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #
//! #     fn id(&self) -> usize { unimplemented!() }
//! #     fn clear(&mut self, _: [f32; 4], _: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn draw_solid(
//! #         &mut self,
//! #         _dst: Rectangle<i32, Physical>,
//! #         _damage: &[Rectangle<i32, Physical>],
//! #         _color: [f32; 4],
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn render_texture_from_to(
//! #         &mut self,
//! #         _: &Self::TextureId,
//! #         _: Rectangle<f64, Buffer>,
//! #         _: Rectangle<i32, Physical>,
//! #         _: &[Rectangle<i32, Physical>],
//! #         _: Transform,
//! #         _: f32,
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn transformation(&self) -> Transform {
//! #         unimplemented!()
//! #     }
//! #     fn finish(self) -> Result<SyncPoint, Self::Error> { unimplemented!() }
//! # }
//! #
//! # #[derive(Debug)]
//! # struct FakeRenderer;
//! #
//! # impl Renderer for FakeRenderer {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #     type Frame<'a> = FakeFrame;
//! #
//! #     fn id(&self) -> usize {
//! #         unimplemented!()
//! #     }
//! #     fn downscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn upscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn set_debug_flags(&mut self, _: DebugFlags) {
//! #         unimplemented!()
//! #     }
//! #     fn debug_flags(&self) -> DebugFlags {
//! #         unimplemented!()
//! #     }
//! #     fn render(&mut self, _: Size<i32, Physical>, _: Transform) -> Result<Self::Frame<'_>, Self::Error>
//! #     {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # impl ImportMem for FakeRenderer {
//! #     fn import_memory(
//! #         &mut self,
//! #         _: &[u8],
//! #         _: Fourcc,
//! #         _: Size<i32, Buffer>,
//! #         _: bool,
//! #     ) -> Result<Self::TextureId, Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn update_memory(
//! #         &mut self,
//! #         _: &Self::TextureId,
//! #         _: &[u8],
//! #         _: Rectangle<i32, Buffer>,
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn mem_formats(&self) -> Box<dyn Iterator<Item=Fourcc>> {
//! #         unimplemented!()
//! #     }
//! # }
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
//!     Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(Point::default(), (WIDTH, HEIGHT))])
//! });
//!
//! // Optionally update the opaque regions
//! render_context.update_opaque_regions(Some(vec![Rectangle::from_loc_and_size(
//!     Point::default(),
//!     Size::from((WIDTH, HEIGHT)),
//! )]));
//!
//! // We explicitly drop the context here to make the borrow checker happy
//! std::mem::drop(render_context);
//!
//! // Initialize a static damage tracker
//! let mut damage_tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
//! # let mut renderer = FakeRenderer;
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
//!             Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(Point::default(), (WIDTH, HEIGHT))])
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
//!         .render_output(&mut renderer, 0, &[&render_element], [0.8, 0.8, 0.9, 1.0])
//!         .expect("failed to render output");
//! }
//! ```

use std::{
    any::TypeId,
    cell::{RefCell, RefMut},
    collections::{hash_map::Entry, HashMap},
    marker::PhantomData,
    rc::Rc,
    sync::Arc,
};

use tracing::{instrument, trace, warn};

use crate::{
    backend::{
        allocator::{format::get_bpp, Fourcc},
        renderer::{
            utils::{CommitCounter, DamageBag},
            Frame, ImportMem, Renderer,
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
        let size = size.into();
        let stride = size.w * (get_bpp(self.format).expect("Format with unknown bits per pixel") / 8) as i32;
        let mem = Arc::make_mut(&mut self.mem);
        let mem_size = (stride * size.h) as usize;
        if mem.len() != mem_size {
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
    textures: HashMap<(TypeId, usize), Box<dyn std::any::Any>>,
    renderer_seen: HashMap<(TypeId, usize), CommitCounter>,
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
    fn import_texture<R>(&mut self, renderer: &mut R) -> Result<(), <R as Renderer>::Error>
    where
        R: Renderer + ImportMem,
        <R as Renderer>::TextureId: 'static,
    {
        let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
        let current_commit = self.damage_bag.current_commit();
        let last_commit = self.renderer_seen.get(&texture_id).copied();
        let buffer_damage = self
            .damage_bag
            .damage_since(last_commit)
            .map(|d| d.into_iter().reduce(|a, b| a.merge(b)).unwrap_or_default())
            .unwrap_or_else(|| Rectangle::from_loc_and_size(Point::default(), self.mem.size()));

        match self.textures.entry(texture_id) {
            Entry::Occupied(entry) => {
                if !buffer_damage.is_empty() {
                    trace!("updating memory with damage {:#?}", &buffer_damage);
                    renderer.update_memory(entry.get().downcast_ref().unwrap(), &self.mem, buffer_damage)?
                }
            }
            Entry::Vacant(entry) => {
                trace!("importing memory");
                let tex = renderer.import_memory(&self.mem, self.mem.format(), self.mem.size(), false)?;
                entry.insert(Box::new(tex));
            }
        };

        self.renderer_seen.insert(texture_id, current_commit);
        Ok(())
    }

    fn get_texture<R>(&mut self, renderer_id: usize) -> Option<&<R as Renderer>::TextureId>
    where
        R: Renderer,
        <R as Renderer>::TextureId: 'static,
    {
        let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer_id);
        self.textures
            .get(&texture_id)
            .and_then(|boxed| boxed.downcast_ref::<<R as Renderer>::TextureId>())
    }
}

/// A memory backed render buffer
#[derive(Debug, Clone)]
pub struct MemoryRenderBuffer {
    id: Id,
    inner: Rc<RefCell<MemoryRenderBufferInner>>,
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
            inner: Rc::new(RefCell::new(inner)),
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
            inner: Rc::new(RefCell::new(inner)),
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
            inner: Rc::new(RefCell::new(inner)),
        }
    }

    /// Render to the memory buffer
    pub fn render(&mut self) -> RenderContext<'_> {
        let guard = self.inner.borrow_mut();
        RenderContext {
            buffer: guard,
            damage: Vec::new(),
            opaque_regions: None,
        }
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.borrow_mut().damage_bag.current_commit()
    }

    fn size(&self) -> Size<i32, Logical> {
        let guard = self.inner.borrow_mut();
        guard.mem.size().to_logical(guard.scale, guard.transform)
    }
}

/// A render context for [`MemoryRenderBuffer`]
#[derive(Debug)]
pub struct RenderContext<'a> {
    buffer: RefMut<'a, MemoryRenderBufferInner>,
    damage: Vec<Rectangle<i32, Buffer>>,
    opaque_regions: Option<Option<Vec<Rectangle<i32, Buffer>>>>,
}

impl<'a> RenderContext<'a> {
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

impl<'a> Drop for RenderContext<'a> {
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
    location: Point<f64, Physical>,
    buffer: MemoryRenderBuffer,
    alpha: f32,
    src: Option<Rectangle<f64, Logical>>,
    size: Option<Size<i32, Logical>>,
    renderer_type: PhantomData<R>,
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
    ) -> Result<Self, <R as Renderer>::Error>
    where
        R: ImportMem,
        <R as Renderer>::TextureId: 'static,
    {
        buffer.inner.borrow_mut().import_texture(renderer)?;
        Ok(MemoryRenderBufferRenderElement {
            location: location.into(),
            buffer: buffer.clone(),
            alpha: alpha.unwrap_or(1.0),
            src,
            size,
            renderer_type: PhantomData,
            kind,
        })
    }

    fn logical_size(&self) -> Size<i32, Logical> {
        self.size
            .or_else(|| {
                self.src
                    .map(|src| Size::from((src.size.w as i32, src.size.h as i32)))
            })
            .unwrap_or_else(|| self.buffer.size())
    }

    fn physical_size(&self, scale: Scale<f64>) -> Size<i32, Physical> {
        let logical_size = self.logical_size();
        ((logical_size.to_f64().to_physical(scale).to_point() + self.location).to_i32_round()
            - self.location.to_i32_round())
        .to_size()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let src = self.src();
        let logical_size = self.logical_size();
        let physical_size = self.physical_size(scale);
        let logical_scale = self.scale();
        let guard = self.buffer.inner.borrow_mut();

        guard
            .damage_bag
            .damage_since(commit)
            .map(|damage| {
                damage
                    .into_iter()
                    .filter_map(|rect| {
                        rect.to_f64()
                            .to_logical(guard.scale as f64, guard.transform, &guard.mem.size().to_f64())
                            .intersection(src)
                            .map(|mut rect| {
                                rect.loc -= self.src.map(|rect| rect.loc).unwrap_or_default();
                                rect.upscale(logical_scale)
                            })
                            .map(|rect| {
                                let surface_scale =
                                    physical_size.to_f64() / logical_size.to_f64().to_physical(scale);
                                rect.to_physical_precise_up(surface_scale * scale)
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![Rectangle::from_loc_and_size(Point::default(), physical_size)])
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        if self.alpha < 1.0 {
            return Vec::new();
        }

        let src = self.src();
        let logical_size = self.logical_size();
        let physical_size = self.physical_size(scale);
        let logical_scale = self.scale();
        let guard = self.buffer.inner.borrow_mut();

        guard
            .opaque_regions
            .as_ref()
            .map(|regions| {
                regions
                    .iter()
                    .filter_map(|rect| {
                        rect.to_f64()
                            .to_logical(guard.scale as f64, guard.transform, &guard.mem.size().to_f64())
                            .intersection(src)
                            .map(|mut rect| {
                                rect.loc -= self.src.map(|rect| rect.loc).unwrap_or_default();
                                rect.upscale(logical_scale)
                            })
                            .map(|rect| {
                                let surface_scale =
                                    physical_size.to_f64() / logical_size.to_f64().to_physical(scale);
                                rect.to_physical_precise_up(surface_scale * scale)
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    fn src(&self) -> Rectangle<f64, Logical> {
        self.src
            .unwrap_or_else(|| Rectangle::from_loc_and_size(Point::default(), self.logical_size().to_f64()))
    }

    fn scale(&self) -> Scale<f64> {
        let size = self.logical_size();
        let src = self.src();
        Scale::from((size.w as f64 / src.size.w, size.h as f64 / src.size.h))
    }
}

impl<R: Renderer> Element for MemoryRenderBufferRenderElement<R> {
    fn id(&self) -> &super::Id {
        &self.buffer.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.buffer.current_commit()
    }

    fn transform(&self) -> Transform {
        self.buffer.inner.borrow_mut().transform
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        let logical_size = self.logical_size();
        let guard = self.buffer.inner.borrow_mut();
        self.src
            .map(|src| src.to_buffer(guard.scale as f64, guard.transform, &logical_size.to_f64()))
            .unwrap_or_else(|| Rectangle::from_loc_and_size(Point::default(), guard.mem.size()).to_f64())
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(self.location.to_i32_round(), self.physical_size(scale))
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        self.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.opaque_regions(scale)
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
    <R as Renderer>::TextureId: 'static,
{
    #[instrument(level = "trace", skip(self, frame))]
    #[profiling::function]
    fn draw<'a>(
        &self,
        frame: &mut <R as Renderer>::Frame<'a>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        let mut guard = self.buffer.inner.borrow_mut();
        let transform = guard.transform;
        let Some(texture) = guard.get_texture::<R>(frame.id()) else {
            warn!("trying to render texture from different renderer");
            return Ok(());
        };

        frame.render_texture_from_to(texture, src, dst, damage, transform, self.alpha)
    }

    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage> {
        let buf = self.buffer.inner.borrow();
        Some(UnderlyingStorage::Memory(buf.mem.clone()))
    }
}
