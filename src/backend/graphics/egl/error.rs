//! EGL error types

error_chain! {
    errors {
        #[doc = "The requested OpenGL version is not supported"]
        OpenGlVersionNotSupported(version: (u8, u8)) {
            description("The requested OpenGL version is not supported."),
            display("The requested OpenGL version {:?} is not supported.", version),
        }

        #[doc = "The EGL implementation does not support creating OpenGL ES contexts"]
        OpenGlesNotSupported {
            description("The EGL implementation does not support creating OpenGL ES contexts")
        }

        #[doc = "No available pixel format matched the criteria"]
        NoAvailablePixelFormat {
            description("No available pixel format matched the criteria.")
        }

        #[doc = "Backend does not match the context type"]
        NonMatchingBackend(expected: &'static str) {
            description("The expected backend did not match the runtime."),
            display("The expected backend '{:?}' does not match the runtime.", expected),
        }

        #[doc = "EGL was unable to optain a valid EGL Display"]
        DisplayNotSupported {
            description("EGL was unable to optain a valid EGL Display")
        }

        #[doc = "eglInitialize returned an error"]
        InitFailed {
            description("Failed to initialize EGL")
        }

        #[doc = "Failed to configure the EGL context"]
        ConfigFailed {
            description("Failed to configure the EGL context")
        }

        #[doc = "Context creation failed as one or more requirements could not be met. Try removing some gl attributes or pixel format requirements"]
        CreationFailed {
            description("Context creation failed as one or more requirements could not be met. Try removing some gl attributes or pixel format requirements")
        }

        #[doc = "eglCreateWindowSurface failed"]
        SurfaceCreationFailed {
            description("Failed to create a new EGLSurface")
        }

        #[doc = "The required EGL extension is not supported by the underlying EGL implementation"]
        EglExtensionNotSupported(extensions: &'static [&'static str]) {
            description("The required EGL extension is not supported by the underlying EGL implementation"),
            display("None of the following EGL extensions is supported by the underlying EGL implementation,
                     at least one is required: {:?}", extensions)
        }

        #[doc = "Only one EGLDisplay may be bound to a given WlDisplay at any time"]
        OtherEGLDisplayAlreadyBound {
            description("Only one EGLDisplay may be bound to a given WlDisplay at any time")
        }

        #[doc = "No EGLDisplay is currently bound to this WlDisplay"]
        NoEGLDisplayBound {
            description("No EGLDisplay is currently bound to this WlDisplay")
        }

        #[doc = "Index of plane is out of bounds for EGLImages"]
        PlaneIndexOutOfBounds {
            description("Index of plane is out of bounds for EGLImages")
        }

        #[doc = "Failed to create EGLImages from the buffer"]
        EGLImageCreationFailed {
            description("Failed to create EGLImages from the buffer")
        }

        #[doc = "The reason of failure could not be determined"]
        Unknown(err_no: u32)
    }
}
