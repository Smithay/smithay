use std::{ffi::CStr, os::raw::c_void, path::PathBuf, ptr};

use super::{
    ffi::{self, egl::types::EGLDeviceEXT},
    wrap_egl_call_bool, wrap_egl_call_ptr, EGLDisplay, EGLError, Error,
};

#[cfg(feature = "backend_drm")]
use crate::backend::drm::{DrmNode, NodeType};

/// safe EGLDevice wrapper
#[derive(Debug, Clone)]
pub struct EGLDevice {
    pub(super) inner: EGLDeviceEXT,
    device_extensions: Vec<String>,
}

unsafe impl Send for EGLDevice {}

impl EGLDevice {
    /// Returns an iterator which enumerates over the available [`EGLDevices`](EGLDevice) on the system.
    ///
    /// This function will return an error if the following extensions are not available:
    /// - [`EGL_EXT_device_base`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_base.txt)
    /// - [`EGL_EXT_device_enumeration`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_enumeration.txt)
    /// - [`EGL_EXT_device_query`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_query.txt)
    pub fn enumerate() -> Result<impl Iterator<Item = EGLDevice>, Error> {
        // Check the required extensions are present:
        let extensions = ffi::make_sure_egl_is_loaded()?;

        if !extensions.iter().any(|s| s == "EGL_EXT_device_base") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_base"]));
        }

        if !extensions.iter().any(|s| s == "EGL_EXT_device_enumeration") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_enumeration"]));
        }

        if !extensions.iter().any(|s| s == "EGL_EXT_device_query") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_query"]));
        }

        // Yes, this is marked as `mut` even though the value is never mutated. EGL expects a mutable pointer
        // for num_devices and will not modify the value if we are asking for pointers to some EGLDeviceEXT.
        let mut device_amount = 0;
        wrap_egl_call_bool(|| unsafe {
            // Passing 0 for max devices and a null-pointer for devices is safe because we indicate we only
            // want the number of devices.
            ffi::egl::QueryDevicesEXT(0, ptr::null_mut(), &mut device_amount)
        })
        .map_err(Error::QueryDevices)?;

        let mut devices = Vec::with_capacity(device_amount as usize);

        wrap_egl_call_bool(|| unsafe {
            // SAFETY:
            // - Vector used as pointer is correct size.
            // - Device amount will accommodate all available devices because we have checked the size earlier.
            ffi::egl::QueryDevicesEXT(device_amount, devices.as_mut_ptr(), &mut device_amount)
        })
        .map_err(Error::QueryDevices)?;

        // Set the length of the vec so that rust does not think it is still empty.

        // SAFETY:
        // 1) the vector is pre-allocated to the same size as the amount of returned devices.
        // 2) EGL has initialized every value in the vector.
        unsafe { devices.set_len(device_amount as usize) };

        Ok(devices
            .into_iter()
            .map(|device| {
                // SAFETY: We have queried that the extensions are valid and the device pointer is valid.
                let device_extensions = unsafe { device_extensions(device) }?;
                Ok(EGLDevice {
                    inner: device,
                    device_extensions,
                })
            })
            .collect::<Result<Vec<_>, EGLError>>()
            .map_err(Error::QueryDevices)?
            .into_iter())
    }

    /// Returns the [`EGLDevices`](EGLDevice) related to the given `EGLDisplay`.
    ///
    /// This function will return an error if the following extensions are not available:
    /// - [`EGL_EXT_device_base`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_base.txt)
    /// - [`EGL_EXT_device_query`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_query.txt)
    pub fn device_for_display(display: &EGLDisplay) -> Result<EGLDevice, Error> {
        // Check the required extensions are present:
        let extensions = ffi::make_sure_egl_is_loaded()?;

        if !extensions.iter().any(|s| s == "EGL_EXT_device_base") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_base"]));
        }

        if !extensions.iter().any(|s| s == "EGL_EXT_device_query") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_query"]));
        }

        let mut device: ffi::egl::types::EGLAttrib = 0;
        if unsafe {
            ffi::egl::QueryDisplayAttribEXT(
                display.get_display_handle().handle,
                ffi::egl::DEVICE_EXT as i32,
                &mut device as *mut _,
            )
        } != ffi::egl::TRUE
        {
            return Err(Error::DisplayNotSupported);
        }

        let device = device as EGLDeviceEXT;

        // Per the EGL specification:
        //
        // > Functions with a return type of EGLDeviceEXT will return this value on failure: EGL_NO_DEVICE_EXT
        if device == ffi::egl::NO_DEVICE_EXT {
            return Err(Error::DisplayNotSupported);
        }

        // SAFETY: We have queried that the extensions are valid and the device pointer is valid.
        let device_extensions = unsafe { device_extensions(device) }.map_err(Error::QueryDevices)?;
        Ok(EGLDevice {
            inner: device,
            device_extensions,
        })
    }

    /// Returns a list of extensions the device supports.
    pub fn extensions(&self) -> Vec<String> {
        self.device_extensions.clone()
    }

    /// Returns the path to the drm node of this EGLDevice.
    ///
    /// This function will return an error if the following extensions are not available:
    /// - [`EGL_EXT_device_drm`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_drm.txt)
    pub fn drm_device_path(&self) -> Result<PathBuf, Error> {
        if !self.extensions().contains(&"EGL_EXT_device_drm".to_owned()) {
            Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_drm"]))
        } else {
            let raw_path = wrap_egl_call_ptr(|| unsafe {
                ffi::egl::QueryDeviceStringEXT(
                    self.inner,
                    ffi::egl::DRM_DEVICE_FILE_EXT as ffi::egl::types::EGLint,
                )
            })
            .map_err(Error::QueryDeviceProperty)?;
            if raw_path.is_null() {
                return Err(Error::EmptyDeviceProperty);
            }

            // This is safe because of the following:
            // 1) The string returned by `eglQueryDeviceStringEXT` is string which will exist as long
            //    as the EGLDisplay is valid. Since the pointer is only used in this function, the
            //    lifetime of the pointer will fulfil Rust's CStr requirements on lifetime.
            // 2) The string returned by EGL is null terminated.
            let device_path = unsafe { CStr::from_ptr(raw_path) }
                .to_str()
                // EGL ensures the string is valid UTF-8
                .expect("Non-UTF8 device path name");

            Ok(PathBuf::from(device_path))
        }
    }

    /// Returns the path to the render node of this EGLDevice.
    ///
    /// This function will return an error if the following extensions are not available:
    /// - [`EGL_EXT_device_drm_render_node`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_drm_render_node.txt)
    pub fn render_device_path(&self) -> Result<PathBuf, Error> {
        if !self
            .extensions()
            .contains(&"EGL_EXT_device_drm_render_node".to_owned())
        {
            Err(Error::EglExtensionNotSupported(&[
                "EGL_EXT_device_drm_render_node",
            ]))
        } else {
            let raw_path = wrap_egl_call_ptr(|| unsafe {
                ffi::egl::QueryDeviceStringEXT(
                    self.inner,
                    ffi::egl::DRM_RENDER_NODE_FILE_EXT as ffi::egl::types::EGLint,
                )
            })
            .map_err(Error::QueryDeviceProperty)?;
            if raw_path.is_null() {
                return Err(Error::EmptyDeviceProperty);
            }

            // This is safe because of the following:
            // 1) The string returned by `eglQueryDeviceStringEXT` is string which will exist as long
            //    as the EGLDisplay is valid. Since the pointer is only used in this function, the
            //    lifetime of the pointer will fulfil Rust's CStr requirements on lifetime.
            // 2) The string returned by EGL is null terminated.
            let device_path = unsafe { CStr::from_ptr(raw_path) }
                .to_str()
                // EGL ensures the string is valid UTF-8
                .expect("Non-UTF8 device path name");

            Ok(PathBuf::from(device_path))
        }
    }

    /// Returns the drm node beloging to this device.
    /// Tries to optain a render_node first through `EGL_EXT_device_drm_render_node`
    /// (see also [`EGLDevice::render_device_path`]) and then falls back to
    /// get a render_node from `EGL_EXT_device_drm` (see also [`EGLDevice::drm_device_path`]).
    /// If both fail to produce a render node, whichever device returned by
    /// `EGL_EXT_device_drm` is returned.
    #[cfg(feature = "backend_drm")]
    pub fn try_get_render_node(&self) -> Result<Option<DrmNode>, Error> {
        // first lets try to get a render_node directly
        match self
            .render_device_path()
            .ok()
            .and_then(|path| DrmNode::from_path(path).ok())
        {
            Some(node) => Ok(Some(node)),
            // else we take a drm_path
            None => {
                let path = self.drm_device_path()?;
                let node = DrmNode::from_path(path).ok();
                // and try to convert it to a render_node
                Ok(node.map(|node| {
                    node.node_with_type(NodeType::Render)
                        .and_then(Result::ok)
                        // and otherwise go with whatever we got initially
                        .unwrap_or(node)
                }))
            }
        }
    }

    /// Queries if device is not backed by any actual device node and simply renders into client memory.
    ///
    /// Note: This simply tests presence of the [`EGL_MESA_device_software`](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/_extra/specs/EGL_MESA_device_software.txt) extension.
    pub fn is_software(&self) -> bool {
        // Note: EGL_MESA_device_software requires EGL_EXT_device_query which we already test for initializing the device,
        // so there is no need to re-test it here again.
        self.extensions().contains(&"EGL_MESA_device_software".to_owned())
    }

    /// Returns the pointer to the raw [`EGLDevice`].
    ///
    /// The pointer will become invalid, when this struct is destroyed.
    pub fn get_device_handle(&self) -> *const c_void {
        self.inner
    }
}

/// Returns all device extensions a device supports.
///
/// # Safety
///
/// - The `device` must be a valid pointer to an `EGLDeviceEXT`.
/// - The following extensions must be supported by the display which provides the device:
///     - [`EGL_EXT_device_base`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_base.txt)
///     - [`EGL_EXT_device_query`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_query.txt)
unsafe fn device_extensions(device: EGLDeviceEXT) -> Result<Vec<String>, EGLError> {
    let raw_extensions = wrap_egl_call_ptr(|| {
        ffi::egl::QueryDeviceStringEXT(device, ffi::egl::EXTENSIONS as ffi::egl::types::EGLint)
    })?;

    // SAFETY:
    // 1) The string returned by `eglQueryDeviceStringEXT` is string which will exist as long
    //    as the EGLDisplay is valid. Safety requirements for the function ensure this.
    // 2) The string returned by EGL is null terminated.
    let c_extensions = CStr::from_ptr(raw_extensions);

    Ok(c_extensions
        .to_str()
        // EGL ensures the string is valid UTF-8
        .expect("Non-UTF8 device extension name")
        // Each extension is space separated (0x20) in the pointer, so strlen cannot return an improper length.
        .split_whitespace()
        // Take an owned copy so we do not point to garbage if EGL somehow vanishes.
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>())
}
