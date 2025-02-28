//! Implementation of the multi-gpu [`GraphicsApi`] using
//! user provided DRM devices and pixman for rendering.

use std::fmt;
use std::sync::atomic::Ordering;
use std::{collections::HashMap, sync::atomic::AtomicBool};

use drm::node::{CreateDrmNodeError, DrmNode};
use tracing::warn;
use wayland_server::protocol::wl_buffer;

use crate::backend::allocator::dmabuf::DmabufAllocator;
use crate::backend::renderer::pixman::PixmanError;
use crate::backend::renderer::{ImportDma, ImportMem, Renderer};
use crate::backend::SwapBuffersError;
use crate::backend::{
    allocator::{
        dmabuf::{AnyError, Dmabuf},
        dumb::DumbAllocator,
        Allocator,
    },
    drm::DrmDevice,
    renderer::pixman::PixmanRenderer,
};
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use crate::backend::{
    egl::{display::EGLBufferReader, Error as EGLError},
    renderer::{multigpu::Error as MultigpuError, ImportEgl},
};
use crate::utils::{Buffer as BufferCoords, Rectangle};

use super::{ApiDevice, GraphicsApi, MultiRenderer, TryImportEgl};

/// Errors raised by the [`DrmPixmanBackend`]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Pixman error
    #[error(transparent)]
    Pixman(#[from] PixmanError),
    /// Error creating a drm node
    #[error(transparent)]
    DrmNode(#[from] CreateDrmNodeError),
}

impl From<Error> for SwapBuffersError {
    #[inline]
    fn from(err: Error) -> SwapBuffersError {
        match err {
            x @ Error::DrmNode(_) => SwapBuffersError::ContextLost(Box::new(x)),
            Error::Pixman(x) => x.into(),
        }
    }
}

/// A [`GraphicsApi`] utilizing user-provided DRM Devices and Pixman for rendering.
#[derive(Debug)]
pub struct DrmPixmanBackend {
    devices: HashMap<DrmNode, DumbAllocator>,
    needs_enumeration: AtomicBool,
}

impl DrmPixmanBackend {
    /// Add a new DRM device for a given node to the api
    pub fn add_node(&mut self, node: DrmNode, drm: &DrmDevice) {
        if self.devices.contains_key(&node) {
            return;
        }

        let allocator = DumbAllocator::new(drm.device_fd().clone());
        self.devices.insert(node, allocator);
        self.needs_enumeration.store(true, Ordering::SeqCst);
    }

    /// Remove a given node from the api
    pub fn remove_node(&mut self, node: &DrmNode) {
        if self.devices.remove(node).is_some() {
            self.needs_enumeration.store(true, Ordering::SeqCst);
        }
    }
}

impl Default for DrmPixmanBackend {
    #[inline]
    fn default() -> Self {
        Self {
            devices: Default::default(),
            needs_enumeration: AtomicBool::new(true),
        }
    }
}

impl GraphicsApi for DrmPixmanBackend {
    type Device = DrmPixmanDevice;

    type Error = Error;

    fn enumerate(&self, list: &mut Vec<Self::Device>) -> Result<(), Self::Error> {
        self.needs_enumeration.store(false, Ordering::SeqCst);

        // remove old stuff
        list.retain(|renderer| {
            self.devices
                .keys()
                .any(|node| renderer.node.dev_id() == node.dev_id())
        });

        // add new stuff
        let new_renderers = self
            .devices
            .iter()
            .filter(|(node, _)| {
                !list
                    .iter()
                    .any(|renderer| renderer.node.dev_id() == node.dev_id())
            })
            .map(|(node, drm)| {
                let renderer = PixmanRenderer::new()?;

                Ok(DrmPixmanDevice {
                    node: *node,
                    renderer,
                    allocator: Box::new(DmabufAllocator(drm.clone())),
                })
            })
            .flat_map(|x: Result<DrmPixmanDevice, Error>| match x {
                Ok(x) => Some(x),
                Err(x) => {
                    warn!("Skipping DrmDevice: {}", x);
                    None
                }
            })
            .collect::<Vec<DrmPixmanDevice>>();
        list.extend(new_renderers);

        // but don't replace already initialized renderers

        Ok(())
    }

    fn needs_enumeration(&self) -> bool {
        self.needs_enumeration.load(Ordering::Acquire)
    }

    fn identifier() -> &'static str {
        "drm_pixman"
    }
}

/// [`ApiDevice`] of the [`DrmPixmanBackend`]
pub struct DrmPixmanDevice {
    node: DrmNode,
    renderer: PixmanRenderer,
    allocator: Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>>,
}

impl fmt::Debug for DrmPixmanDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DrmPixmanDevice")
            .field("node", &self.node)
            .field("renderer", &self.renderer)
            .finish()
    }
}

impl ApiDevice for DrmPixmanDevice {
    type Renderer = PixmanRenderer;

    fn renderer(&self) -> &Self::Renderer {
        &self.renderer
    }

    fn renderer_mut(&mut self) -> &mut Self::Renderer {
        &mut self.renderer
    }

    fn allocator(&mut self) -> &mut dyn Allocator<Buffer = Dmabuf, Error = AnyError> {
        &mut self.allocator
    }

    fn node(&self) -> &DrmNode {
        &self.node
    }
}

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
impl TryImportEgl<PixmanRenderer> for DrmPixmanDevice {
    type Error = PixmanError;

    fn try_import_egl(
        _renderer: &mut PixmanRenderer,
        _buffer: &wl_buffer::WlBuffer,
    ) -> Result<Dmabuf, Self::Error> {
        return Err(PixmanError::Unsupported);
    }
}

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
impl<T> ImportEgl for MultiRenderer<'_, '_, DrmPixmanBackend, T>
where
    T: GraphicsApi,
    <T as GraphicsApi>::Error: 'static,
    <<T as GraphicsApi>::Device as ApiDevice>::Renderer: ImportDma + ImportMem,
    <<<T as GraphicsApi>::Device as ApiDevice>::Renderer as Renderer>::Error: 'static,
{
    fn bind_wl_display(&mut self, display: &wayland_server::DisplayHandle) -> Result<(), EGLError> {
        self.render.renderer_mut().bind_wl_display(display)
    }
    fn unbind_wl_display(&mut self) {
        self.render.renderer_mut().unbind_wl_display()
    }
    fn egl_reader(&self) -> Option<&EGLBufferReader> {
        self.render.renderer().egl_reader()
    }

    #[profiling::function]
    fn import_egl_buffer(
        &mut self,
        _buffer: &wl_buffer::WlBuffer,
        _surface: Option<&crate::wayland::compositor::SurfaceData>,
        _damage: &[Rectangle<i32, BufferCoords>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        Err(MultigpuError::Render(PixmanError::Unsupported))
    }
}
