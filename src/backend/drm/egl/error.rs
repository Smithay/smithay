//!
//! Errors thrown by the [`EglDevice`](::backend::drm::egl::EglDevice)
//! and [`EglSurface`](::backend::drm::egl::EglSurface).
//!

use backend::egl::error as egl;

error_chain! {
    errors {
        #[doc = "Underlying backend failed"]
        UnderlyingBackendError {
            description("The underlying backend reported an error"),
            display("The underlying backend reported an error"),
        }
    }

    links {
        EGL(egl::Error, egl::ErrorKind) #[doc = "EGL error"];
    }
}
