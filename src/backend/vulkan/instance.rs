//! Helper for VkInstance.
//!
//! To use Vulkan, you would first instantiate an [`Instance`]. An instance is effectively the Vulkan library
//! and provides some information about the environment. This includes the list of supported
//! [instance extensions](Instance::enumerate_extensions) and the list of available physical devices.
//!
//! An instance is constructed using an [`Instance::new`] or [`Instance::with_extensions`].

use std::{
    ffi::{CStr, CString},
    fmt,
    sync::Arc,
};

use ash::{ext, vk};
use scopeguard::ScopeGuard;
use tracing::{error, info, info_span, warn};

use super::{get_env_or_max_version, vulkan_debug_utils_callback, LoadError, Version, LIBRARY};

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
pub struct Instance(pub(super) Arc<InstanceInner>);

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
            unsafe {
                LIBRARY
                    .as_ref()
                    .or(Err(LoadError))?
                    .try_enumerate_instance_version()
            }
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
            const VALIDATION: &CStr = c"VK_LAYER_KHRONOS_validation";

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
            .any(|name| name.as_c_str() == ext::debug_utils::NAME);

        if has_debug_utils {
            enabled_extensions.push(ext::debug_utils::NAME);
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
        let mut app_info = vk::ApplicationInfo::default()
            .api_version(api_version.to_raw())
            // SAFETY: null terminated with no interior null bytes.
            .engine_name(c"Smithay")
            .engine_version(Version::SMITHAY.to_raw());

        if let Some(app_version) = app_version {
            app_info = app_info.application_version(app_version);
        }

        if let Some(app_name) = &app_name {
            app_info = app_info.application_name(app_name);
        }

        let library = LIBRARY.as_ref().map_err(|_| LoadError)?;
        let create_info = vk::InstanceCreateInfo::default()
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
            let debug_utils = ext::debug_utils::Instance::new(library, &instance);
            // Place the pointer to the span in a scopeguard to prevent a memory leak in case creating the
            // debug messenger fails.
            let span_ptr = scopeguard::guard(Box::into_raw(Box::new(span)), |ptr| unsafe {
                let _ = Box::from_raw(ptr);
            });

            let create_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
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

        let extensions = unsafe { library.enumerate_instance_extension_properties(None) }
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

pub struct InstanceInner {
    pub instance: ash::Instance,
    pub version: Version,
    pub debug_state: Option<DebugState>,
    pub span: tracing::Span,

    /// Enabled instance extensions.
    pub enabled_extensions: Vec<&'static CStr>,
}

// SAFETY: Destruction is externally synchronized (`InstanceInner` owns the
// `Instance`, and is held by a single thread when `Drop` is called).
unsafe impl Send for InstanceInner {}
unsafe impl Sync for InstanceInner {}

pub struct DebugState {
    pub debug_utils: ext::debug_utils::Instance,
    pub debug_messenger: vk::DebugUtilsMessengerEXT,
    pub span_ptr: *mut tracing::Span,
}

impl fmt::Debug for InstanceInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceInner")
            .field("instance", &self.instance.handle())
            .finish_non_exhaustive()
    }
}

impl Drop for InstanceInner {
    fn drop(&mut self) {
        let span = if let Some(debug) = &self.debug_state {
            unsafe {
                debug
                    .debug_utils
                    .destroy_debug_utils_messenger(debug.debug_messenger, None);
            }
            Some(unsafe { Box::from_raw(debug.span_ptr) })
        } else {
            None
        };

        // Users of `Instance` are responsible for compliance with `VUID-vkDestroyInstance-instance-00629`.

        // SAFETY (Host Synchronization): InstanceInner is always stored in an Arc, therefore destruction is
        // synchronized (since the inner value of an Arc is always dropped on a single thread).
        unsafe { self.instance.destroy_instance(None) };

        // Now that the instance has been destroyed, we can destroy the span.
        drop(span);
    }
}

impl super::Instance {
    pub(super) fn enumerate_layers() -> Result<impl Iterator<Item = CString>, LoadError> {
        let library = LIBRARY.as_ref().or(Err(LoadError))?;

        let layers = unsafe { library.enumerate_instance_layer_properties() }
            .or(Err(LoadError))?
            .into_iter()
            .map(|properties| {
                // SAFETY: Vulkan guarantees the string is null terminated.
                unsafe { CStr::from_ptr(&properties.layer_name as *const _) }.to_owned()
            });

        Ok(layers)
    }
}
