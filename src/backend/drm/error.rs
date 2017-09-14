use backend::graphics::egl::{CreationError, SwapBuffersError};
use drm::result::Error as DrmError;
use gbm::FrontBufferError;
use rental::TryNewError;

use std::error::{self, Error as ErrorTrait};
use std::fmt;
use std::io::Error as IoError;

/// Error summing up error types related to all underlying libraries
/// involved in creating the a `DrmDevice`/`DrmBackend`
#[derive(Debug)]
pub enum Error {
    /// The `DrmDevice` has encountered an error on an ioctl
    Drm(DrmError),
    /// The `EGLContext` could not be created
    EGLCreation(CreationError),
    /// Swapping Buffers via EGL was not possible
    EGLSwap(SwapBuffersError),
    /// Locking the front buffer of the underlying `GbmSurface` failed
    Gbm(FrontBufferError),
    /// A generic IO-Error happened accessing the underlying devices
    Io(IoError),
    /// Selected an invalid Mode
    Mode(ModeError),
    /// Error related to the selected crtc
    Crtc(CrtcError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DrmBackend error: {}",
            match self {
                &Error::Drm(ref x) => x as &error::Error,
                &Error::EGLCreation(ref x) => x as &error::Error,
                &Error::EGLSwap(ref x) => x as &error::Error,
                &Error::Gbm(ref x) => x as &error::Error,
                &Error::Io(ref x) => x as &error::Error,
                &Error::Mode(ref x) => x as &error::Error,
                &Error::Crtc(ref x) => x as &error::Error,
            }
        )
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        "DrmBackend error"
    }

    fn cause(&self) -> Option<&error::Error> {
        match self {
            &Error::Drm(ref x) => Some(x as &error::Error),
            &Error::EGLCreation(ref x) => Some(x as &error::Error),
            &Error::EGLSwap(ref x) => Some(x as &error::Error),
            &Error::Gbm(ref x) => Some(x as &error::Error),
            &Error::Io(ref x) => Some(x as &error::Error),
            &Error::Mode(ref x) => Some(x as &error::Error),
            &Error::Crtc(ref x) => Some(x as &error::Error),
        }
    }
}

impl From<DrmError> for Error {
    fn from(err: DrmError) -> Error {
        Error::Drm(err)
    }
}

impl From<CreationError> for Error {
    fn from(err: CreationError) -> Error {
        Error::EGLCreation(err)
    }
}

impl From<SwapBuffersError> for Error {
    fn from(err: SwapBuffersError) -> Error {
        Error::EGLSwap(err)
    }
}

impl From<FrontBufferError> for Error {
    fn from(err: FrontBufferError) -> Error {
        Error::Gbm(err)
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Error::Io(err)
    }
}

impl From<ModeError> for Error {
    fn from(err: ModeError) -> Error {
        match err {
            err @ ModeError::ModeNotSuitable => Error::Mode(err),
            ModeError::FailedToLoad(err) => Error::Drm(err),
        }
    }
}

impl From<CrtcError> for Error {
    fn from(err: CrtcError) -> Error {
        Error::Crtc(err)
    }
}

impl<H> From<TryNewError<Error, H>> for Error {
    fn from(err: TryNewError<Error, H>) -> Error {
        err.0
    }
}

/// Error when trying to select an invalid mode
#[derive(Debug)]
pub enum ModeError {
    /// `Mode` is not compatible with all given connectors
    ModeNotSuitable,
    /// Failed to load `Mode` information
    FailedToLoad(DrmError),
}

impl fmt::Display for ModeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())?;
        if let Some(cause) = self.cause() {
            write!(f, "\tCause: {}", cause)?;
        }
        Ok(())
    }
}

impl error::Error for ModeError {
    fn description(&self) -> &str {
        match self {
            &ModeError::ModeNotSuitable => "Mode does not match all attached connectors",
            &ModeError::FailedToLoad(_) => "Failed to load mode information from device",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match self {
            &ModeError::FailedToLoad(ref err) => Some(err as &error::Error),
            _ => None,
        }
    }
}

/// Errors related to the selected crtc
#[derive(Debug)]
pub enum CrtcError {
    /// Selected crtc is already in use by another `DrmBackend`
    AlreadyInUse
}

impl fmt::Display for CrtcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())?;
        if let Some(cause) = self.cause() {
            write!(f, "\tCause: {}", cause)?;
        }
        Ok(())
    }
}

impl error::Error for CrtcError {
    fn description(&self) -> &str {
        match self {
            &CrtcError::AlreadyInUse => "Crtc is already in use by another DrmBackend",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match self {
            _ => None,
        }
    }
}
