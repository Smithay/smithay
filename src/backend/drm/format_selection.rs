use drm::control::{connector, crtc, Mode};
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};

use crate::backend::allocator::Allocator;

use super::{exporter::ExportFramebuffer, DrmDevice, DrmSurface};

pub struct SurfaceDefinition<'a> {
    pub crtc: crtc::Handle,
    pub mode: &'a Mode,
    pub connectors: &'a [connector::Handle],
}

pub fn select_formats<'a, A: Allocator, F: ExportFramebuffer<A::Buffer>>(
    device: &DrmDevice,
    allocator: &mut A,
    framebuffer_exporter: &F,
    surfaces: impl IntoIterator<Item = SurfaceDefinition<'a>>,
    color_formats: impl IntoIterator<Item = DrmFourcc>,
    renderer_formats: impl IntoIterator<Item = DrmFormat>,
) -> Vec<SurfaceFormat<'a>> {
    let surfaces = surfaces.into_iter();
    let color_formats = color_formats.into_iter().collect::<Vec<_>>();
    let mut surface_formats: Vec<SurfaceFormat<'a>> = Vec::with_capacity(surfaces.size_hint().0);

    // TODO: Okay, so the idea is that we first check if we have a legacy device or atomic
    // In case we have a legacy device we just search for supported formats and test them accepting it might just flicker like hell...
    // For atomic issue test commits with:
    // - All planes except the primaries for the supplied surfaces disabled
    // - All crtc disabled except the passed ones
    // - Format by format...with some limit and then just use Invalid

    todo!("Implement the actual format selection...")
}

pub struct SurfaceFormat<'a> {
    pub surface: &'a DrmSurface,
    pub code: DrmFourcc,
    pub modifiers: Vec<DrmModifier>,
}
