use std::{
    io::{Result as IOResult, Error as IOError},
    os::unix::net::UnixStream,
};

use nix::{
    libc::exit,
    sys::signal,
    unistd::{fork, ForkResult, Pid},
    Error as NixError, Result as NixResult
};

use super::xserver::exec_xwayland;

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

    /// Fork a child and call exec_xwayland() in it.
    pub(crate) fn launch(
        &self,
        display: u32,
        wayland_socket: UnixStream,
        wm_socket: UnixStream,
        listen_sockets: &[UnixStream],
        log: &::slog::Logger,
    ) -> Result<(Pid, UnixStream), ()> {
        let (status_child, status_me) = UnixStream::pair().map_err(|_| ())?;
        match unsafe { fork() } {
            Ok(ForkResult::Parent { child }) => {
                // we are the main smithay process
                Ok((child, status_me))
            }
            Ok(ForkResult::Child) => {
                // we are the first child
                let mut set = signal::SigSet::empty();
                set.add(signal::Signal::SIGUSR1);
                set.add(signal::Signal::SIGCHLD);
                // we can't handle errors here anyway
                let _ = signal::sigprocmask(signal::SigmaskHow::SIG_BLOCK, Some(&set), None);
                match unsafe { fork() } {
                    Ok(ForkResult::Parent { child }) => {
                        // When we exit(), we will close() this which wakes up the main process.
                        let _status_child = status_child;
                        // we are still the first child
                        let sig = set.wait();
                        // Parent will wait for us and know from out
                        // exit status if XWayland launch was a success or not =)
                        if let Ok(signal::Signal::SIGCHLD) = sig {
                            // XWayland has exited before being ready
                            let _ = ::nix::sys::wait::waitpid(child, None);
                            unsafe { ::nix::libc::exit(1) };
                        }
                        unsafe { ::nix::libc::exit(0) };
                    }
                    Ok(ForkResult::Child) => {
                        // we are the second child, we exec xwayland
                        match exec_xwayland(display, wayland_socket, wm_socket, listen_sockets) {
                            Ok(x) => match x {},
                            Err(e) => {
                                // well, what can we do ?
                                error!(log, "exec XWayland failed"; "err" => format!("{:?}", e));
                                unsafe { ::nix::libc::exit(1) };
                            }
                        }
                    }
                    Err(e) => {
                        // well, what can we do ?
                        error!(log, "XWayland second fork failed"; "err" => format!("{:?}", e));
                        unsafe { ::nix::libc::exit(1) };
                    }
                }
            }
            Err(e) => {
                error!(log, "XWayland first fork failed"; "err" => format!("{:?}", e));
                Err(())
            }
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
