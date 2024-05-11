//! Types to initialize and use Vulkan.
//!
//! This module provides some thin abstractions over [`ash`](https://crates.io/crates/ash) for initializing
//! Vulkan.
//!
//! This module does not provide abstractions for logical devices, rendering or memory allocation. These
//! should instead be provided in higher level abstractions.
//!
//! Smithay requires at least Vulkan 1.1[^version].
//!
//! # [`Instance`]
//!
//! To use Vulkan, you would first instantiate an [`Instance`]. An instance is effectively the Vulkan library
//! and provides some information about the environment. This includes the list of supported
//! [instance extensions](Instance::enumerate_extensions) and the list of available physical devices.
//!
//! An instance is constructed using an [`Instance::new`] or [`Instance::with_extensions`].
//!
//! ## Layers
//!
//! The validation layers will be enabled if debug assertions are enabled and the validation layers are
//! available on your system.
//!
//! ## Instance extensions
//!
//! Instances may be created with some enabled extensions. Note that any features gated by an extension are
//! only available if the extension (and it's dependencies) are enabled (you will get a validation error if
//! you ignore that).
//!
//! Some features such as window system integration are only available if their features are enabled.
//! Available instance extensions may be obtained using [`Instance::enumerate_extensions`].
//!
//! # [`PhysicalDevice`]
//!
//! Once you have an instance, you may want to find a suitable device to use. A [`PhysicalDevice`] describes a
//! Vulkan implementation that may correspond to a real or virtual device.
//!
//! To get all the available devices, use [`PhysicalDevice::enumerate`].
//!
//! Physical devices are also describe the logical devices that can be created. A physical device can describe
//! a variety of properties that may be used for device selection, including but not limited to:
//! - [Device name](PhysicalDevice::name)
//! - [Supported Vulkan API version](PhysicalDevice::api_version)
//! - [Type](PhysicalDevice::ty) of the device
//! - [Driver information](PhysicalDevice::driver)
//! - [Extensions](PhysicalDevice::device_extensions)
//! - [Features](PhysicalDevice::features) and [limits](PhysicalDevice::limits)
//!
//! Physical devices implement [`Eq`][^device_eq], meaning two physical devices can be tested for equality.
//!
//! ## Device extensions
//!
//! Depending on the device extension (see the Vulkan specification), a device extension may indicate some
//! physical device data is available or indicate some device feature is supported. Any features that are
//! added using a device extension must be enabled in order to be used.
//!
//! [^version]: Internally Vulkan 1.1 is required because several extensions that were made part of core are
//! used quite extensively in some of the extensions our abstractions use. The vast majority of systems using
//! Vulkan also support at least Vulkan 1.1. If you need Vulkan 1.0 support, please open an issue and we can
//! discuss Vulkan 1.0 support.
//!
//! [^device_eq]: Two physical devices are only equal if both physical devices are created from the same
//! instance and have the same physical device handle.

#![warn(missing_debug_implementations)]
#![forbid(unsafe_op_in_unsafe_fn)]

use std::{
    env::{self, VarError},
    ffi::{CStr, CString},
    sync::Arc,
};

use ash::{
    extensions::ext::DebugUtils,
    prelude::VkResult,
    vk::{self, PhysicalDeviceDriverProperties, PhysicalDeviceDrmPropertiesEXT},
    Entry,
};
use libc::c_void;
use once_cell::sync::Lazy;
use scopeguard::ScopeGuard;
use tracing::{error, info, info_span, instrument, trace, warn};

use crate::backend::vulkan::inner::DebugState;

use self::{inner::InstanceInner, version::Version};

#[cfg(feature = "backend_drm")]
use super::drm::DrmNode;

mod inner;
mod phd;

pub mod version;

static LIBRARY: Lazy<Result<Entry, LoadError>> =
    Lazy::new(|| unsafe { Entry::load().map_err(|_| LoadError) });

/// Error loading the Vulkan library
#[derive(Debug, thiserror::Error)]
#[error("Failed to load the Vulkan library")]
pub struct LoadError;

/// An error that may occur when creating an [`Instance`].
#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    /// The instance was created using Vulkan 1.0.
    #[error("Smithay requires at least Vulkan 1.1")]
    UnsupportedVersion,

    /// Failed to load the Vulkan library.
    #[error(transparent)]
    Load(#[from] LoadError),

    /// Vulkan API error.
    #[error(transparent)]
    Vk(#[from] vk::Result),
}

/// Error returned when a physical device property is not supported
#[derive(Debug, thiserror::Error)]
pub enum UnsupportedProperty {
    /// Some required extensions are not available.
    #[error("The following extensions are not available {0:?}")]
    Extensions(&'static [&'static CStr]),
}

/// App info to be passed to the Vulkan implementation.
#[derive(Debug)]
pub struct AppInfo {
    /// Name of the app.
    pub name: String,
    /// Version of the app.
    pub version: Version,
}

/// A Vulkan instance.
///
/// An instance is the object which tracks an application's Vulkan state. An instance allows an application to
/// get a list of physical devices.
///
/// In the Vulkan it is common to have objects which may not outlive the parent instance. A great way to
/// ensure compliance when using child objects is to [`Clone`] the instance and keep a handle with the child
/// object. This will ensure the child object does not outlive the instance.
///
/// An instance is [`Send`] and [`Sync`] which allows meaning multiple threads to access the Vulkan state.
/// Note that this **does not** mean the entire Vulkan API is thread safe, you will need to read the
/// specification to determine what parts of the Vulkan API require external synchronization.
///
/// # Instance extensions
///
/// In order to use features exposed through instance extensions (such as window system integration), you must
/// enable the extensions corresponding to the feature.
///
/// By default, [`Instance`] will automatically try to enable the following instance extensions if available:
/// * `VK_EXT_debug_utils`
///
/// No users should assume the instance extensions that are automatically enabled are available.
#[derive(Debug, Clone)]
pub struct Instance(Arc<InstanceInner>);

impl Instance {
    /// Creates a new [`Instance`].
    pub fn new(max_version: Version, app_info: Option<AppInfo>) -> Result<Instance, InstanceError> {
        unsafe { Self::with_extensions(max_version, app_info, &[]) }
    }

    /// Creates a new [`Instance`] with some additionally specified extensions.
    ///
    /// # Safety
    ///
    /// * All valid usage requirements specified by [`vkCreateInstance`](https://www.khronos.org/registry/vulkan/specs/1.3-extensions/man/html/vkCreateInstance.html)
    ///   must be satisfied.
    /// * Any enabled extensions must also have the dependency extensions enabled
    ///   (see `VUID-vkCreateInstance-ppEnabledExtensionNames-01388`).
    pub unsafe fn with_extensions(
        max_version: Version,
        app_info: Option<AppInfo>,
        extensions: &[&'static CStr],
    ) -> Result<Instance, InstanceError> {
        assert!(
            max_version >= Version::VERSION_1_1,
            "Smithay requires at least Vulkan 1.1"
        );
        let requested_max_version = get_env_or_max_version(max_version);

        let span = info_span!("backend_vulkan", version = tracing::field::Empty);
        let _guard = span.enter();

        // Determine the maximum instance version that is possible.
        let max_version = {
            LIBRARY
                .as_ref()
                .or(Err(LoadError))?
                .try_enumerate_instance_version()
                // Any allocation errors must be the result of the loader or layers
                .or(Err(LoadError))?
                .map(Version::from_raw)
                // Vulkan 1.0 does not have `vkEnumerateInstanceVersion`.
                .unwrap_or(Version::VERSION_1_0)
        };

        if max_version == Version::VERSION_1_0 {
            error!("Vulkan does not support version 1.1");
            return Err(InstanceError::UnsupportedVersion);
        }

        // Pick the lower of the requested max version and max possible version
        let api_version = Version::from_raw(u32::min(max_version.to_raw(), requested_max_version.to_raw()));
        span.record("version", tracing::field::display(api_version));

        let available_layers = Self::enumerate_layers()?.collect::<Vec<_>>();
        let available_extensions = Self::enumerate_extensions()?.collect::<Vec<_>>();

        let mut layers = Vec::new();

        // Enable debug layers if present and debug assertions are enabled.
        if cfg!(debug_assertions) {
            const VALIDATION: &CStr =
                unsafe { CStr::from_bytes_with_nul_unchecked(b"VK_LAYER_KHRONOS_validation\0") };

            if available_layers
                .iter()
                .any(|layer| layer.as_c_str() == VALIDATION)
            {
                layers.push(VALIDATION);
            } else {
                warn!("Validation layers not available. These can be installed through your package manager",);
            }
        }

        let mut enabled_extensions = Vec::<&'static CStr>::new();
        enabled_extensions.extend(extensions);

        // Enable debug utils if available.
        let has_debug_utils = available_extensions
            .iter()
            .any(|name| name.as_c_str() == vk::ExtDebugUtilsFn::name());

        if has_debug_utils {
            enabled_extensions.push(vk::ExtDebugUtilsFn::name());
        }

        // Both of these are safe because both vecs contain static CStrs.
        let extension_pointers = enabled_extensions
            .iter()
            .map(|name| name.as_ptr())
            .collect::<Vec<_>>();
        let layer_pointers = layers.iter().map(|name| name.as_ptr()).collect::<Vec<_>>();

        let app_version = app_info.as_ref().map(|info| info.version.to_raw());
        let app_name =
            app_info.map(|info| CString::new(info.name).expect("app name contains null terminator"));
        let mut app_info = vk::ApplicationInfo::builder()
            .api_version(api_version.to_raw())
            // SAFETY: null terminated with no interior null bytes.
            .engine_name(unsafe { CStr::from_bytes_with_nul_unchecked(b"Smithay\0") })
            .engine_version(Version::SMITHAY.to_raw());

        if let Some(app_version) = app_version {
            app_info = app_info.application_version(app_version);
        }

        if let Some(app_name) = &app_name {
            app_info = app_info.application_name(app_name);
        }

        let library = LIBRARY.as_ref().map_err(|_| LoadError)?;
        let create_info = vk::InstanceCreateInfo::builder()
            .application_info(&app_info)
            .enabled_layer_names(&layer_pointers)
            .enabled_extension_names(&extension_pointers);

        // Place the instance in a scopeguard in case creating the debug messenger fails.
        let instance = scopeguard::guard(
            unsafe { library.create_instance(&create_info, None) }?,
            |instance| unsafe {
                instance.destroy_instance(None);
            },
        );

        // Setup the debug utils
        let debug_state = if has_debug_utils {
            let span = info_span!("backend_vulkan_debug");
            let debug_utils = DebugUtils::new(library, &instance);
            // Place the pointer to the span in a scopeguard to prevent a memory leak in case creating the
            // debug messenger fails.
            let span_ptr = scopeguard::guard(Box::into_raw(Box::new(span)), |ptr| unsafe {
                let _ = Box::from_raw(ptr);
            });

            let create_info = vk::DebugUtilsMessengerCreateInfoEXT::builder()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                        | vk::DebugUtilsMessageSeverityFlagsEXT::VERBOSE
                        | vk::DebugUtilsMessageSeverityFlagsEXT::INFO
                        | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
                )
                .pfn_user_callback(Some(vulkan_debug_utils_callback))
                .user_data(*span_ptr as *mut _);

            let debug_messenger = unsafe { debug_utils.create_debug_utils_messenger(&create_info, None) }?;

            // Disarm the destructor for the logger pointer since the instance is now responsible for
            // destroying the logger.
            let span_ptr = ScopeGuard::into_inner(span_ptr);

            Some(DebugState {
                debug_utils,
                debug_messenger,
                span_ptr,
            })
        } else {
            None
        };

        // Creating the debug messenger was successful, disarm the scopeguard and let InstanceInner manage
        // destroying the instance.
        let instance = ScopeGuard::into_inner(instance);
        drop(_guard);
        let inner = InstanceInner {
            instance,
            version: api_version,
            debug_state,
            span,
            enabled_extensions,
        };

        info!("Created new instance");
        info!("Enabled instance extensions: {:?}", inner.enabled_extensions);

        #[allow(clippy::arc_with_non_send_sync)]
        Ok(Instance(Arc::new(inner)))
    }

    /// Returns an iterator which contains the available instance extensions on the system.
    pub fn enumerate_extensions() -> Result<impl Iterator<Item = CString>, LoadError> {
        let library = LIBRARY.as_ref().or(Err(LoadError))?;

        let extensions = library
            .enumerate_instance_extension_properties(None)
            .or(Err(LoadError))?
            .into_iter()
            .map(|properties| {
                // SAFETY: Vulkan guarantees the string is null terminated.
                unsafe { CStr::from_ptr(&properties.extension_name as *const _) }.to_owned()
            })
            .collect::<Vec<_>>()
            .into_iter();

        Ok(extensions)
    }

    /// Returns the enabled instance extensions.
    pub fn enabled_extensions(&self) -> impl Iterator<Item = &CStr> {
        self.0.enabled_extensions.iter().copied()
    }

    /// Returns true if the specified instance extension is enabled.
    ///
    /// This function may be used to ensure safe access to features provided by instance extensions.
    pub fn is_extension_enabled(&self, extension: &CStr) -> bool {
        self.enabled_extensions().any(|name| name == extension)
    }

    /// Returns the version of Vulkan supported by this instance.
    ///
    /// This corresponds to the version specified when building the instance.
    pub fn api_version(&self) -> Version {
        self.0.version
    }

    /// Returns a reference to the underlying [`ash::Instance`].
    ///
    /// Any objects created using the handle must be destroyed before the final instance is dropped per the
    /// valid usage requirements (`VUID-vkDestroyInstance-instance-00629`).
    pub fn handle(&self) -> &ash::Instance {
        &self.0.instance
    }
}

/// A Vulkan physical device.
///
/// A physical device refers to a Vulkan implementation. A physical device has no associated resources and may
/// be used to create a logical device.
#[derive(Debug, Clone)]
pub struct PhysicalDevice {
    phd: vk::PhysicalDevice,
    info: PhdInfo,
    extensions: Vec<CString>,
    instance: Instance,
    span: tracing::Span,
}

impl PhysicalDevice {
    /// Enumerates over all physical devices available on the system, returning an iterator of [`PhysicalDevice`]
    pub fn enumerate(instance: &Instance) -> VkResult<impl Iterator<Item = PhysicalDevice>> {
        let _span = instance.0.span.enter();

        // Must clone instance or else the returned iterator has a lifetime over `&Instance`
        let instance = instance.clone();
        let devices = unsafe { instance.handle().enumerate_physical_devices() }?;
        let devices = devices
            .into_iter()
            // TODO: Warn if any physical devices have an error when getting device properties.
            .flat_map(move |phd| unsafe { PhysicalDevice::from_phd(&instance, phd) })
            .flatten();

        Ok(devices)
    }

    /// Returns the name of the device.
    pub fn name(&self) -> &str {
        &self.info.name
    }

    /// Returns the version of Vulkan supported by this device.
    ///
    /// Unlike the `api_version` property, which is the version reported by the device directly, this function
    /// returns the version the device can actually support, based on the instance’s, `api_version`.
    ///
    /// The Vulkan specification provides more information about the version requirements: <https://www.khronos.org/registry/vulkan/specs/1.3-extensions/html/vkspec.html#fundamentals-validusage-versions>
    pub fn api_version(&self) -> Version {
        self.info.api_version
    }

    /// Returns the device type.
    ///
    /// This may be used during device selection to choose a higher performance GPU.
    pub fn ty(&self) -> vk::PhysicalDeviceType {
        self.info.properties.device_type
    }

    /// Returns the Vulkan 1.0 physical device features.
    pub fn features(&self) -> vk::PhysicalDeviceFeatures {
        self.info.features
    }

    /// Returns the physical device properties.
    ///
    /// Some properties such as the device name can be obtained using other functions defined on
    /// [`PhysicalDevice`].
    pub fn properties(&self) -> vk::PhysicalDeviceProperties {
        self.info.properties
    }

    /// Returns the device's descriptor set properties.
    ///
    /// This also describes the maximum memory allocation size.
    pub fn properties_maintenance_3(&self) -> vk::PhysicalDeviceMaintenance3Properties {
        self.info.maintenance_3
    }

    /// Information about universally unique identifiers (UUIDs) that identify this device.
    pub fn id_properties(&self) -> vk::PhysicalDeviceIDProperties {
        self.info.id
    }

    /// Returns the physical device limits.
    pub fn limits(&self) -> vk::PhysicalDeviceLimits {
        self.info.properties.limits
    }

    /// Information about the Vulkan driver.
    ///
    /// This may return [`None`] for a few reasons:
    /// * The Vulkan implementation is not at least 1.2
    /// * If the Vulkan implementation is not at least Vulkan 1.2, the `VK_KHR_driver_properties` device
    ///   extension is not available.
    pub fn driver(&self) -> Option<&DriverInfo> {
        self.info.driver.as_ref()
    }

    /// Returns the major and minor numbers of the primary node which corresponds to this physical device's DRM
    /// device.
    #[cfg(feature = "backend_drm")]
    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn primary_node(&self) -> Result<Option<DrmNode>, UnsupportedProperty> {
        let properties_drm = self.info.get_drm_properties()?;
        let node = Some(properties_drm)
            .filter(|props| props.has_primary == vk::TRUE)
            .and_then(|props| {
                DrmNode::from_dev_id(libc::makedev(props.primary_major as _, props.primary_minor as _)).ok()
            });

        Ok(node)
    }

    /// Returns the major and minor numbers of the render node which corresponds to this physical device's DRM
    /// device.
    ///
    /// Note that not every device has a render node. If there is no render node (this function returns [`None`])
    /// then try to use the primary node.
    #[cfg(feature = "backend_drm")]
    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn render_node(&self) -> Result<Option<DrmNode>, UnsupportedProperty> {
        let properties_drm = self.info.get_drm_properties()?;
        let node = Some(properties_drm)
            .filter(|props| props.has_render == vk::TRUE)
            .and_then(|props| {
                DrmNode::from_dev_id(libc::makedev(props.render_major as _, props.render_minor as _)).ok()
            });

        Ok(node)
    }

    /// Get physical device properties.
    ///
    /// This function is equivalent to calling [`vkGetPhysicalDeviceProperties2`].
    ///
    /// # Safety
    ///
    /// - All valid usage requirements for [`vkGetPhysicalDeviceProperties2`] apply. Read the specification
    ///   for more information.
    ///
    /// [`vkGetPhysicalDeviceProperties2`]: https://www.khronos.org/registry/vulkan/specs/1.3-extensions/man/html/vkGetPhysicalDeviceProperties2.html
    pub unsafe fn get_properties(&self, props: &mut vk::PhysicalDeviceProperties2) {
        let instance = self.instance().handle();
        // SAFETY: The caller has garunteed all valid usage requirements for vkGetPhysicalDeviceProperties2
        // are satisfied.
        unsafe { instance.get_physical_device_properties2(self.handle(), props) }
    }

    /// Get physical device format properties.
    ///
    /// This function is equivalent to calling [`vkGetPhysicalDeviceFormatProperties2`].
    ///
    /// # Safety
    ///
    /// - All valid usage requirements for [`vkGetPhysicalDeviceFormatProperties2`] apply. Read the specification
    ///   for more information.
    ///
    /// [`vkGetPhysicalDeviceFormatProperties2`]: https://www.khronos.org/registry/vulkan/specs/1.3-extensions/man/html/vkGetPhysicalDeviceFormatProperties2.html
    pub unsafe fn get_format_properties(&self, format: vk::Format, props: &mut vk::FormatProperties2) {
        let instance = self.instance().handle();
        // SAFETY: The caller has garunteed all valid usage requirements for vkGetPhysicalDeviceFormatProperties2
        // are satisfied.
        unsafe { instance.get_physical_device_format_properties2(self.handle(), format, props) }
    }

    /// Returns properties for each supported DRM modifier for the specified format.
    ///
    /// Returns [`Err`] if the `VK_EXT_image_drm_format_modifier` extension is not supported.
    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn get_format_modifier_properties(
        &self,
        format: vk::Format,
    ) -> Result<Vec<vk::DrmFormatModifierPropertiesEXT>, UnsupportedProperty> {
        if !self.has_device_extension(vk::ExtImageDrmFormatModifierFn::name()) {
            const EXTENSIONS: &[&CStr] = &[vk::ExtImageDrmFormatModifierFn::name()];
            return Err(UnsupportedProperty::Extensions(EXTENSIONS));
        }

        // First get the number of modifiers the driver supports.
        let count = unsafe {
            let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
            let mut format_properties2 = vk::FormatProperties2::builder().push_next(&mut list);
            self.get_format_properties(format, &mut format_properties2);
            list.drm_format_modifier_count as usize
        };

        // Allocate the vector to receive the modifiers in.
        let mut data = Vec::with_capacity(count);

        unsafe {
            let mut list = vk::DrmFormatModifierPropertiesListEXT {
                // We cannot use the builder here because the Vec is currently empty, so we need to tell Vulkan
                // where to write out the modifier properties and tell it how large the Vec is.
                p_drm_format_modifier_properties: data.as_mut_ptr(),
                drm_format_modifier_count: count as u32,
                ..Default::default()
            };

            let mut format_properties2 = vk::FormatProperties2::builder().push_next(&mut list);
            self.get_format_properties(format, &mut format_properties2);
            // SAFETY: Vulkan just initialized the elements of the vector.
            data.set_len(list.drm_format_modifier_count as usize);
        }

        Ok(data)
    }

    /// Returns the device extensions supported by the physical device.
    pub fn device_extensions(&self) -> impl Iterator<Item = &CStr> {
        self.extensions.iter().map(CString::as_c_str)
    }

    /// Returns `true` if this device supports the specified device extension.
    pub fn has_device_extension(&self, extension: &CStr) -> bool {
        self.device_extensions().any(|name| name == extension)
    }

    /// Returns a handle to the underlying [`vk::PhysicalDevice`].
    ///
    /// The handle refers to a specific physical device advertised by the instance. This handle is only valid
    /// for the lifetime of the instance.
    pub fn handle(&self) -> vk::PhysicalDevice {
        self.phd
    }

    /// The instance which provided this physical device.
    pub fn instance(&self) -> &Instance {
        &self.instance
    }
}

impl PartialEq for PhysicalDevice {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Both the physical device handle and instance handle must be the same
        self.phd == other.phd && self.instance().handle().handle() == other.instance().handle().handle()
    }
}

// SAFETY: The internal pointers in the PhysicalDevice*Properties are always null and only copies of the
// PhysicalDevice*Properties types are returned.
unsafe impl Send for PhysicalDevice {}
unsafe impl Sync for PhysicalDevice {}

/// Information about the driver providing a [`PhysicalDevice`].
#[derive(Debug, Clone)]
pub struct DriverInfo {
    /// ID which identifies the driver.
    pub id: vk::DriverId,

    /// The name of the driver.
    pub name: String,

    /// Information describing the driver.
    ///
    /// This may include information such as the driver version.
    pub info: String,

    /// The Vulkan conformance test this driver is conformant against.
    pub conformance: vk::ConformanceVersion,
}

#[derive(Debug, Clone, Default)]
struct PhdInfo {
    api_version: Version,
    name: String,
    properties: vk::PhysicalDeviceProperties,
    features: vk::PhysicalDeviceFeatures,
    maintenance_3: vk::PhysicalDeviceMaintenance3Properties,
    id: vk::PhysicalDeviceIDProperties,
    properties_driver: Option<PhysicalDeviceDriverProperties>,
    /// Information about the DRM device which corresponds to this physical device.
    #[cfg_attr(not(feature = "backend_drm"), allow(dead_code))]
    properties_drm: Option<PhysicalDeviceDrmPropertiesEXT>,
    driver: Option<DriverInfo>,
}

fn get_env_or_max_version(max_version: Version) -> Version {
    // Consider max version overrides from env
    match env::var("SMITHAY_VK_VERSION") {
        Ok(version) => {
            let overriden_version = match &version[..] {
                "1.0" => {
                    warn!("Smithay does not support Vulkan 1.0, ignoring SMITHAY_VK_VERSION");
                    return max_version;
                }
                "1.1" => Some(Version::VERSION_1_1),
                "1.2" => Some(Version::VERSION_1_2),
                "1.3" => Some(Version::VERSION_1_3),
                _ => None,
            };

            // The env var can only lower the maximum version, not raise it.
            if let Some(overridden_version) = overriden_version {
                if overridden_version > max_version {
                    warn!(
                        "Ignoring SMITHAY_VK_VERSION since the requested max version is higher than the maximum of {}.{}",
                        max_version.major,
                        max_version.minor
                    );
                    max_version
                } else {
                    overridden_version
                }
            } else {
                warn!("SMITHAY_VK_VERSION was set to an unknown Vulkan version");
                max_version
            }
        }

        Err(VarError::NotUnicode(_)) => {
            warn!("Value of SMITHAY_VK_VERSION is not valid Unicode, ignoring.");

            max_version
        }

        Err(VarError::NotPresent) => max_version,
    }
}

unsafe extern "system" fn vulkan_debug_utils_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    span: *mut c_void,
) -> vk::Bool32 {
    let _ = std::panic::catch_unwind(|| {
        // Get the span from the user data pointer we gave to Vulkan.
        //
        // The span is allocated on the heap using a box, but we do not want to drop the span,
        // so read from the pointer.
        let _guard = unsafe { (span as *mut tracing::Span).as_ref() }.unwrap().enter();

        // VUID-VkDebugUtilsMessengerCallbackDataEXT-pMessage-parameter: Message must be valid UTF-8 with a null
        // terminator.
        let message = unsafe { CStr::from_ptr((*p_callback_data).p_message) }.to_string_lossy();
        // Message type is in full uppercase since we print the bitflag debug representation.
        let ty = format!("{:?}", message_type).to_lowercase();

        match message_severity {
            vk::DebugUtilsMessageSeverityFlagsEXT::VERBOSE => {
                trace!(ty, "{}", message)
            }
            vk::DebugUtilsMessageSeverityFlagsEXT::INFO => info!(ty, "{}", message),
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING => warn!(ty, "{}", message),
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR => error!(ty, "{}", message),
            _ => (),
        }
    });

    // Must always return false.
    vk::FALSE
}

#[cfg(test)]
mod tests {
    use super::{Instance, PhysicalDevice};

    fn is_send_sync<T: Send + Sync>() {}

    /// Test that both [`Instance`] and [`PhysicalDevice`] are Send and Sync.
    #[test]
    fn send_sync() {
        is_send_sync::<Instance>();
        is_send_sync::<PhysicalDevice>();
    }
}
