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
//! #     backend::renderer::{DebugFlags, Frame, ImportMem, Renderer, Texture, TextureFilter},
//! #     utils::{Buffer, Physical},
//! # };
//! #
//! # #[derive(Clone)]
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
//! #     fn finish(self) -> Result<(), Self::Error> { unimplemented!() }
//! # }
//! #
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
//!             element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
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
//!     let render_element = MemoryRenderBufferRenderElement::from_buffer(&mut renderer, location, &buffer, None, None, None)
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
    collections::{hash_map::Entry, HashMap},
    fmt,
    marker::PhantomData,
    sync::{Arc, Mutex, MutexGuard},
};

use tracing::{instrument, trace, warn};

use crate::{
    backend::{
        allocator::{format::get_bpp, Fourcc},
        color::CMS,
        renderer::{
            utils::{CommitCounter, DamageBag},
            Frame, ImportMem, Renderer,
        },
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use super::{Element, Id, RenderElement};

struct MemoryRenderBufferInner<C: CMS> {
    mem: Vec<u8>,
    format: Fourcc,
    size: Size<i32, Buffer>,
    scale: i32,
    transform: Transform,
    input_profile: C::ColorProfile,
    opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    damage_bag: DamageBag<i32, Buffer>,
    textures: HashMap<(TypeId, usize), Box<dyn std::any::Any>>,
    renderer_seen: HashMap<(TypeId, usize), CommitCounter>,
}

impl<C: CMS> fmt::Debug for MemoryRenderBufferInner<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryRenderBufferInner")
            .field("mem", &"<...>")
            .field("format", &self.format)
            .field("size", &self.size)
            .field("scale", &self.scale)
            .field("transform", &self.transform)
            .field("opaque_regions", &self.opaque_regions)
            .field("damage_bag", &self.damage_bag)
            .finish_non_exhaustive()
    }
}

impl<C: CMS> MemoryRenderBufferInner<C> {
    fn new(
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
        input_profile: C::ColorProfile,
    ) -> Self {
        let size = size.into();
        MemoryRenderBufferInner {
            mem: vec![
                0;
                (size.w * (get_bpp(format).expect("Format with unknown bits per pixel") / 8) as i32 * size.h)
                    as usize
            ],
            format,
            size,
            scale,
            transform,
            opaque_regions,
            input_profile,
            damage_bag: DamageBag::default(),
            textures: HashMap::default(),
            renderer_seen: HashMap::default(),
        }
    }

    fn from_memory(
        mem: &[u8],
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
        input_profile: C::ColorProfile,
    ) -> Self {
        let size = size.into();
        assert_eq!(
            mem.len(),
            (size.w * (get_bpp(format).expect("Format with unknown bits per pixel") / 8) as i32 * size.h)
                as usize
        );

        MemoryRenderBufferInner {
            mem: mem.to_vec(),
            format,
            size,
            scale,
            transform,
            opaque_regions,
            input_profile,
            damage_bag: DamageBag::default(),
            textures: HashMap::default(),
            renderer_seen: HashMap::default(),
        }
    }

    fn resize(&mut self, size: impl Into<Size<i32, Buffer>>) {
        let size = size.into();
        let mem_size = (size.w * 4 * size.h) as usize;
        if self.mem.len() != mem_size {
            self.mem.resize(mem_size, 0);
            self.renderer_seen.clear();
            self.textures.clear();
            self.damage_bag.reset();
            self.size = size;
            self.opaque_regions = None;
        }
    }

    #[instrument(level = "trace", skip(renderer))]
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
            .unwrap_or_else(|| Rectangle::from_loc_and_size(Point::default(), self.size));

        match self.textures.entry(texture_id) {
            Entry::Occupied(entry) => {
                if !buffer_damage.is_empty() {
                    trace!("updating memory with damage {:#?}", &buffer_damage);
                    renderer.update_memory(entry.get().downcast_ref().unwrap(), &self.mem, buffer_damage)?
                }
            }
            Entry::Vacant(entry) => {
                trace!("importing memory");
                let tex = renderer.import_memory(&self.mem, self.format, self.size, false)?;
                entry.insert(Box::new(tex));
            }
        };

        self.renderer_seen.insert(texture_id, current_commit);
        Ok(())
    }

    fn get_texture<R>(&self, renderer_id: usize) -> Option<&<R as Renderer>::TextureId>
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
#[derive(Debug)]
pub struct MemoryRenderBuffer<C: CMS> {
    id: Id,
    inner: Arc<Mutex<MemoryRenderBufferInner<C>>>,
}

impl<C: CMS> Clone for MemoryRenderBuffer<C> {
    fn clone(&self) -> Self {
        MemoryRenderBuffer {
            id: self.id.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<C: CMS> MemoryRenderBuffer<C> {
    /// Initialize a empty [`MemoryRenderBuffer`]
    pub fn new(
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
        input_profile: C::ColorProfile,
    ) -> Self {
        let inner =
            MemoryRenderBufferInner::new(format, size, scale, transform, opaque_regions, input_profile);
        MemoryRenderBuffer {
            id: Id::new(),
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Initialize a [`MemoryRenderBuffer`] from existing memory
    pub fn from_memory(
        mem: &[u8],
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
        input_profile: C::ColorProfile,
    ) -> Self {
        let inner = MemoryRenderBufferInner::from_memory(
            mem,
            format,
            size,
            scale,
            transform,
            opaque_regions,
            input_profile,
        );
        MemoryRenderBuffer {
            id: Id::new(),
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Render to the memory buffer
    pub fn render(&mut self) -> RenderContext<'_, C> {
        let guard = self.inner.lock().unwrap();
        RenderContext {
            buffer: guard,
            damage: Vec::new(),
            opaque_regions: None,
        }
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.lock().unwrap().damage_bag.current_commit()
    }

    fn size(&self) -> Size<i32, Logical> {
        let guard = self.inner.lock().unwrap();
        guard.size.to_logical(guard.scale, guard.transform)
    }
}

/// A render context for [`MemoryRenderBuffer`]
#[derive(Debug)]
pub struct RenderContext<'a, C: CMS> {
    buffer: MutexGuard<'a, MemoryRenderBufferInner<C>>,
    damage: Vec<Rectangle<i32, Buffer>>,
    opaque_regions: Option<Option<Vec<Rectangle<i32, Buffer>>>>,
}

impl<'a, C: CMS> RenderContext<'a, C> {
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

impl<'a, C: CMS> Drop for RenderContext<'a, C> {
    fn drop(&mut self) {
        self.buffer.damage_bag.add(std::mem::take(&mut self.damage));
        if let Some(opaque_regions) = self.opaque_regions.take() {
            self.buffer.opaque_regions = opaque_regions;
        }
    }
}

/// A render element for [`MemoryRenderBuffer`]
#[derive(Debug)]
pub struct MemoryRenderBufferRenderElement<R: Renderer, C: CMS> {
    location: Point<f64, Physical>,
    buffer: MemoryRenderBuffer<C>,
    alpha: f32,
    src: Option<Rectangle<f64, Logical>>,
    size: Option<Size<i32, Logical>>,
    renderer_type: PhantomData<R>,
}

impl<R: Renderer, C: CMS> MemoryRenderBufferRenderElement<R, C> {
    /// Create a new [`MemoryRenderBufferRenderElement`] for
    /// a [`MemoryRenderBuffer`]
    pub fn from_buffer(
        renderer: &mut R,
        location: impl Into<Point<f64, Physical>>,
        buffer: &MemoryRenderBuffer<C>,
        alpha: Option<f32>,
        src: Option<Rectangle<f64, Logical>>,
        size: Option<Size<i32, Logical>>,
    ) -> Result<Self, <R as Renderer>::Error>
    where
        R: ImportMem,
        <R as Renderer>::TextureId: 'static,
    {
        buffer.inner.lock().unwrap().import_texture(renderer)?;
        Ok(MemoryRenderBufferRenderElement {
            location: location.into(),
            buffer: buffer.clone(),
            alpha: alpha.unwrap_or(1.0),
            src,
            size,
            renderer_type: PhantomData,
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
        let guard = self.buffer.inner.lock().unwrap();

        guard
            .damage_bag
            .damage_since(commit)
            .map(|damage| {
                damage
                    .into_iter()
                    .filter_map(|rect| {
                        rect.to_f64()
                            .to_logical(guard.scale as f64, guard.transform, &guard.size.to_f64())
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
        let guard = self.buffer.inner.lock().unwrap();

        guard
            .opaque_regions
            .as_ref()
            .map(|regions| {
                regions
                    .iter()
                    .filter_map(|rect| {
                        rect.to_f64()
                            .to_logical(guard.scale as f64, guard.transform, &guard.size.to_f64())
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

impl<R: Renderer, C: CMS> Element for MemoryRenderBufferRenderElement<R, C> {
    fn id(&self) -> &super::Id {
        &self.buffer.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.buffer.current_commit()
    }

    fn transform(&self) -> Transform {
        self.buffer.inner.lock().unwrap().transform
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        let logical_size = self.logical_size();
        let guard = self.buffer.inner.lock().unwrap();
        self.src
            .map(|src| src.to_buffer(guard.scale as f64, guard.transform, &logical_size.to_f64()))
            .unwrap_or_else(|| Rectangle::from_loc_and_size(Point::default(), guard.size).to_f64())
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
}

impl<R, C: CMS> RenderElement<R, C> for MemoryRenderBufferRenderElement<R, C>
where
    R: Renderer + ImportMem,
    <R as Renderer>::TextureId: 'static,
{
    #[instrument(level = "trace", skip(self, frame))]
    fn draw<'a, 'b>(
        &self,
        frame: &mut <R as Renderer>::Frame<'a, 'b, C>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        let guard = self.buffer.inner.lock().unwrap();
        let transform = guard.transform;
        let Some(texture) = guard.get_texture::<R>(frame.id()) else {
            warn!("trying to render texture from different renderer");
            return Ok(());
        };

        frame.render_texture_from_to(
            texture,
            src,
            dst,
            damage,
            transform,
            self.alpha,
            &guard.input_profile,
        )
    }

    fn color_profile(&self) -> C::ColorProfile {
        self.buffer.inner.lock().unwrap().input_profile.clone()
    }
}
