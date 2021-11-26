#![allow(missing_docs)]

use super::Error;
use nix::libc::{c_long, c_uint, c_void};

pub type khronos_utime_nanoseconds_t = khronos_uint64_t;
pub type khronos_uint64_t = u64;
pub type khronos_ssize_t = c_long;
pub type EGLint = i32;
pub type EGLchar = char;
pub type EGLLabelKHR = *const c_void;
pub type EGLNativeDisplayType = NativeDisplayType;
pub type EGLNativePixmapType = NativePixmapType;
pub type EGLNativeWindowType = NativeWindowType;
pub type NativeDisplayType = *const c_void;
pub type NativePixmapType = *const c_void;
pub type NativeWindowType = *const c_void;

extern "system" fn egl_debug_log(
    severity: egl::types::EGLenum,
    command: *const EGLchar,
    _id: EGLint,
    _thread: EGLLabelKHR,
    _obj: EGLLabelKHR,
    message: *const EGLchar,
) {
    let _ = std::panic::catch_unwind(move || unsafe {
        let msg = std::ffi::CStr::from_ptr(message as *const _);
        let message_utf8 = msg.to_string_lossy();
        let command_utf8 = if !command.is_null() {
            let cmd = std::ffi::CStr::from_ptr(command as *const _);
            cmd.to_string_lossy()
        } else {
            std::borrow::Cow::Borrowed("")
        };
        let logger = crate::slog_or_fallback(None).new(slog::o!("backend" => "egl"));
        match severity {
            egl::DEBUG_MSG_CRITICAL_KHR | egl::DEBUG_MSG_ERROR_KHR => {
                slog::error!(logger, "[EGL] {}: {}", command_utf8, message_utf8)
            }
            egl::DEBUG_MSG_WARN_KHR => slog::warn!(logger, "[EGL] {}: {}", command_utf8, message_utf8),
            egl::DEBUG_MSG_INFO_KHR => slog::info!(logger, "[EGL] {}: {}", command_utf8, message_utf8),
            _ => slog::debug!(logger, "[EGL] {}: {}", command_utf8, message_utf8),
        };
    });
}

/// Loads libEGL symbols, if not loaded already.
/// This normally happens automatically during [`EGLDisplay`](super::EGLDisplay) initialization.
pub fn make_sure_egl_is_loaded() -> Result<Vec<String>, Error> {
    use std::{
        ffi::{CStr, CString},
        ptr,
    };

    fn constrain<F>(f: F) -> F
    where
        F: for<'a> Fn(&'a str) -> *const ::std::os::raw::c_void,
    {
        f
    }
    let proc_address = constrain(|sym| unsafe { super::get_proc_address(sym) });

    egl::LOAD.call_once(|| unsafe {
        egl::load_with(|sym| {
            let name = CString::new(sym).unwrap();
            let symbol = egl::LIB.get::<*mut c_void>(name.as_bytes());
            match symbol {
                Ok(x) => *x as *const _,
                Err(_) => ptr::null(),
            }
        });
        egl::load_with(&proc_address);
        egl::BindWaylandDisplayWL::load_with(&proc_address);
        egl::UnbindWaylandDisplayWL::load_with(&proc_address);
        egl::QueryWaylandBufferWL::load_with(&proc_address);
        egl::DebugMessageControlKHR::load_with(&proc_address);
    });

    let extensions = unsafe {
        let p = super::wrap_egl_call(|| egl::QueryString(egl::NO_DISPLAY, egl::EXTENSIONS as i32))
            .map_err(Error::InitFailed)?; //TODO EGL_EXT_client_extensions not supported

        // this possibility is available only with EGL 1.5 or EGL_EXT_platform_base, otherwise
        // `eglQueryString` returns an error
        if p.is_null() {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_platform_base"]));
        } else {
            let p = CStr::from_ptr(p);
            let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
            list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
        }
    };

    egl::DEBUG.call_once(|| unsafe {
        if extensions.iter().any(|ext| ext == "EGL_KHR_debug") {
            let debug_attribs = [
                egl::DEBUG_MSG_CRITICAL_KHR as isize,
                egl::TRUE as isize,
                egl::DEBUG_MSG_ERROR_KHR as isize,
                egl::TRUE as isize,
                egl::DEBUG_MSG_WARN_KHR as isize,
                egl::TRUE as isize,
                egl::DEBUG_MSG_INFO_KHR as isize,
                egl::TRUE as isize,
                egl::NONE as isize,
            ];
            // we do not check for success, because there is not much we can do otherwise.
            egl::DebugMessageControlKHR(Some(egl_debug_log), debug_attribs.as_ptr());
        }
    });

    Ok(extensions)
}

/// Module containing raw egl function bindings
#[allow(clippy::all, missing_debug_implementations)]
pub mod egl {
    use super::*;
    use libloading::Library;
    use std::sync::Once;

    lazy_static::lazy_static! {
        pub static ref LIB: Library = unsafe { Library::new("libEGL.so.1") }.expect("Failed to load LibEGL");
    }

    pub static LOAD: Once = Once::new();
    pub static DEBUG: Once = Once::new();

    include!(concat!(env!("OUT_DIR"), "/egl_bindings.rs"));

    pub const RESOURCE_BUSY_EXT: u32 = 0x3353;

    type EGLDEBUGPROCKHR = Option<
        extern "system" fn(
            _error: egl::types::EGLenum,
            command: *const EGLchar,
            _id: EGLint,
            _thread: EGLLabelKHR,
            _obj: EGLLabelKHR,
            message: *const EGLchar,
        ),
    >;
    #[allow(dead_code, non_upper_case_globals)]
    pub const DEBUG_MSG_CRITICAL_KHR: types::EGLenum = 0x33B9;
    #[allow(dead_code, non_upper_case_globals)]
    pub const DEBUG_MSG_ERROR_KHR: types::EGLenum = 0x33BA;
    #[allow(dead_code, non_upper_case_globals)]
    pub const DEBUG_MSG_INFO_KHR: types::EGLenum = 0x33BC;
    #[allow(dead_code, non_upper_case_globals)]
    pub const DEBUG_MSG_WARN_KHR: types::EGLenum = 0x33BB;

    #[allow(non_snake_case, unused_variables, dead_code)]
    #[inline]
    pub unsafe fn DebugMessageControlKHR(
        callback: EGLDEBUGPROCKHR,
        attrib_list: *const types::EGLAttrib,
    ) -> types::EGLint {
        __gl_imports::mem::transmute::<
            _,
            extern "system" fn(EGLDEBUGPROCKHR, *const types::EGLAttrib) -> types::EGLint,
        >(wayland_storage::DebugMessageControlKHR.f)(callback, attrib_list)
    }
    /*
     * `gl_generator` cannot generate bindings for the `EGL_WL_bind_wayland_display` extension.
     *  Lets do it ourselves...
     */

    #[allow(non_snake_case, unused_variables, dead_code)]
    #[inline]
    pub unsafe fn BindWaylandDisplayWL(
        dpy: types::EGLDisplay,
        display: *mut __gl_imports::raw::c_void,
    ) -> types::EGLBoolean {
        __gl_imports::mem::transmute::<
            _,
            extern "system" fn(types::EGLDisplay, *mut __gl_imports::raw::c_void) -> types::EGLBoolean,
        >(wayland_storage::BindWaylandDisplayWL.f)(dpy, display)
    }

    #[allow(non_snake_case, unused_variables, dead_code)]
    #[inline]
    pub unsafe fn UnbindWaylandDisplayWL(
        dpy: types::EGLDisplay,
        display: *mut __gl_imports::raw::c_void,
    ) -> types::EGLBoolean {
        __gl_imports::mem::transmute::<
            _,
            extern "system" fn(types::EGLDisplay, *mut __gl_imports::raw::c_void) -> types::EGLBoolean,
        >(wayland_storage::UnbindWaylandDisplayWL.f)(dpy, display)
    }

    #[allow(non_snake_case, unused_variables, dead_code)]
    #[inline]
    pub unsafe fn QueryWaylandBufferWL(
        dpy: types::EGLDisplay,
        buffer: *mut __gl_imports::raw::c_void,
        attribute: types::EGLint,
        value: *mut types::EGLint,
    ) -> types::EGLBoolean {
        __gl_imports::mem::transmute::<
            _,
            extern "system" fn(
                types::EGLDisplay,
                *mut __gl_imports::raw::c_void,
                types::EGLint,
                *mut types::EGLint,
            ) -> types::EGLBoolean,
        >(wayland_storage::QueryWaylandBufferWL.f)(dpy, buffer, attribute, value)
    }

    mod wayland_storage {
        use super::{FnPtr, __gl_imports::raw};
        pub static mut BindWaylandDisplayWL: FnPtr = FnPtr {
            f: super::missing_fn_panic as *const raw::c_void,
            is_loaded: false,
        };
        pub static mut UnbindWaylandDisplayWL: FnPtr = FnPtr {
            f: super::missing_fn_panic as *const raw::c_void,
            is_loaded: false,
        };
        pub static mut QueryWaylandBufferWL: FnPtr = FnPtr {
            f: super::missing_fn_panic as *const raw::c_void,
            is_loaded: false,
        };
        pub static mut DebugMessageControlKHR: FnPtr = FnPtr {
            f: super::missing_fn_panic as *const raw::c_void,
            is_loaded: false,
        };
    }

    #[allow(non_snake_case)]
    pub mod DebugMessageControlKHR {
        use super::FnPtr;
        use super::__gl_imports::raw;
        use super::{metaloadfn, wayland_storage};

        #[inline]
        #[allow(dead_code)]
        pub fn is_loaded() -> bool {
            unsafe { wayland_storage::DebugMessageControlKHR.is_loaded }
        }

        #[allow(dead_code)]
        pub fn load_with<F>(mut loadfn: F)
        where
            F: FnMut(&'static str) -> *const raw::c_void,
        {
            unsafe {
                wayland_storage::DebugMessageControlKHR =
                    FnPtr::new(metaloadfn(&mut loadfn, "eglDebugMessageControlKHR", &[]))
            }
        }
    }

    #[allow(non_snake_case)]
    pub mod BindWaylandDisplayWL {
        use super::{FnPtr, __gl_imports::raw, metaloadfn, wayland_storage};

        #[inline]
        #[allow(dead_code)]
        pub fn is_loaded() -> bool {
            unsafe { wayland_storage::BindWaylandDisplayWL.is_loaded }
        }

        #[allow(dead_code)]
        pub fn load_with<F>(mut loadfn: F)
        where
            F: FnMut(&str) -> *const raw::c_void,
        {
            unsafe {
                wayland_storage::BindWaylandDisplayWL =
                    FnPtr::new(metaloadfn(&mut loadfn, "eglBindWaylandDisplayWL", &[]))
            }
        }
    }

    #[allow(non_snake_case)]
    pub mod UnbindWaylandDisplayWL {
        use super::{FnPtr, __gl_imports::raw, metaloadfn, wayland_storage};

        #[inline]
        #[allow(dead_code)]
        pub fn is_loaded() -> bool {
            unsafe { wayland_storage::UnbindWaylandDisplayWL.is_loaded }
        }

        #[allow(dead_code)]
        pub fn load_with<F>(mut loadfn: F)
        where
            F: FnMut(&str) -> *const raw::c_void,
        {
            unsafe {
                wayland_storage::UnbindWaylandDisplayWL =
                    FnPtr::new(metaloadfn(&mut loadfn, "eglUnbindWaylandDisplayWL", &[]))
            }
        }
    }

    #[allow(non_snake_case)]
    pub mod QueryWaylandBufferWL {
        use super::{FnPtr, __gl_imports::raw, metaloadfn, wayland_storage};

        #[inline]
        #[allow(dead_code)]
        pub fn is_loaded() -> bool {
            unsafe { wayland_storage::QueryWaylandBufferWL.is_loaded }
        }

        #[allow(dead_code)]
        pub fn load_with<F>(mut loadfn: F)
        where
            F: FnMut(&str) -> *const raw::c_void,
        {
            unsafe {
                wayland_storage::QueryWaylandBufferWL =
                    FnPtr::new(metaloadfn(&mut loadfn, "eglQueryWaylandBufferWL", &[]))
            }
        }
    }

    // Accepted as <target> in eglCreateImageKHR
    pub const WAYLAND_BUFFER_WL: c_uint = 0x31D5;
    // Accepted in the <attrib_list> parameter of eglCreateImageKHR:
    pub const WAYLAND_PLANE_WL: c_uint = 0x31D6;
    // Possible values for EGL_TEXTURE_FORMAT:
    pub const TEXTURE_Y_U_V_WL: i32 = 0x31D7;
    pub const TEXTURE_Y_UV_WL: i32 = 0x31D8;
    pub const TEXTURE_Y_XUXV_WL: i32 = 0x31D9;
    pub const TEXTURE_EXTERNAL_WL: i32 = 0x31DA;
    // Accepted in the <attribute> parameter of eglQueryWaylandBufferWL:
    pub const EGL_TEXTURE_FORMAT: i32 = 0x3080;
    pub const WAYLAND_Y_INVERTED_WL: i32 = 0x31DB;
}
