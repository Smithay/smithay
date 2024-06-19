use std::{
    ffi::{CStr, CString},
    fmt,
};

use ash::{ext, vk};

use super::{version::Version, LoadError, LIBRARY};

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
