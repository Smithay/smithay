//! RenderElements specific to using a `Gles2Renderer`

use crate::{
    backend::{
        color::CMS,
        renderer::{
            element::{Element, Id, RenderElement, UnderlyingStorage},
            utils::CommitCounter,
        },
    },
    utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform},
};

use super::{GlesError, GlesFrame, GlesRenderer, ShaderFactory, Uniform};

use std::cell::RefCell;

/// Render element for drawing with a gles2 pixel shader
#[derive(Debug)]
pub struct PixelShaderElement<C: CMS> {
    shader: RefCell<ShaderFactory>,
    id: Id,
    commit_counter: CommitCounter,
    area: Rectangle<i32, Logical>,
    opaque_regions: Vec<Rectangle<i32, Logical>>,
    alpha: f32,
    input_profile: C::ColorProfile,
    additional_uniforms: Vec<Uniform<'static>>,
}

impl<C: CMS> PixelShaderElement<C> {
    /// Create a new [`PixelShaderElement`] from a [`ShaderFactory`],
    /// which can be constructed using [`GlesRenderer::compile_custom_shader`]
    pub fn new(
        shader: ShaderFactory,
        area: Rectangle<i32, Logical>,
        opaque_regions: Option<Vec<Rectangle<i32, Logical>>>,
        alpha: f32,
        input_profile: C::ColorProfile,
        additional_uniforms: Vec<Uniform<'_>>,
    ) -> Self {
        PixelShaderElement {
            shader: RefCell::new(shader),
            id: Id::new(),
            commit_counter: CommitCounter::default(),
            area,
            opaque_regions: opaque_regions.unwrap_or_default(),
            alpha,
            input_profile,
            additional_uniforms: additional_uniforms.into_iter().map(|u| u.into_owned()).collect(),
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
    /// (see [`Gles2Renderer::compile_custom_pixel_shader`] and [`Gles2Renderer::render_pixel_shader_to`]).
    ///
    /// This replaces the stored uniforms, you have to update all of them, partial updates are not possible.
    pub fn update_uniforms(&mut self, additional_uniforms: Vec<Uniform<'_>>) {
        self.additional_uniforms = additional_uniforms.into_iter().map(|u| u.into_owned()).collect();
        self.commit_counter.increment();
    }
}

impl<C: CMS> Element for PixelShaderElement<C> {
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

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.opaque_regions
            .iter()
            .map(|region| region.to_physical_precise_round(scale))
            .collect()
    }
}

impl<C> RenderElement<GlesRenderer, C> for PixelShaderElement<C>
where
    C: CMS,
    C::Error: Send + Sync + 'static,
{
    fn draw<'a, 'b>(
        &self,
        frame: &mut GlesFrame<'a, 'b, C>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.render_pixel_shader_to(
            &mut *self.shader.borrow_mut(),
            dst,
            Some(damage),
            self.alpha,
            &self.additional_uniforms,
            &self.input_profile,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage> {
        None
    }

    fn color_profile(&self) -> <C as CMS>::ColorProfile {
        self.input_profile.clone()
    }
}
