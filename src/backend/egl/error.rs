#[derive(thiserror::Error, Debug)]
/// EGL errors
pub enum Error {
    /// The requested OpenGL version is not supported
    #[error("The requested OpenGL version {0:?} is not supported")]
    OpenGlVersionNotSupported((u8, u8)),
    /// The EGL implementation does not support creating OpenGL ES contexts
    #[error("The EGL implementation does not support creating OpenGL ES contexts")]
    OpenGlesNotSupported,
    /// No available pixel format matched the criteria
    #[error("No available pixel format matched the criteria")]
    NoAvailablePixelFormat,
    /// Backend does not match the context type
    #[error("The expected backend '{0:?}' does not match the runtime")]
    NonMatchingBackend(&'static str),
    /// Unable to obtain a valid EGL Display
    #[error("Unable to obtain a valid EGL Display")]
    DisplayNotSupported,
    /// `eglInitialize` returned an error
    #[error("Failed to initialize EGL")]
    InitFailed,
    /// Failed to configure the EGL context
    #[error("Failed to configure the EGL context")]
    ConfigFailed,
    /// Context creation failed as one or more requirements could not be met. Try removing some gl attributes or pixel format requirements
    #[error("Context creation failed as one or more requirements could not be met. Try removing some gl attributes or pixel format requirements")]
    CreationFailed,
    /// `eglCreateWindowSurface` failed
    #[error("`eglCreateWindowSurface` failed")]
    SurfaceCreationFailed,
    /// The required EGL extension is not supported by the underlying EGL implementation
    #[error("None of the following EGL extensions is supported by the underlying EGL implementation, at least one is required: {0:?}")]
    EglExtensionNotSupported(&'static [&'static str]),
    /// Only one EGLDisplay may be bound to a given `WlDisplay` at any time
    #[error("Only one EGLDisplay may be bound to a given `WlDisplay` at any time")]
    OtherEGLDisplayAlreadyBound,
    /// No EGLDisplay is currently bound to this `WlDisplay`
    #[error("No EGLDisplay is currently bound to this `WlDisplay`")]
    NoEGLDisplayBound,
    /// Index of plane is out of bounds for `EGLImages`
    #[error("Index of plane is out of bounds for `EGLImages`")]
    PlaneIndexOutOfBounds,
    /// Failed to create `EGLImages` from the buffer
    #[error("Failed to create `EGLImages` from the buffer")]
    EGLImageCreationFailed,
    /// The reason of failure could not be determined
    #[error("Unknown error: {0}")]
    Unknown(u32),
}
