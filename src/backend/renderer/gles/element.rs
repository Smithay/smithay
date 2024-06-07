//! RenderElements specific to using a `GlesRenderer`

use crate::{
    backend::renderer::{
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        utils::{CommitCounter, OpaqueRegions},
    },
    utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform},
};

use super::{GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, Uniform};

/// Render element for drawing with a gles2 pixel shader
#[derive(Debug, Clone)]
pub struct PixelShaderElement {
    shader: GlesPixelProgram,
    id: Id,
    commit_counter: CommitCounter,
    area: Rectangle<i32, Logical>,
    opaque_regions: Vec<Rectangle<i32, Logical>>,
    alpha: f32,
    additional_uniforms: Vec<Uniform<'static>>,
    kind: Kind,
}

impl PixelShaderElement {
    /// Create a new [`PixelShaderElement`] from a [`GlesPixelProgram`],
    /// which can be constructed using [`GlesRenderer::compile_custom_pixel_shader`]
    pub fn new(
        shader: GlesPixelProgram,
        area: Rectangle<i32, Logical>,
        opaque_regions: Option<Vec<Rectangle<i32, Logical>>>,
        alpha: f32,
        additional_uniforms: Vec<Uniform<'_>>,
        kind: Kind,
    ) -> Self {
        PixelShaderElement {
            shader,
            id: Id::new(),
            commit_counter: CommitCounter::default(),
            area,
            opaque_regions: opaque_regions.unwrap_or_default(),
            alpha,
            additional_uniforms: additional_uniforms.into_iter().map(|u| u.into_owned()).collect(),
            kind,
        }
    }

    /// Resize the canvas area
    pub fn resize(
        &mut self,
        area: Rectangle<i32, Logical>,
        opaque_regions: Option<Vec<Rectangle<i32, Logical>>>,
    ) {
        let opaque_regions = opaque_regions.unwrap_or_default();
        if self.area != area || self.opaque_regions != opaque_regions {
            self.area = area;
            self.opaque_regions = opaque_regions;
            self.commit_counter.increment();
        }
    }

    /// Update the additional uniforms
    /// (see [`GlesRenderer::compile_custom_pixel_shader`] and [`GlesFrame::render_pixel_shader_to`]).
    ///
    /// This replaces the stored uniforms, you have to update all of them, partial updates are not possible.
    pub fn update_uniforms(&mut self, additional_uniforms: Vec<Uniform<'_>>) {
        self.additional_uniforms = additional_uniforms.into_iter().map(|u| u.into_owned()).collect();
        self.commit_counter.increment();
    }
}

impl Element for PixelShaderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit_counter
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.area
            .to_f64()
            .to_buffer(1.0, Transform::Normal, &self.area.size.to_f64())
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.area.to_physical_precise_round(scale)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.opaque_regions
            .iter()
            .map(|region| region.to_physical_precise_round(scale))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl RenderElement<GlesRenderer> for PixelShaderElement {
    #[profiling::function]
    fn draw(
        &self,
        frame: &mut GlesFrame<'_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.render_pixel_shader_to(
            &self.shader,
            dst,
            Some(damage),
            self.alpha,
            &self.additional_uniforms,
        )
    }

    #[inline]
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
