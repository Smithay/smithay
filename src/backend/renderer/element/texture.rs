//! Element to render a texture
//!
//! # Why use this implementation
//!
//! This module provides the following elements
//!
//! ## Static texture
//!
//! [`TextureBuffer`] represents a static texture which is expected to not change.
//! It is possible to either use a [`pre-existing texture`](TextureBuffer::from_texture) or to create the texture
//! from [`RGBA memory`](TextureBuffer::from_memory).
//! The [`TextureBuffer`] can be used in the smithay pipeline by using [`TextureRenderElement::from_texture_buffer`].
//!  
//! ## Hardware accelerated rendering
//!
//! [`TextureRenderBuffer`] provides a solution for hardware accelerated rending with
//! proper damage tracking. It provides a [`RenderContext`] for rendering to the stored
//! texture. The collected damage from [`RenderContext::draw`] is used to report the
//! damaged regions in the [`TextureRenderElement`]. For rendering a [`TextureRenderBuffer`] you
//! can create a render element with [`TextureRenderElement::from_texture_render_buffer`].
//!
//! # Why **not** to use this implementation
//!
//! For software rendering you may take a look at [`MemoryRenderBuffer`](super::memory::MemoryRenderBuffer)
//! which provides proper damage tracking and is agnostic over the [`Renderer`]
//!
//! # How to use it
//!
//! Both, [`TextureBuffer`] and [`TextureRenderBuffer`] represent a buffer of your texture data.
//! To render the buffer you have to create a [`TextureRenderElement`] in your render loop as
//! shown in the examples.
//!
//! To integrate custom texture based buffers the [`TextureRenderElement`] provides two functions
//! to create the render element. [`from_static_texture`](TextureRenderElement::from_static_texture) can be used to
//! create a render element from a static texture without damage tracking.
//! [`from_texture_with_damage`](TextureRenderElement::from_texture_with_damage) can be used to create
//! a render element from an existing texture with custom damage tracking.
//! In both cases you have to make sure to provide a stable [`Id`] or otherwise damage tracking will not work.
//!
//! ## [`TextureBuffer`]
//!
//! ```no_run
//! # use smithay::{
//! #     backend::renderer::{DebugFlags, Frame, ImportMem, Renderer, Texture, TextureFilter, sync::SyncPoint},
//! #     utils::{Buffer, Physical, Rectangle, Size},
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
//! use smithay::{
//!     backend::{
//!         allocator::Fourcc,
//!         renderer::{
//!             damage::OutputDamageTracker,
//!             element::{
//!                 Kind,
//!                 texture::{TextureBuffer, TextureRenderElement},
//!             },
//!         },
//!     },
//!     utils::{Point, Transform},
//! };
//!
//! const WIDTH: i32 = 10;
//! const HEIGHT: i32 = 10;
//!
//! let memory = vec![0; (WIDTH * 4 * HEIGHT) as usize];
//! # let mut renderer = FakeRenderer;
//!
//! // Create the texture buffer from a chunk of memory
//! let texture_buffer = TextureBuffer::from_memory(
//!     &mut renderer,
//!     &memory,
//!     Fourcc::Argb8888,
//!     (WIDTH, HEIGHT),
//!     false,
//!     1,
//!     Transform::Normal,
//!     None,
//! )
//! .expect("failed to import mem");
//!
//! let mut damage_tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
//!
//! loop {
//!     // Create a render element from the buffer
//!     let location = Point::from((100.0, 100.0));
//!     let render_element =
//!         TextureRenderElement::from_texture_buffer(location, &texture_buffer, None, None, None, Kind::Unspecified);
//!
//!     // Render the element(s)
//!     damage_tracker
//!         .render_output(&mut renderer, 0, &[&render_element], [0.8, 0.8, 0.9, 1.0])
//!         .expect("failed to render output");
//! }
//! ```
//!
//! ## [`TextureRenderBuffer`]
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
//!                 texture::{TextureRenderBuffer, TextureRenderElement},
//!             },
//!         },
//!     },
//!     utils::{Point, Rectangle, Size, Transform},
//! };
//!
//! const WIDTH: i32 = 10;
//! const HEIGHT: i32 = 10;
//!
//! let memory = vec![0; (WIDTH * 4 * HEIGHT) as usize];
//! # let mut renderer = FakeRenderer;
//!
//! // Create the texture buffer from a chunk of memory
//! let mut texture_render_buffer = TextureRenderBuffer::from_memory(
//!     &mut renderer,
//!     &memory,
//!     Fourcc::Argb8888,
//!     (WIDTH, HEIGHT),
//!     false,
//!     1,
//!     Transform::Normal,
//!     None,
//! )
//! .expect("failed to import mem");
//!
//! // Create the rendering context
//! let mut render_context = texture_render_buffer.render();
//!
//! // Draw to the texture
//! render_context.draw(|_texture| {
//!     // Your draw code here...
//!
//!     // Return the damage areas
//!     Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
//!         Point::default(),
//!         Size::from((WIDTH, HEIGHT)),
//!     )])
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
//! let mut damage_tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
//!
//! let mut last_update = Instant::now();
//!
//! loop {
//!     let now = Instant::now();
//!     if now.duration_since(last_update) >= Duration::from_secs(3) {
//!         let mut render_context = texture_render_buffer.render();
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
//!     let render_element = TextureRenderElement::from_texture_render_buffer(
//!         location,
//!         &texture_render_buffer,
//!         None,
//!         None,
//!         None,
//!         Kind::Unspecified,
//!     );
//!
//!     // Render the element(s)
//!     damage_tracker
//!         .render_output(&mut renderer, 0, &[&render_element], [0.8, 0.8, 0.9, 1.0])
//!         .expect("failed to render output");
//! }
//! ```

use std::sync::{Arc, Mutex};

use tracing::{instrument, warn};

use crate::{
    backend::{
        allocator::Fourcc,
        renderer::{
            utils::{DamageBag, DamageSnapshot},
            Frame, ImportMem, Renderer, Texture,
        },
    },
    utils::{Buffer, Coordinate, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use super::{CommitCounter, Element, Id, Kind, RenderElement};

/// A single texture buffer
#[derive(Debug, Clone)]
pub struct TextureBuffer<T> {
    id: Id,
    renderer_id: usize,
    texture: T,
    scale: i32,
    transform: Transform,
    opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
}

impl<T> TextureBuffer<T> {
    /// Create a [`TextureBuffer`] from an existing texture
    pub fn from_texture<R: Renderer>(
        renderer: &R,
        texture: T,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        TextureBuffer {
            id: Id::new(),
            renderer_id: renderer.id(),
            texture,
            scale,
            transform,
            opaque_regions,
        }
    }

    /// Create [`TextureBuffer`] from a chunk of memory
    #[allow(clippy::too_many_arguments)]
    pub fn from_memory<R: Renderer<TextureId = T> + ImportMem>(
        renderer: &mut R,
        data: &[u8],
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        flipped: bool,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Result<Self, <R as Renderer>::Error> {
        let texture = renderer.import_memory(data, format, size.into(), flipped)?;
        Ok(TextureBuffer::from_texture(
            renderer,
            texture,
            scale,
            transform,
            opaque_regions,
        ))
    }

    /// Format of the underlying texture
    pub fn format(&self) -> Option<Fourcc>
    where
        T: Texture,
    {
        self.texture.format()
    }
}

/// A texture backed render buffer
#[derive(Debug, Clone)]
pub struct TextureRenderBuffer<T> {
    id: Id,
    renderer_id: usize,
    texture: T,
    scale: i32,
    transform: Transform,
    opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    damage_tracker: Arc<Mutex<DamageBag<i32, Buffer>>>,
}

impl<T: Texture> TextureRenderBuffer<T> {
    /// Create [`TextureRenderBuffer`] from an existing texture
    pub fn from_texture<R: Renderer>(
        renderer: &R,
        texture: T,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Self {
        TextureRenderBuffer {
            id: Id::new(),
            renderer_id: renderer.id(),
            texture,
            scale,
            transform,
            opaque_regions,
            damage_tracker: Arc::new(Mutex::new(DamageBag::default())),
        }
    }

    /// Create [`TextureRenderBuffer`] from a chunk of memory
    #[allow(clippy::too_many_arguments)]
    pub fn from_memory<R: Renderer<TextureId = T> + ImportMem>(
        renderer: &mut R,
        data: &[u8],
        format: Fourcc,
        size: impl Into<Size<i32, Buffer>>,
        flipped: bool,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Result<Self, <R as Renderer>::Error> {
        let texture = renderer.import_memory(data, format, size.into(), flipped)?;
        Ok(TextureRenderBuffer::from_texture(
            renderer,
            texture,
            scale,
            transform,
            opaque_regions,
        ))
    }

    /// Replace the stored texture
    pub fn update_from_texture<R: Renderer>(
        &mut self,
        renderer: &R,
        texture: T,
        scale: i32,
        transform: Transform,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) {
        assert_eq!(self.renderer_id, renderer.id());
        self.texture = texture;
        self.scale = scale;
        self.transform = transform;
        self.opaque_regions = opaque_regions;
        self.damage_tracker.lock().unwrap().reset();
    }

    /// Update the texture from a chunk of memory
    pub fn update_from_memory<R: Renderer<TextureId = T> + ImportMem>(
        &mut self,
        renderer: &mut R,
        data: &[u8],
        region: Rectangle<i32, Buffer>,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
    ) -> Result<(), <R as Renderer>::Error> {
        assert_eq!(self.renderer_id, renderer.id());
        renderer.update_memory(&self.texture, data, region)?;
        self.damage_tracker.lock().unwrap().add([region]);
        self.opaque_regions = opaque_regions;
        Ok(())
    }

    /// Render to the texture
    pub fn render(&mut self) -> RenderContext<'_, T> {
        RenderContext {
            buffer: self,
            damage: Vec::new(),
            opaque_regions: None,
        }
    }

    /// Format of the underlying texture
    pub fn format(&self) -> Option<Fourcc> {
        self.texture.format()
    }
}

/// A render context for [`TextureRenderBuffer`]
#[derive(Debug)]
pub struct RenderContext<'a, T> {
    buffer: &'a mut TextureRenderBuffer<T>,
    damage: Vec<Rectangle<i32, Buffer>>,
    opaque_regions: Option<Option<Vec<Rectangle<i32, Buffer>>>>,
}

impl<'a, T> RenderContext<'a, T> {
    /// Draw to the buffer
    pub fn draw<F, E>(&mut self, f: F) -> Result<(), E>
    where
        F: FnOnce(&T) -> Result<Vec<Rectangle<i32, Buffer>>, E>,
    {
        let draw_damage = f(&self.buffer.texture)?;
        self.damage.extend(draw_damage);
        Ok(())
    }

    /// Update the opaque regions
    pub fn update_opaque_regions(&mut self, opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>) {
        self.opaque_regions = Some(opaque_regions);
    }
}

impl<'a, T> Drop for RenderContext<'a, T> {
    fn drop(&mut self) {
        self.buffer
            .damage_tracker
            .lock()
            .unwrap()
            .add(std::mem::take(&mut self.damage));
        if let Some(opaque_regions) = self.opaque_regions.take() {
            self.buffer.opaque_regions = opaque_regions;
        }
    }
}

/// A render element for a [`TextureRenderBuffer`]
#[derive(Debug)]
pub struct TextureRenderElement<T> {
    location: Point<f64, Physical>,
    id: Id,
    renderer_id: usize,
    texture: T,
    scale: i32,
    transform: Transform,
    alpha: f32,
    src: Option<Rectangle<f64, Logical>>,
    size: Option<Size<i32, Logical>>,
    opaque_regions: Option<Vec<Rectangle<i32, Logical>>>,
    snapshot: DamageSnapshot<i32, Buffer>,
    kind: Kind,
}

impl<T: Texture> TextureRenderElement<T> {
    fn damage_since(&self, commit: Option<CommitCounter>) -> Vec<Rectangle<i32, Buffer>> {
        self.snapshot.damage_since(commit).unwrap_or_else(|| {
            vec![Rectangle::from_loc_and_size(
                Point::default(),
                self.texture.size(),
            )]
        })
    }
}

impl<T: Texture + Clone> TextureRenderElement<T> {
    /// Create a [`TextureRenderElement`] from a [`TextureRenderBuffer`]
    pub fn from_texture_render_buffer(
        location: impl Into<Point<f64, Physical>>,
        buffer: &TextureRenderBuffer<T>,
        alpha: Option<f32>,
        src: Option<Rectangle<f64, Logical>>,
        size: Option<Size<i32, Logical>>,
        kind: Kind,
    ) -> Self {
        TextureRenderElement::from_texture_with_damage(
            buffer.id.clone(),
            buffer.renderer_id,
            location,
            buffer.texture.clone(),
            buffer.scale,
            buffer.transform,
            alpha,
            src,
            size,
            buffer.opaque_regions.clone(),
            buffer.damage_tracker.lock().unwrap().snapshot(),
            kind,
        )
    }

    /// Create a [`TextureRenderElement`] from a [`TextureBuffer`]
    pub fn from_texture_buffer(
        location: impl Into<Point<f64, Physical>>,
        buffer: &TextureBuffer<T>,
        alpha: Option<f32>,
        src: Option<Rectangle<f64, Logical>>,
        size: Option<Size<i32, Logical>>,
        kind: Kind,
    ) -> Self {
        TextureRenderElement::from_static_texture(
            buffer.id.clone(),
            buffer.renderer_id,
            location,
            buffer.texture.clone(),
            buffer.scale,
            buffer.transform,
            alpha,
            src,
            size,
            buffer.opaque_regions.clone(),
            kind,
        )
    }
}

impl<T: Texture> TextureRenderElement<T> {
    /// Create a [`TextureRenderElement`] from an
    /// existing texture and a [`DamageSnapshot`]
    #[allow(clippy::too_many_arguments)]
    pub fn from_texture_with_damage(
        id: Id,
        renderer_id: usize,
        location: impl Into<Point<f64, Physical>>,
        texture: T,
        scale: i32,
        transform: Transform,
        alpha: Option<f32>,
        src: Option<Rectangle<f64, Logical>>,
        size: Option<Size<i32, Logical>>,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
        snapshot: DamageSnapshot<i32, Buffer>,
        kind: Kind,
    ) -> Self {
        let opaque_regions = opaque_regions.map(|regions| {
            regions
                .into_iter()
                .map(|region| region.to_logical(scale, transform, &texture.size()))
                .collect::<Vec<_>>()
        });
        TextureRenderElement {
            location: location.into(),
            id,
            renderer_id,
            texture,
            scale,
            transform,
            alpha: alpha.unwrap_or(1.0),
            src,
            size,
            opaque_regions,
            snapshot,
            kind,
        }
    }

    /// Create a static [`TextureRenderElement`] from
    /// an existing texture
    #[allow(clippy::too_many_arguments)]
    pub fn from_static_texture(
        id: Id,
        renderer_id: usize,
        location: impl Into<Point<f64, Physical>>,
        texture: T,
        scale: i32,
        transform: Transform,
        alpha: Option<f32>,
        src: Option<Rectangle<f64, Logical>>,
        size: Option<Size<i32, Logical>>,
        opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
        kind: Kind,
    ) -> Self {
        TextureRenderElement::from_texture_with_damage(
            id,
            renderer_id,
            location,
            texture,
            scale,
            transform,
            alpha,
            src,
            size,
            opaque_regions,
            DamageSnapshot::empty(),
            kind,
        )
    }

    fn logical_size(&self) -> Size<i32, Logical> {
        self.size
            .or_else(|| {
                self.src
                    .map(|src| Size::from((src.size.w as i32, src.size.h as i32)))
            })
            .unwrap_or_else(|| self.texture.size().to_logical(self.scale, self.transform))
    }

    fn physical_size(&self, scale: Scale<f64>) -> Size<i32, Physical> {
        let logical_size = self.logical_size();
        ((logical_size.to_f64().to_physical(scale).to_point() + self.location).to_i32_round()
            - self.location.to_i32_round())
        .to_size()
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

    fn rect_to_global<N>(&self, rect: Rectangle<N, Logical>) -> Rectangle<f64, Logical>
    where
        N: Coordinate,
    {
        let scale = self.scale();
        let mut rect = rect.to_f64();
        rect.loc -= self.src.map(|rect| rect.loc).unwrap_or_default();
        rect.upscale(scale)
    }
}

impl<T> Element for TextureRenderElement<T>
where
    T: Texture,
{
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.snapshot.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(self.location.to_i32_round(), self.physical_size(scale))
    }

    fn transform(&self) -> Transform {
        self.transform
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        let size = self.logical_size();
        self.src()
            .to_buffer(self.scale as f64, self.transform, &size.to_f64())
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let src = self.src();
        let texture_size = self.texture.size();
        let physical_size = self.physical_size(scale);
        let logical_size = self.logical_size();
        self.damage_since(commit)
            .into_iter()
            .filter_map(|rect| {
                rect.to_f64()
                    .to_logical(self.scale as f64, self.transform, &texture_size.to_f64())
                    .intersection(src)
                    .map(|rect| self.rect_to_global(rect).to_i32_up::<i32>())
                    .map(|rect| {
                        let surface_scale = physical_size.to_f64() / logical_size.to_f64().to_physical(scale);
                        rect.to_physical_precise_up(surface_scale * scale)
                    })
            })
            .collect::<Vec<_>>()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        if self.alpha < 1.0 {
            return Vec::new();
        }

        let src = self.src();
        let physical_size = self.physical_size(scale);
        let logical_size = self.logical_size();
        self.opaque_regions
            .as_ref()
            .map(|r| {
                r.iter()
                    .filter_map(|rect| {
                        rect.to_f64()
                            .intersection(src)
                            .map(|rect| self.rect_to_global(rect).to_i32_up::<i32>())
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

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl<R, T> RenderElement<R> for TextureRenderElement<T>
where
    R: Renderer<TextureId = T>,
    T: Texture,
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
        if frame.id() != self.renderer_id {
            warn!("trying to render texture from different renderer");
            return Ok(());
        }

        frame.render_texture_from_to(&self.texture, src, dst, damage, self.transform, self.alpha)
    }
}
