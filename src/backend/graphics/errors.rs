use std::error::Error;
use std::fmt;

/// Error that can happen when swapping buffers.
#[derive(Debug, Clone, PartialEq)]
pub enum SwapBuffersError {
    /// The corresponding context has been lost and needs to be recreated.
    ///
    /// All the objects associated to it (textures, buffers, programs, etc.)
    /// need to be recreated from scratch.
    ///
    /// Operations will have no effect. Functions that read textures, buffers, etc.
    /// will return uninitialized data instead.
    ContextLost,
    /// The buffers have already been swapped.
    ///
    /// This error can be returned when `swap_buffers` has been called multiple times
    /// without any modification in between.
    AlreadySwapped,
    /// Unknown error
    Unknown(u32),
}

impl fmt::Display for SwapBuffersError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(formatter, "{}", self.description())
    }
}

impl Error for SwapBuffersError {
    fn description(&self) -> &str {
        match *self {
            SwapBuffersError::ContextLost => "The context has been lost, it needs to be recreated",
            SwapBuffersError::AlreadySwapped => {
                "Buffers are already swapped, swap_buffers was called too many times"
            }
            SwapBuffersError::Unknown(_) => "Unknown error occurred",
        }
    }

    fn cause(&self) -> Option<&dyn Error> {
        None
    }
}
