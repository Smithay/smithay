use super::ffi;

#[derive(thiserror::Error, Debug)]
/// EGL errors
pub enum Error {
    /// The requested OpenGL version is not supported
    #[error("The requested OpenGL version {0:?} is not supported")]
    OpenGlVersionNotSupported((u8, u8)),
    /// The EGL implementation does not support creating OpenGL ES contexts
    #[error("The EGL implementation does not support creating OpenGL ES contexts. Err: {0:?}")]
    OpenGlesNotSupported(#[source] Option<EGLError>),
    /// No available pixel format matched the criteria
    #[error("No available pixel format matched the criteria")]
    NoAvailablePixelFormat,
    /// Backend does not match the context type
    #[error("The expected backend '{0:?}' does not match the runtime")]
    NonMatchingBackend(&'static str),
    /// Unable to obtain a valid EGL Display
    #[error("Unable to obtain a valid EGL Display. Err: {0:}")]
    DisplayNotSupported(#[source] EGLError),
    /// `eglInitialize` returned an error
    #[error("Failed to initialize EGL. Err: {0:}")]
    InitFailed(#[source] EGLError),
    /// Failed to configure the EGL context
    #[error("Failed to configure the EGL context")]
    ConfigFailed(#[source] EGLError),
    /// Context creation failed as one or more requirements could not be met. Try removing some gl attributes or pixel format requirements
    #[error("Context creation failed as one or more requirements could not be met. Try removing some gl attributes or pixel format requirements. Err: {0:}")]
    CreationFailed(#[source] EGLError),
    /// The required EGL extension is not supported by the underlying EGL implementation
    #[error("None of the following EGL extensions is supported by the underlying EGL implementation, at least one is required: {0:?}")]
    EglExtensionNotSupported(&'static [&'static str]),
    /// Only one EGLDisplay may be bound to a given `WlDisplay` at any time
    #[error("Only one EGLDisplay may be bound to a given `WlDisplay` at any time")]
    OtherEGLDisplayAlreadyBound(#[source] EGLError),
    /// No EGLDisplay is currently bound to this `WlDisplay`
    #[error("No EGLDisplay is currently bound to this `WlDisplay`")]
    NoEGLDisplayBound,
    /// Index of plane is out of bounds for `EGLImages`
    #[error("Index of plane is out of bounds for `EGLImages`")]
    PlaneIndexOutOfBounds,
    /// Failed to create `EGLImages` from the buffer
    #[error("Failed to create `EGLImages` from the buffer")]
    EGLImageCreationFailed,
}

/// Raw EGL error
#[derive(thiserror::Error, Debug)]
pub enum EGLError {
    /// EGL is not initialized, or could not be initialized, for the specified EGL display connection.
    #[error(
        "EGL is not initialized, or could not be initialized, for the specified EGL display connection."
    )]
    NotInitialized,
    /// EGL cannot access a requested resource (for example a context is bound in another thread).
    #[error("EGL cannot access a requested resource (for example a context is bound in another thread).")]
    BadAccess,
    /// EGL failed to allocate resources for the requested operation.
    #[error("EGL failed to allocate resources for the requested operation.")]
    BadAlloc,
    /// An unrecognized attribute or attribute value was passed in the attribute list.
    #[error("An unrecognized attribute or attribute value was passed in the attribute list.")]
    BadAttribute,
    /// An EGLContext argument does not name a valid EGL rendering context.
    #[error("An EGLContext argument does not name a valid EGL rendering context.")]
    BadContext,
    /// An EGLConfig argument does not name a valid EGL frame buffer configuration.
    #[error("An EGLConfig argument does not name a valid EGL frame buffer configuration.")]
    BadConfig,
    /// The current surface of the calling thread is a window, pixel buffer or pixmap that is no longer valid.
    #[error("The current surface of the calling thread is a window, pixel buffer or pixmap that is no longer valid.")]
    BadCurrentSurface,
    /// An EGLDisplay argument does not name a valid EGL display connection.
    #[error("An EGLDisplay argument does not name a valid EGL display connection.")]
    BadDisplay,
    /// An EGLSurface argument does not name a valid surface (window, pixel buffer or pixmap) configured for GL rendering.
    #[error("An EGLSurface argument does not name a valid surface (window, pixel buffer or pixmap) configured for GL rendering.")]
    BadSurface,
    /// Arguments are inconsistent (for example, a valid context requires buffers not supplied by a valid surface).
    #[error("Arguments are inconsistent (for example, a valid context requires buffers not supplied by a valid surface).")]
    BadMatch,
    /// One or more argument values are invalid.
    #[error("One or more argument values are invalid.")]
    BadParameter,
    /// A NativePixmapType argument does not refer to a valid native pixmap.
    #[error("A NativePixmapType argument does not refer to a valid native pixmap.")]
    BadNativePixmap,
    /// A NativeWindowType argument does not refer to a valid native window.
    #[error("A NativeWindowType argument does not refer to a valid native window.")]
    BadNativeWindow,
    /// A power management event has occurred. The application must destroy all contexts and reinitialise OpenGL ES state and objects to continue rendering.
    #[error("A power management event has occurred. The application must destroy all contexts and reinitialise OpenGL ES state and objects to continue rendering.")]
    ContextLost,
    /// An unknown error
    #[error("An unknown error ({0:x})")]
    Unknown(u32),
}

impl From<u32> for EGLError {
    fn from(value: u32) -> Self {
        match value {
            ffi::egl::NOT_INITIALIZED => EGLError::NotInitialized,
            ffi::egl::BAD_ACCESS => EGLError::BadAccess,
            ffi::egl::BAD_ALLOC => EGLError::BadAlloc,
            ffi::egl::BAD_ATTRIBUTE => EGLError::BadAttribute,
            ffi::egl::BAD_CONTEXT => EGLError::BadContext,
            ffi::egl::BAD_CURRENT_SURFACE => EGLError::BadCurrentSurface,
            ffi::egl::BAD_DISPLAY => EGLError::BadDisplay,
            ffi::egl::BAD_SURFACE => EGLError::BadSurface,
            ffi::egl::BAD_MATCH => EGLError::BadMatch,
            ffi::egl::BAD_PARAMETER => EGLError::BadParameter,
            ffi::egl::BAD_NATIVE_PIXMAP => EGLError::BadNativePixmap,
            ffi::egl::BAD_NATIVE_WINDOW => EGLError::BadNativeWindow,
            ffi::egl::CONTEXT_LOST => EGLError::ContextLost,
            x => EGLError::Unknown(x),
        }
    }
}

impl EGLError {
    fn from_last_call() -> Result<(), EGLError> {
        match unsafe { ffi::egl::GetError() as u32 } {
            ffi::egl::SUCCESS => Ok(()),
            x => Err(EGLError::from(x)),
        }
    }
}

pub(crate) fn wrap_egl_call<R, F: FnOnce() -> R>(call: F) -> Result<R, EGLError> {
    let res = call();
    EGLError::from_last_call().map(|()| res)
}
