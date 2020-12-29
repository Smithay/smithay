use std::{
    io::{Result as IOResult, Error as IOError},
    os::unix::net::UnixStream,
};

use nix::{
    libc::exit,
    unistd::{fork, ForkResult},
    Error as NixError, Result as NixResult
};

/// A handle to a `fork()`'d child process that is used for starting XWayland
pub struct LaunchHelper(UnixStream);

impl LaunchHelper {
    /// Start a new launch helper process.
    ///
    /// This function calls [`nix::unistd::fork`] to create a new process. This function is unsafe
    /// because `fork` is unsafe. Most importantly: Calling `fork` in a multi-threaded process has
    /// lots of limitations. Thus, you must call this function before any threads are created.
    pub unsafe fn fork() -> IOResult<Self> {
        let (child, me) = UnixStream::pair()?;
        match fork().map_err(nix_error_to_io)? {
            ForkResult::Child => {
                drop(me);
                match do_child(child) {
                    Ok(()) => exit(0),
                    Err(e) => {
                        eprintln!("Error in smithay fork child: {:?}", e);
                        exit(1);
                    }
                }
            }
            ForkResult::Parent { child: _ } => Ok(Self(me))
        }
    }
}

fn do_child(stream: UnixStream) -> NixResult<()> {
    let _ = stream; // TODO
    Ok(())
}

fn nix_error_to_io(err: NixError) -> IOError {
    use std::io::ErrorKind;
    match err {
        NixError::Sys(errno) => errno.into(),
        NixError::InvalidPath | NixError::InvalidUtf8 =>
            IOError::new(ErrorKind::InvalidInput, err),
        NixError::UnsupportedOperation =>
            IOError::new(ErrorKind::Other, err),
    }
}
