#[cfg(any(feature = "backend_egl", feature = "renderer_gl"))]
extern crate gl_generator;

#[cfg(any(feature = "backend_egl", feature = "renderer_gl"))]
use gl_generator::{Api, Fallbacks, Profile, Registry};
use std::{env, fs::File, path::PathBuf};

#[cfg(any(feature = "backend_egl", feature = "renderer_gl"))]
fn main() {
    let dest = PathBuf::from(&env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=build.rs");

    if env::var_os("CARGO_FEATURE_BACKEND_EGL").is_some() {
        let mut file = File::create(&dest.join("egl_bindings.rs")).unwrap();
        Registry::new(
            Api::Egl,
            (1, 5),
            Profile::Core,
            Fallbacks::All,
            [
                "EGL_KHR_create_context",
                "EGL_EXT_create_context_robustness",
                "EGL_KHR_create_context_no_error",
                "EGL_KHR_platform_x11",
                "EGL_KHR_platform_android",
                "EGL_KHR_platform_wayland",
                "EGL_KHR_platform_gbm",
                "EGL_EXT_platform_base",
                "EGL_EXT_platform_x11",
                "EGL_MESA_platform_gbm",
                "EGL_EXT_platform_wayland",
                "EGL_EXT_platform_device",
                "EGL_KHR_image_base",
                "EGL_EXT_output_base",
                "EGL_EXT_output_drm",
                "EGL_EXT_device_drm",
                "EGL_EXT_device_enumeration",
                "EGL_EXT_device_query",
                "EGL_KHR_stream",
                "EGL_KHR_stream_producer_eglsurface",
                "EGL_EXT_stream_consumer_egloutput",
                "EGL_KHR_stream_fifo",
                "EGL_NV_output_drm_flip_event",
                "EGL_NV_stream_attrib",
            ],
        )
        .write_bindings(gl_generator::GlobalGenerator, &mut file)
        .unwrap();
    }

    if env::var_os("CARGO_FEATURE_RENDERER_GL").is_some() {
        let mut file = File::create(&dest.join("gl_bindings.rs")).unwrap();
        Registry::new(
            Api::Gles2,
            (3, 2),
            Profile::Compatibility,
            Fallbacks::None,
            ["GL_OES_EGL_image"],
        )
        .write_bindings(gl_generator::StructGenerator, &mut file)
        .unwrap();
    }
}

#[cfg(not(any(feature = "backend_egl", feature = "renderer_gl")))]
fn main() {}
