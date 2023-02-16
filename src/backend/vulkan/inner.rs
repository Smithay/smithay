use std::{
    ffi::{CStr, CString},
    fmt,
};

use ash::{extensions::ext::DebugUtils, vk};

use super::{version::Version, LoadError, LIBRARY};

pub struct InstanceInner {
    pub instance: ash::Instance,
    pub version: Version,
    pub debug_state: Option<DebugState>,

    /// Enabled instance extensions.
    pub enabled_extensions: Vec<&'static CStr>,
}

pub struct DebugState {
    pub debug_utils: DebugUtils,
    pub debug_messenger: vk::DebugUtilsMessengerEXT,
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
        if let Some(debug) = &self.debug_state {
            unsafe {
                debug
                    .debug_utils
                    .destroy_debug_utils_messenger(debug.debug_messenger, None);
            }
        }

        // Users of `Instance` are responsible for compliance with `VUID-vkDestroyInstance-instance-00629`.

        // SAFETY (Host Synchronization): InstanceInner is always stored in an Arc, therefore destruction is
        // synchronized (since the inner value of an Arc is always dropped on a single thread).
        unsafe { self.instance.destroy_instance(None) };
    }
}

impl super::Instance {
    pub(super) fn enumerate_layers() -> Result<impl Iterator<Item = CString>, LoadError> {
        let library = LIBRARY.as_ref().or(Err(LoadError))?;

        let layers = library
            .enumerate_instance_layer_properties()
            .or(Err(LoadError))?
            .into_iter()
            .map(|properties| {
                // SAFETY: Vulkan guarantees the string is null terminated.
                unsafe { CStr::from_ptr(&properties.layer_name as *const _) }.to_owned()
            });

        Ok(layers)
    }
}
