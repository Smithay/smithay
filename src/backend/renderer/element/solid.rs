//! Element to render a solid color
//!
//! # How to use it
//!
//! ```no_run
//! # use smithay::{
//! #     backend::{
//! #         allocator::Fourcc,
//! #         renderer::{DebugFlags, Frame, ImportMem, Renderer, Texture, TextureFilter, sync::SyncPoint},
//! #     },
//! #     utils::{Buffer, Physical, Rectangle, Transform},
//! # };
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
//! #     fn id(&self) -> usize {
//! #         unimplemented!()
//! #     }
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
//! #     fn finish(self) -> Result<SyncPoint, Self::Error> {
//! #         unimplemented!()
//! #     }
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
//! #     fn render(&mut self, _: Size<i32, Physical>, _: Transform) -> Result<Self::Frame<'_>, Self::Error> {
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
//! #     fn mem_formats(&self) -> Box<(dyn Iterator<Item=Fourcc> + 'static)> { unimplemented!() }
//! # }
//! use smithay::{
//!     backend::renderer::{
//!         damage::OutputDamageTracker,
//!         element::{
//!             Kind,
//!             solid::{SolidColorBuffer, SolidColorRenderElement},
//!         },
//!     },
//!     utils::{Point, Size},
//! };
//!
//! const WIDTH: i32 = 10;
//! const HEIGHT: i32 = 10;
//! const COLOR: [f32; 4] = [1f32, 0.9f32, 0.78f32, 1f32];
//!
//! // Initialize the solid color buffer
//! let buffer = SolidColorBuffer::new((WIDTH, HEIGHT), COLOR);
//!
//! // Initialize a static damage tracked renderer
//! let mut damage_tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
//! # let mut renderer = FakeRenderer;
//!
//! loop {
//!     // Create a render element from the buffer
//!     let location = Point::from((100, 100));
//!     let render_element = SolidColorRenderElement::from_buffer(&buffer, location, 1f64, 1.0, Kind::Unspecified);
//!
//!     // Render the element(s)
//!     damage_tracker
//!         .render_output(&mut renderer, 0, &[&render_element], [0.8, 0.8, 0.9, 1.0])
//!         .expect("failed to render output");
//! }
//! ```
use crate::{
    backend::renderer::{utils::CommitCounter, Frame, Renderer},
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use super::{AsRenderElements, Element, Id, Kind, RenderElement};

/// A single color buffer
#[derive(Debug, Clone)]
pub struct SolidColorBuffer {
    id: Id,
    size: Size<i32, Logical>,
    commit: CommitCounter,
    color: [f32; 4],
}

impl Default for SolidColorBuffer {
    fn default() -> Self {
        Self {
            id: Id::new(),
            size: Default::default(),
            commit: Default::default(),
            color: Default::default(),
        }
    }
}

impl SolidColorBuffer {
    /// Initialize a new solid color buffer with the specified size and color
    pub fn new(size: impl Into<Size<i32, Logical>>, color: [f32; 4]) -> Self {
        SolidColorBuffer {
            id: Id::new(),
            color,
            commit: CommitCounter::default(),
            size: size.into(),
        }
    }

    /// Set the new size of this solid color buffer
    ///
    /// Note: If the size matches the current size this will do nothing
    pub fn resize(&mut self, size: impl Into<Size<i32, Logical>>) {
        let size = size.into();
        if size != self.size {
            self.size = size;
            self.commit.increment();
        }
    }

    /// Set a new color on this solid color buffer
    ///
    /// Note: If the color matches the current color this will do nothing
    pub fn set_color(&mut self, color: [f32; 4]) {
        if color != self.color {
            self.color = color;
            self.commit.increment();
        }
    }

    /// Update the size and color of this solid color buffer
    ///
    /// Note: If the size and color match the current size and color this will do nothing
    pub fn update(&mut self, size: impl Into<Size<i32, Logical>>, color: [f32; 4]) {
        let size = size.into();
        if size != self.size || color != self.color {
            self.size = size;
            self.color = color;
            self.commit.increment();
        }
    }
}

/// [`Element`] to render a solid color
#[derive(Debug, Clone)]
pub struct SolidColorRenderElement {
    id: Id,
    geometry: Rectangle<i32, Physical>,
    src: Rectangle<f64, Buffer>,
    opaque_regions: Vec<Rectangle<i32, Physical>>,
    commit: CommitCounter,
    color: [f32; 4],
    kind: Kind,
}

impl SolidColorRenderElement {
    /// Create a render element from a [`SolidColorBuffer`]
    pub fn from_buffer(
        buffer: &SolidColorBuffer,
        location: impl Into<Point<i32, Physical>>,
        scale: impl Into<Scale<f64>>,
        alpha: f32,
        kind: Kind,
    ) -> Self {
        let geo = Rectangle::from_loc_and_size(location, buffer.size.to_physical_precise_round(scale));
        let color = [
            buffer.color[0] * alpha,
            buffer.color[1] * alpha,
            buffer.color[2] * alpha,
            buffer.color[3] * alpha,
        ];
        Self::new(buffer.id.clone(), geo, buffer.commit, color, kind)
    }

    /// Create a new solid color render element with the specified geometry and color
    pub fn new(
        id: impl Into<Id>,
        geometry: Rectangle<i32, Physical>,
        commit: impl Into<CommitCounter>,
        color: [f32; 4],
        kind: Kind,
    ) -> Self {
        let src = Rectangle::from_loc_and_size((0, 0), geometry.size)
            .to_f64()
            .to_logical(1f64)
            .to_buffer(1f64, Transform::Normal, &geometry.size.to_f64().to_logical(1f64));
        let opaque_regions = if color[3] == 1f32 {
            vec![Rectangle::from_loc_and_size((0, 0), geometry.size)]
        } else {
            vec![]
        };
        SolidColorRenderElement {
            id: id.into(),
            geometry,
            src,
            opaque_regions,
            commit: commit.into(),
            color,
            kind,
        }
    }
}

impl Element for SolidColorRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.src
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.opaque_regions.clone()
    }

    fn alpha(&self) -> f32 {
        1.0
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl<R: Renderer> RenderElement<R> for SolidColorRenderElement {
    #[profiling::function]
    fn draw(
        &self,
        frame: &mut <R as Renderer>::Frame<'_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        frame.draw_solid(dst, damage, self.color)
    }

    fn underlying_storage(&self, _renderer: &mut R) -> Option<super::UnderlyingStorage> {
        None
    }
}

impl<R> AsRenderElements<R> for SolidColorBuffer
where
    R: Renderer,
{
    type RenderElement = SolidColorRenderElement;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        _renderer: &mut R,
        location: crate::utils::Point<i32, crate::utils::Physical>,
        scale: crate::utils::Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        vec![SolidColorRenderElement::from_buffer(self, location, scale, alpha, Kind::Unspecified).into()]
    }
}
