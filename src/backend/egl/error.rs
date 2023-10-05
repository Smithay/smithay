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
    /// Display creation failed
    #[error("Display creation failed with error: {0:}")]
    DisplayCreationError(#[source] EGLError),
    /// Display query result invalid
    #[error("Display query result invalid")]
    DisplayQueryResultInvalid,
    /// Unable to obtain a valid EGL Display
    #[error("Unable to obtain a valid EGL Display.")]
    DisplayNotSupported,
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
    /// Index of plane is out of bounds for `EGLImage`
    #[error("Index of plane is out of bounds for `EGLBuffer`")]
    PlaneIndexOutOfBounds,
    /// Failed to create `EGLImage` from the buffer
    #[error("Failed to create `EGLImage` from the buffer")]
    EGLImageCreationFailed,
    /// Failed to create `Dmabuf` from the image
    #[error("Faiedl to create `Dmabuf` from the image")]
    DmabufExportFailed(#[source] EGLError),
    /// Failed to query the available `EGLDevice`s
    #[error("Failed to query the available `EGLDevice`s")]
    QueryDevices(#[source] EGLError),
    /// Failed to query device properties
    #[error("Failed to query the requested device property")]
    QueryDeviceProperty(#[source] EGLError),
    /// The device does not have the given property
    #[error("The device does not have the given property")]
    EmptyDeviceProperty,
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
    /// An EGLDevice argument is not valid for this display.
    #[error("An EGLDevice argument is not valid for this display.")]
    BadDevice,
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
    /// The EGL operation failed due to temporary unavailability of a requested resource, but the arguments were otherwise valid, and a subsequent attempt may succeed.
    #[error("The EGL operation failed due to temporary unavailability of a requested resource, but the arguments were otherwise valid, and a subsequent attempt may succeed.")]
    ResourceBusy,
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
            ffi::egl::BAD_DEVICE_EXT => EGLError::BadDevice,
            ffi::egl::BAD_DISPLAY => EGLError::BadDisplay,
            ffi::egl::BAD_SURFACE => EGLError::BadSurface,
            ffi::egl::BAD_MATCH => EGLError::BadMatch,
            ffi::egl::BAD_PARAMETER => EGLError::BadParameter,
            ffi::egl::BAD_NATIVE_PIXMAP => EGLError::BadNativePixmap,
            ffi::egl::BAD_NATIVE_WINDOW => EGLError::BadNativeWindow,
            ffi::egl::RESOURCE_BUSY_EXT => EGLError::ResourceBusy,
            ffi::egl::CONTEXT_LOST => EGLError::ContextLost,
            x => EGLError::Unknown(x),
        }
    }
}

impl EGLError {
    pub(super) fn from_last_call() -> Option<EGLError> {
        match unsafe { ffi::egl::GetError() as u32 } {
            ffi::egl::SUCCESS => None,
            x => Some(EGLError::from(x)),
        }
    }
}

/// Wraps a raw egl call and returns error codes from `eglGetError()`, only if the result of the
/// call is different from the `err` value.
pub fn wrap_egl_call<R: PartialEq, F: FnOnce() -> R>(call: F, err: R) -> Result<R, EGLError> {
    let res = call();
    if res != err {
        Ok(res)
    } else {
        Err(EGLError::from_last_call().unwrap_or_else(|| {
            tracing::warn!("Erroneous EGL call didn't set EGLError");
            EGLError::Unknown(0)
        }))
    }
}

/// Wraps a raw egl call and returns error codes from `eglGetError()`, only if the pointer returned
/// is null.
pub fn wrap_egl_call_ptr<R, F: FnOnce() -> *const R>(call: F) -> Result<*const R, EGLError> {
    let res = call();
    if !res.is_null() {
        Ok(res)
    } else {
        Err(EGLError::from_last_call().unwrap_or_else(|| {
            tracing::warn!("Erroneous EGL call didn't set EGLError");
            EGLError::Unknown(0)
        }))
    }
}

/// Wraps a raw egl call and returns error codes from `eglGetError()`, only if the `EGLBoolean`
/// returned is `EGL_FALSE`.
pub fn wrap_egl_call_bool<F: FnOnce() -> ffi::egl::types::EGLBoolean>(
    call: F,
) -> Result<ffi::egl::types::EGLBoolean, EGLError> {
    wrap_egl_call(call, ffi::egl::FALSE)
}
