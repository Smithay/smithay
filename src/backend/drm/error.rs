

use backend::graphics::egl::{CreationError, SwapBuffersError};
use drm::result::Error as DrmError;
use gbm::FrontBufferError;
use rental::TryNewError;

use std::error::{self, Error as ErrorTrait};
use std::fmt;
use std::io::Error as IoError;

#[derive(Debug)]
pub enum Error {
    Drm(DrmError),
    EGLCreation(CreationError),
    EGLSwap(SwapBuffersError),
    Gbm(FrontBufferError),
    Io(IoError),
    Mode(ModeError),
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

impl<H> From<TryNewError<Error, H>> for Error {
    fn from(err: TryNewError<Error, H>) -> Error {
        err.0
    }
}

#[derive(Debug)]
pub enum ModeError {
    ModeNotSuitable,
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
