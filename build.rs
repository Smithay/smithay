#[cfg(any(feature = "backend_egl", feature = "renderer_gl"))]
fn gl_generate() {
    use gl_generator::{Api, Fallbacks, Profile, Registry};
    use std::{env, fs::File, path::PathBuf};

    let dest = PathBuf::from(&env::var("OUT_DIR").unwrap());

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
                "EGL_KHR_no_config_context",
                "EGL_EXT_device_base",
                "EGL_EXT_device_enumeration",
                "EGL_EXT_device_query",
                "EGL_EXT_device_drm",
                "EGL_KHR_stream",
                "EGL_KHR_stream_producer_eglsurface",
                "EGL_EXT_platform_base",
                "EGL_KHR_platform_x11",
                "EGL_EXT_platform_x11",
                "EGL_KHR_platform_wayland",
                "EGL_EXT_platform_wayland",
                "EGL_KHR_platform_gbm",
                "EGL_MESA_platform_gbm",
                "EGL_EXT_platform_device",
                "EGL_WL_bind_wayland_display",
                "EGL_KHR_image_base",
                "EGL_EXT_image_dma_buf_import",
                "EGL_EXT_image_dma_buf_import_modifiers",
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
            [
                "GL_OES_EGL_image",
                "GL_OES_EGL_image_external",
                "GL_EXT_texture_format_BGRA8888",
                "GL_EXT_unpack_subimage",
            ],
        )
        .write_bindings(gl_generator::StructGenerator, &mut file)
        .unwrap();
    }
}

#[cfg(feature = "backend_session_logind")]
fn find_logind() {
    // We should allow only dynamic linkage due to libsystemd and libelogind LICENSE.

    #[cfg(feature = "backend_session_elogind")]
    {
        if pkg_config::Config::new()
            .statik(false)
            .probe("libelogind")
            .is_err()
        {
            println!("cargo:warning=Could not find `libelogind.so`.");
            println!("cargo:warning=If your system is systemd-based, you should only enable the `backend_session_logind` feature, not `backend_session_elogind`.");
            std::process::exit(1);
        }
    }

    #[cfg(not(feature = "backend_session_elogind"))]
    {
        if pkg_config::Config::new()
            .statik(false)
            .probe("libsystemd")
            .is_err()
        {
            println!("cargo:warning=Could not find `libsystemd.so`.");
            println!("cargo:warning=If your system uses elogind, please enable the `backend_session_elogind` feature.");
            println!("cargo:warning=Otherwise, you may need to disable the `backend_session_logind` feature as your system does not support it.");
            std::process::exit(1);
        }
    }
}

fn main() {
    #[cfg(any(feature = "backend_egl", feature = "renderer_gl"))]
    gl_generate();

    #[cfg(feature = "backend_session_logind")]
    find_logind();
}
