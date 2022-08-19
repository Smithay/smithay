use crate::backend::renderer::{
    buffer_type,
    output::{
        element::{RenderElement, UnderlyingStorage},
        OutputRender,
    },
    BufferType, ImportAll, Renderer, Texture,
};

/// Specialized render for drm output which uses planes
#[derive(Debug)]
pub struct DrmOutputRender {
    output_render: OutputRender,
}

impl DrmOutputRender {
    /// Render the output
    pub fn render_output<R, E>(
        &mut self,
        renderer: &mut R,
        elements: &[E],
        clear_color: [f32; 4],
        log: &slog::Logger,
    ) where
        E: RenderElement<R>,
        R: Renderer + ImportAll + std::fmt::Debug,
        <R as Renderer>::TextureId: Texture + std::fmt::Debug + 'static,
    {
        for element in elements {
            if let Some(underlying_storage) = element.underlying_storage(renderer) {
                match underlying_storage {
                    UnderlyingStorage::Wayland(buffer) => {
                        let buffer_type = buffer_type(&buffer);

                        let buffer_supports_direct_scan_out = matches!(buffer_type, Some(BufferType::Dma));

                        if buffer_supports_direct_scan_out {
                            // Try to assign the element to a plane
                            todo!()
                        } else {
                            // No direct-scan out possible, needs to be rendered on the primary plane
                        }
                    }
                    UnderlyingStorage::External(_) => {
                        // No direct-scan out possible, needs to be rendered on the primary plane
                    }
                }
            } else {
                // No direct-scan out possible, needs to be rendered on the primary plane
            }
        }

        // Draw the remaining elements on the primary plane
        self.output_render
            .render_output(renderer, 0, elements, clear_color, log)
            .expect("failed to render");
    }
}
