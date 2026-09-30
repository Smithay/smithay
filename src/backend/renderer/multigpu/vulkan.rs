//! Implementation of the multi-gpu [`GraphicsApi`] using
//! Vulkan for rendering optionally with user-provided
//! GBM devices.

#[cfg(feature = "backend_gbm")]
use std::os::fd::AsFd;
use std::{
    collections::HashMap,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use ash::vk::{self, ImageUsageFlags};
#[cfg(feature = "backend_gbm")]
use tracing::warn;

#[cfg(feature = "backend_gbm")]
use crate::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use crate::backend::{
    allocator::{
        dmabuf::{AnyError, Dmabuf, DmabufAllocator},
        vulkan::VulkanAllocator,
        Allocator,
    },
    drm::{CreateDrmNodeError, DrmNode},
    renderer::{
        multigpu::{ApiDevice, GraphicsApi},
        vulkan::{Error as RendererError, VulkanRenderer},
    },
    vulkan::{instance::InstanceError, Instance, PhysicalDevice, UnsupportedProperty, Version},
};

pub struct VulkanBackend {
    instance: Instance,
    allocator_flags: ImageUsageFlags,
    needs_enumeration: AtomicBool,
}

#[cfg(feature = "backend_gbm")]
pub struct GbmVulkanBackend<A: AsFd + 'static> {
    instance: Instance,
    devices: HashMap<DrmNode, (PhysicalDevice, GbmAllocator<A>)>,
    allocator_flags: GbmBufferFlags,
    needs_enumeration: AtomicBool,
}

pub struct VulkanDevice {
    node: DrmNode,
    renderer: VulkanRenderer,
    allocator: Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>>,
}

/// Errors raised by the [`GbmVulkanBackend`]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Vulkan Instance error
    #[error(transparent)]
    Instance(#[from] InstanceError),
    /// Vulkan enumerate error
    #[error(transparent)]
    Enumeration(vk::Result),
    /// No PhysicalDevice matching DrmNode found
    #[error("No PhysicalDevice matching DrmNode found")]
    NoMatchingPhysicalDevice,
    /// Vulkan drm property error
    #[error(transparent)]
    DrmProperty(#[from] UnsupportedProperty),
    /// VulkanRenderer error
    #[error(transparent)]
    Renderer(#[from] RendererError),
    /// Error creating a drm node
    #[error(transparent)]
    DrmNode(#[from] CreateDrmNodeError),
}

impl fmt::Debug for VulkanBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanBackend")
            .field("flags", &self.allocator_flags)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "backend_gbm")]
impl<A: AsFd + fmt::Debug + 'static> fmt::Debug for GbmVulkanBackend<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GbmVulkanBackend")
            .field("devices", &self.devices)
            .field("flags", &self.allocator_flags)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for VulkanDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanDevice")
            .field("node", &self.node)
            .field("renderer", &self.renderer)
            .finish_non_exhaustive()
    }
}

impl VulkanBackend {
    pub fn new(allocator_flags: ImageUsageFlags) -> Result<Self, InstanceError> {
        Ok(VulkanBackend {
            instance: Instance::new(Version::VERSION_1_3, None)?,
            allocator_flags: allocator_flags | ImageUsageFlags::STORAGE,
            needs_enumeration: AtomicBool::new(true),
        })
    }
}

impl GraphicsApi for VulkanBackend {
    type Device = VulkanDevice;
    type Error = Error;

    fn enumerate(&self, list: &mut Vec<Self::Device>) -> Result<(), Self::Error> {
        self.needs_enumeration.store(false, Ordering::SeqCst);

        let mut seen_nodes = Vec::new();
        for device in PhysicalDevice::enumerate(&self.instance).map_err(Error::Enumeration)? {
            let Some(node) = device.render_node()?.or(device.primary_node()?) else {
                continue;
            };

            seen_nodes.push(node);
            if !list.iter().any(|dev| dev.node == node) {
                let renderer = VulkanRenderer::new(&device, None)?;
                let allocator = Box::new(DmabufAllocator(VulkanAllocator::from_renderer(
                    &renderer,
                    self.allocator_flags,
                )));

                let device = VulkanDevice {
                    node,
                    renderer,
                    allocator,
                };
                list.push(device);
            }
        }

        list.retain(|dev| seen_nodes.contains(&dev.node));

        Ok(())
    }

    fn identifier() -> &'static str {
        "vulkan"
    }

    fn needs_enumeration(&self) -> bool {
        self.needs_enumeration.load(Ordering::Acquire)
    }
}

#[cfg(feature = "backend_gbm")]
impl<A: AsFd + Clone + 'static> GbmVulkanBackend<A> {
    pub fn new(allocator_flags: GbmBufferFlags) -> Result<Self, InstanceError> {
        Ok(GbmVulkanBackend {
            instance: Instance::new(Version::VERSION_1_3, None)?,
            devices: HashMap::new(),
            allocator_flags,
            needs_enumeration: AtomicBool::new(false),
        })
    }

    /// Sets the default flags to use for allocating buffers via the [`GbmAllocator`]
    /// provided by these backends devices.
    ///
    /// Only affects nodes added via [`add_node`][Self::add_node] *after* calling this method.
    pub fn set_allocator_flags(&mut self, flags: GbmBufferFlags) {
        self.allocator_flags = flags;
    }

    /// Add a new GBM device for a given node to the api
    pub fn add_node(&mut self, node: DrmNode, gbm: GbmDevice<A>) -> Result<(), Error> {
        if self.devices.contains_key(&node) {
            return Ok(());
        }

        let phys = PhysicalDevice::enumerate(&self.instance)
            .map_err(Error::Enumeration)?
            .find(|phy| {
                phy.render_node()
                    .ok()
                    .flatten()
                    .or(phy.primary_node().ok().flatten())
                    .is_some_and(|n| n == node)
            })
            .ok_or(Error::NoMatchingPhysicalDevice)?;
        let allocator = GbmAllocator::new(gbm.clone(), self.allocator_flags);
        self.devices.insert(node, (phys, allocator));
        self.needs_enumeration.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Remove a given node from the api
    pub fn remove_node(&mut self, node: &DrmNode) {
        if self.devices.remove(node).is_some() {
            self.needs_enumeration.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(feature = "backend_gbm")]
impl<A: AsFd + Clone + 'static> GraphicsApi for GbmVulkanBackend<A> {
    type Device = VulkanDevice;
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
            .map(|(node, (phys, gbm))| {
                let renderer = VulkanRenderer::new(&phys, None)?;
                let allocator = Box::new(DmabufAllocator(gbm.clone()));

                Ok(VulkanDevice {
                    node: *node,
                    renderer,
                    allocator,
                })
            })
            .flat_map(|x: Result<VulkanDevice, Error>| match x {
                Ok(x) => Some(x),
                Err(x) => {
                    warn!("Skipping GbmDevice: {}", x);
                    None
                }
            })
            .collect::<Vec<VulkanDevice>>();
        list.extend(new_renderers);

        Ok(())
    }

    fn identifier() -> &'static str {
        "gbm_vulkan"
    }

    fn needs_enumeration(&self) -> bool {
        self.needs_enumeration.load(Ordering::Acquire)
    }
}

impl ApiDevice for VulkanDevice {
    type Renderer = VulkanRenderer;

    fn renderer(&self) -> &Self::Renderer {
        &self.renderer
    }

    fn renderer_mut(&mut self) -> &mut Self::Renderer {
        &mut self.renderer
    }

    fn allocator(&mut self) -> &mut dyn Allocator<Buffer = Dmabuf, Error = AnyError> {
        &mut self.allocator as &mut _
    }

    fn node(&self) -> &DrmNode {
        &self.node
    }

    fn can_do_cross_device_imports(&self) -> bool {
        true
    }
}
