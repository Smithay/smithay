use x11rb::rust_connection::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};

/// An error emitted by the X11 backend during setup.
#[derive(Debug, thiserror::Error)]
pub enum X11Error {
    /// Connecting to the X server failed.
    #[error("Connecting to the X server failed")]
    ConnectionFailed(#[from] ConnectError),

    /// A required X11 extension was not present or has the right version.
    #[error("{0}")]
    MissingExtension(#[from] MissingExtensionError),

    /// Some protocol error occurred during setup.
    #[error("Some protocol error occurred during setup")]
    Protocol(#[from] ReplyOrIdError),

    /// Creating the window failed.
    #[error("Creating the window failed")]
    CreateWindow(#[from] CreateWindowError),

    /// An X11 surface already exists for this window.
    #[error("An X11 surface already exists for this window")]
    SurfaceExists,

    /// An invalid window was used to create an X11 surface.
    ///
    /// This error will be risen if the window was destroyed or the window does not belong to the [`X11Handle`](super::X11Handle)
    /// in use.
    #[error("An invalid window was used to create an X11 surface")]
    InvalidWindow,

    /// The X server is not capable of direct rendering.
    #[error("The X server is not capable of direct rendering")]
    CannotDirectRender,
}

impl From<ReplyError> for X11Error {
    fn from(err: ReplyError) -> Self {
        Self::Protocol(err.into())
    }
}

impl From<ConnectionError> for X11Error {
    fn from(err: ConnectionError) -> Self {
        Self::Protocol(err.into())
    }
}

/// An error that occurs when a required X11 extension is not present.
#[derive(Debug, thiserror::Error)]
pub enum MissingExtensionError {
    /// An extension was not found.
    #[error("Extension \"{name}\" version {major}.{minor} was not found.")]
    NotFound {
        /// The name of the required extension.
        name: &'static str,
        /// The minimum required major version of extension.
        major: u32,
        /// The minimum required minor version of extension.
        minor: u32,
    },

    /// An extension was present, but the version is too low.
    #[error("Extension \"{name}\" version {required_major}.{required_minor} is required but only version {available_major}.{available_minor} is available.")]
    WrongVersion {
        /// The name of the extension.
        name: &'static str,
        /// The minimum required major version of extension.
        required_major: u32,
        /// The minimum required minor version of extension.
        required_minor: u32,
        /// The major version of the extension available on the X server.
        available_major: u32,
        /// The minor version of the extension available on the X server.
        available_minor: u32,
    },
}

/// An error which may occur when creating an X11 window.
#[derive(Debug, thiserror::Error)]
pub enum CreateWindowError {
    /// No depth fulfilling the pixel format requirements was found.
    #[error("No depth fulfilling the requirements was found")]
    NoDepth,

    /// No visual fulfilling the pixel format requirements was found.
    #[error("No visual fulfilling the requirements was found")]
    NoVisual,
}
