//!
//! Errors thrown by the `DrmDevice` and `DrmBackend`
//!

error_chain! {
    errors {
        #[doc = "Creation of gbm device failed"]
        InitFailed {
            description("Creation of gbm device failed"),
            display("Creation of gbm device failed"),
        }

        #[doc = "Creation of gbm surface failed"]
        SurfaceCreationFailed {
            description("Creation of gbm surface failed"),
            display("Creation of gbm surface failed"),
        }

        #[doc = "No mode is set, blocking the current operation"]
        NoModeSet {
            description("No mode is currently set"),
            display("No mode is currently set"),
        }

        #[doc = "Creation of gbm buffer object failed"]
        BufferCreationFailed {
            description("Creation of gbm buffer object failed"),
            display("Creation of gbm buffer object failed"),
        }

        #[doc = "Writing to gbm buffer failed"]
        BufferWriteFailed {
            description("Writing to gbm buffer failed"),
            display("Writing to gbm buffer failed"),
        }

        #[doc = "Lock of gbm surface front buffer failed"]
        FrontBufferLockFailed {
            description("Lock of gbm surface front buffer failed"),
            display("Lock of gbm surface front buffer failed"),
        }

        #[doc = "Underlying backend failed"]
        UnderlyingBackendError {
            description("The underlying backend reported an error"),
            display("The underlying backend reported an error"),
        }
    }

    foreign_links {
        FailedToSwap(::backend::graphics::SwapBuffersError) #[doc = "Swapping front buffers failed"];
    }
}
