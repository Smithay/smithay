//! Implementation for ExportFramebuffer and related utilizes for gbm types

use std::os::unix::io::AsFd;

use drm::node::DrmNode;

use super::{ExportBuffer, ExportFramebuffer};
#[cfg(feature = "wayland_frontend")]
use crate::backend::drm::gbm::framebuffer_from_wayland_buffer;
use crate::backend::{
    allocator::{
        dmabuf::AsDmabuf,
        gbm::{GbmBuffer, GbmConvertError},
    },
    drm::{
        gbm::{framebuffer_from_bo, framebuffer_from_dmabuf, Error as GbmError, GbmFramebuffer},
        DrmDeviceFd,
    },
};

/// Error for [`GbmFramebufferExporter`]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Exporting the [`GbmBuffer`] as a [`Dmabuf`](crate::backend::allocator::dmabuf::Dmabuf) failed
    #[error(transparent)]
    Dmabuf(#[from] GbmConvertError),
    /// Exporting the [`GbmBuffer`] failed
    #[error(transparent)]
    Gbm(#[from] GbmError),
}

/// Export framebuffers based on [`gbm::Device`]
#[derive(Debug, Clone)]
pub struct GbmFramebufferExporter<A: AsFd + 'static> {
    gbm: gbm::Device<A>,
    drm_node: Option<DrmNode>,
    #[cfg_attr(not(feature = "wayland_frontend"), allow(unused))]
    import_node: NodeFilter,
}

impl<A: AsFd + 'static> GbmFramebufferExporter<A> {
    /// Initialize a new framebuffer exporter.
    ///
    /// `import_node` will be used to filter dmabufs to originate from a particular
    /// device before considering them for direct scanout.
    ///
    /// If `import_node` is [`NodeFilter::None`], direct-scanout of client-buffers
    /// won't be used.
    pub fn new(gbm: gbm::Device<A>, import_node: NodeFilter) -> Self {
        let drm_node = DrmNode::from_file(gbm.as_fd()).ok();
        Self {
            gbm,
            drm_node,
            import_node,
        }
    }
}

impl<A: AsFd + 'static> ExportFramebuffer<GbmBuffer> for GbmFramebufferExporter<A> {
    type Framebuffer = GbmFramebuffer;
    type Error = Error;

    #[profiling::function]
    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, GbmBuffer>,
        use_opaque: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        let framebuffer = match buffer {
            #[cfg(feature = "wayland_frontend")]
            ExportBuffer::Wayland(wl_buffer) => {
                framebuffer_from_wayland_buffer(drm, &self.gbm, wl_buffer, use_opaque)?
            }
            ExportBuffer::Allocator(buffer) => {
                let foreign = self.drm_node.is_none()
                    || buffer.device_node().is_none()
                    || self.drm_node != buffer.device_node();
                if foreign {
                    tracing::debug!("importing foreign buffer");
                    let dmabuf = buffer.export()?;
                    framebuffer_from_dmabuf(drm, &self.gbm, &dmabuf, use_opaque, true).map(Some)?
                } else {
                    framebuffer_from_bo(drm, buffer, use_opaque)
                        .map_err(GbmError::Drm)
                        .map(Some)?
                }
            }
        };
        Ok(framebuffer)
    }

    #[inline]
    #[cfg(feature = "wayland_frontend")]
    fn can_add_framebuffer(&self, buffer: &ExportBuffer<'_, GbmBuffer>) -> bool {
        match buffer {
            #[cfg(not(all(feature = "backend_egl", feature = "use_system_lib")))]
            ExportBuffer::Wayland(buffer) => {
                let node = crate::wayland::dmabuf::get_dmabuf(buffer)
                    .ok()
                    .and_then(|buf| buf.node());
                self.import_node == node
            }
            #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
            ExportBuffer::Wayland(buffer) => match crate::backend::renderer::buffer_type(buffer) {
                Some(crate::backend::renderer::BufferType::Dma) => {
                    let node = crate::wayland::dmabuf::get_dmabuf(buffer).unwrap().node();
                    self.import_node == node
                }
                // Argubly we need specialization here. If the renderer (which we have in `element_config`, which calls this function)
                // has `ImportEGL`, we can verify that `EGLBufferRender` is some, which means we have the renderer advertised via wl_drm,
                // which means this is probably fine.
                // If we don't have `ImportEGL` or `EGLBufferReader` is none, we should reject this, but we don't want to require `ImportEGL`.
                //
                // So for now hope that `gbm_framebuffer_from_wayland_buffer` is smart enough to deal with this correctly.
                // (And most modern compositor don't use wl_drm anyway.)
                Some(crate::backend::renderer::BufferType::Egl) => true,
                _ => false,
            },
            ExportBuffer::Allocator(_) => true,
        }
    }

    #[inline]
    #[cfg(not(feature = "wayland_frontend"))]
    fn can_add_framebuffer(&self, buffer: &ExportBuffer<'_, GbmBuffer>) -> bool {
        match buffer {
            ExportBuffer::Allocator(_) => true,
        }
    }
}

/// Filter to matching nodes against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeFilter {
    /// Consider no nodes a match.
    None,
    /// Consider all nodes a match.
    All,
    /// Consider only the specified node a match.
    Node(DrmNode),
}

impl PartialEq<Option<DrmNode>> for NodeFilter {
    fn eq(&self, other: &Option<DrmNode>) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Node(node) => other.is_some_and(|n| &n == node),
        }
    }
}

impl From<DrmNode> for NodeFilter {
    fn from(node: DrmNode) -> Self {
        Self::Node(node)
    }
}

impl From<Option<DrmNode>> for NodeFilter {
    fn from(node: Option<DrmNode>) -> Self {
        match node {
            Some(node) => Self::Node(node),
            None => Self::None,
        }
    }
}
