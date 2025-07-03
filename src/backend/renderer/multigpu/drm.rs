//! Implementation of the multi-gpu [`GraphicsApi`] using
//! user provided DRM devices and pixman for rendering.

use std::fmt;
use std::sync::atomic::Ordering;
use std::{collections::HashMap, sync::atomic::AtomicBool};

use drm::node::{CreateDrmNodeError, DrmNode};
use tracing::warn;
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use wayland_server::protocol::wl_buffer;

#[cfg(feature = "backend_gbm")]
use crate::backend::{
    allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
    drm::DrmDeviceFd,
};
use crate::backend::{
    allocator::{
        dmabuf::{AnyError, Dmabuf, DmabufAllocator},
        dumb::DumbAllocator,
        Allocator,
    },
    drm::DrmDevice,
    renderer::{
        multigpu::{ApiDevice, GraphicsApi},
        pixman::{PixmanError, PixmanRenderer},
    },
    SwapBuffersError,
};
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use crate::{
    backend::{
        egl::{display::EGLBufferReader, Error as EGLError},
        renderer::{
            multigpu::{Error as MultigpuError, MultiRenderer, TryImportEgl},
            ImportDma, ImportEgl, ImportMem, RendererSuper,
        },
    },
    utils::{Buffer as BufferCoords, Rectangle},
};

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

/// Device used to allocate buffers from
#[derive(Debug)]
pub enum AllocatorDevice<'a> {
    /// Drm device to be used
    Drm(&'a DrmDevice),
    /// Gbm device to be used
    #[cfg(feature = "backend_gbm")]
    Gbm(&'a GbmDevice<DrmDeviceFd>),
}

impl<'a> From<&'a DrmDevice> for AllocatorDevice<'a> {
    fn from(device: &'a DrmDevice) -> Self {
        AllocatorDevice::Drm(device)
    }
}

#[cfg(feature = "backend_gbm")]
impl<'a> From<&'a GbmDevice<DrmDeviceFd>> for AllocatorDevice<'a> {
    fn from(device: &'a GbmDevice<DrmDeviceFd>) -> Self {
        AllocatorDevice::Gbm(device)
    }
}

#[derive(Debug)]
enum BackendAllocator {
    Dumb(DumbAllocator),
    #[cfg(feature = "backend_gbm")]
    Gbm(GbmAllocator<DrmDeviceFd>),
}

/// A [`GraphicsApi`] utilizing user-provided DRM Devices and Pixman for rendering.
#[derive(Debug)]
pub struct DrmPixmanBackend {
    devices: HashMap<DrmNode, BackendAllocator>,
    #[cfg(feature = "backend_gbm")]
    gbm_allocator_flags: GbmBufferFlags,
    needs_enumeration: AtomicBool,
}

impl DrmPixmanBackend {
    /// Add a new DRM device for a given node to the api
    pub fn add_node<'a>(&mut self, node: DrmNode, allocator_device: impl Into<AllocatorDevice<'a>>) {
        if self.devices.contains_key(&node) {
            return;
        }

        let allocator_device = allocator_device.into();
        let allocator = match allocator_device {
            AllocatorDevice::Drm(drm) => BackendAllocator::Dumb(DumbAllocator::new(drm.device_fd().clone())),
            #[cfg(feature = "backend_gbm")]
            AllocatorDevice::Gbm(gbm) => {
                BackendAllocator::Gbm(GbmAllocator::new(gbm.clone(), self.gbm_allocator_flags))
            }
        };

        self.devices.insert(node, allocator);
        self.needs_enumeration.store(true, Ordering::SeqCst);
    }

    /// Sets the default flags to use for allocating buffers via the [`GbmAllocator`]
    /// provided by these backends devices.
    ///
    /// Only affects nodes added via [`add_node`][Self::add_node] *after* calling this method.
    #[cfg(feature = "backend_gbm")]
    pub fn set_gbm_allocator_flags(&mut self, flags: GbmBufferFlags) {
        self.gbm_allocator_flags = flags;
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
            #[cfg(feature = "backend_gbm")]
            gbm_allocator_flags: GbmBufferFlags::RENDERING,
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
            .map(|(node, backend_allocator)| {
                let renderer = PixmanRenderer::new()?;

                let allocator: Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>> = match backend_allocator
                {
                    BackendAllocator::Dumb(allocator) => Box::new(DmabufAllocator(allocator.clone())),
                    #[cfg(feature = "backend_gbm")]
                    BackendAllocator::Gbm(allocator) => Box::new(DmabufAllocator(allocator.clone())),
                };

                Ok(DrmPixmanDevice {
                    node: *node,
                    renderer,
                    allocator,
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
        Err(PixmanError::Unsupported)
    }
}

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
impl<T> ImportEgl for MultiRenderer<'_, '_, DrmPixmanBackend, T>
where
    T: GraphicsApi,
    <T as GraphicsApi>::Error: 'static,
    <<T as GraphicsApi>::Device as ApiDevice>::Renderer: ImportDma + ImportMem,
    <<<T as GraphicsApi>::Device as ApiDevice>::Renderer as RendererSuper>::Error: 'static,
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
    ) -> Result<Self::TextureId, Self::Error> {
        Err(MultigpuError::Render(PixmanError::Unsupported))
    }
}
