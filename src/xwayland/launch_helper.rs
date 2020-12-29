use std::{
    io::{Error as IOError, Read, Result as IOResult, Write},
    os::unix::{
        io::{AsRawFd, FromRawFd, RawFd},
        net::UnixStream,
    },
};

use nix::{
    errno::Errno,
    libc::exit,
    sys::{signal, socket, uio},
    unistd::{fork, ForkResult},
    Error as NixError, Result as NixResult,
};

use super::xserver::exec_xwayland;

const MAX_FDS: usize = 10;

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
            ForkResult::Parent { child: _ } => Ok(Self(me)),
        }
    }

    /// Get access to an fd that becomes readable when a launch finished. Call
    /// `was_launch_succesful()` once this happens.
    pub(crate) fn status_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    /// Check the status of a previous launch.
    pub(crate) fn was_launch_succesful(&self) -> IOResult<bool> {
        // This reads the one byte that is written at the end of do_child()
        let mut buffer = [0];
        let len = (&mut &self.0).read(&mut buffer)?;
        Ok(len > 0 && buffer[0] != 0)
    }

    /// Fork a child and call exec_xwayland() in it.
    pub(crate) fn launch(
        &self,
        display: u32,
        wayland_socket: UnixStream,
        wm_socket: UnixStream,
        listen_sockets: &[UnixStream],
    ) -> IOResult<()> {
        let buffer = display.to_ne_bytes();
        let mut fds = vec![wayland_socket.as_raw_fd(), wm_socket.as_raw_fd()];
        fds.extend(listen_sockets.iter().map(|s| s.as_raw_fd()));
        assert!(fds.len() <= MAX_FDS);
        send_with_fds(self.0.as_raw_fd(), &buffer, &fds)
    }
}

fn do_child(mut stream: UnixStream) -> IOResult<()> {
    use nix::sys::wait;

    let mut display = [0; 4];
    let mut fds = [0; MAX_FDS];

    // Block signals. SIGUSR1 being blocked is inherited by Xwayland and makes it signal its parent
    let mut set = signal::SigSet::empty();
    set.add(signal::Signal::SIGUSR1);
    set.add(signal::Signal::SIGCHLD);
    signal::sigprocmask(signal::SigmaskHow::SIG_BLOCK, Some(&set), None).map_err(nix_error_to_io)?;

    loop {
        while let Ok(_) = wait::waitpid(None, Some(wait::WaitPidFlag::WNOHANG)) {
            // We just want to reap the zombies
        }

        // Receive a new command: u32 display number and associated FDs
        let (bytes, num_fds) = receive_fds(stream.as_raw_fd(), &mut display, &mut fds)?;
        if bytes == 0 {
            // End of file => our parent exited => we should do the same
            break Ok(());
        }

        assert_eq!(bytes, 4);
        let display = u32::from_ne_bytes(display);

        // Wrap the FDs so that they are later closed.
        assert!(num_fds >= 2);
        let wayland_socket = unsafe { UnixStream::from_raw_fd(fds[0]) };
        let wm_socket = unsafe { UnixStream::from_raw_fd(fds[1]) };
        let mut listen_sockets = Vec::new();
        for idx in 2..num_fds {
            listen_sockets.push(unsafe { UnixStream::from_raw_fd(fds[idx]) });
        }

        // Fork Xwayland and report back the result
        let success = match fork_xwayland(display, wayland_socket, wm_socket, &listen_sockets) {
            Ok(true) => 1,
            Ok(false) => {
                eprintln!("Xwayland failed to start");
                0
            }
            Err(e) => {
                eprintln!("Failed to fork Xwayland: {:?}", e);
                0
            }
        };
        stream.write_all(&[success])?;
    }
}

/// fork() a child process and execute XWayland via exec_xwayland()
fn fork_xwayland(
    display: u32,
    wayland_socket: UnixStream,
    wm_socket: UnixStream,
    listen_sockets: &[UnixStream],
) -> NixResult<bool> {
    match unsafe { fork()? } {
        ForkResult::Parent { child: _ } => {
            // Wait for the child process to exit or send SIGUSR1
            let mut set = signal::SigSet::empty();
            set.add(signal::Signal::SIGUSR1);
            set.add(signal::Signal::SIGCHLD);
            match set.wait()? {
                signal::Signal::SIGUSR1 => Ok(true),
                _ => Ok(false),
            }
        }
        ForkResult::Child => {
            match exec_xwayland(display, wayland_socket, wm_socket, listen_sockets) {
                Ok(x) => match x {},
                Err(e) => {
                    // Well, what can we do? Our parent will get SIGCHLD when we exit.
                    eprintln!("exec Xwayland failed: {:?}", e);
                    unsafe { exit(1) };
                }
            }
        }
    }
}

/// Wrapper around `sendmsg()` for FD-passing
fn send_with_fds(fd: RawFd, bytes: &[u8], fds: &[RawFd]) -> IOResult<()> {
    let iov = [uio::IoVec::from_slice(bytes)];
    loop {
        let result = if !fds.is_empty() {
            let cmsgs = [socket::ControlMessage::ScmRights(fds)];
            socket::sendmsg(fd.as_raw_fd(), &iov, &cmsgs, socket::MsgFlags::empty(), None)
        } else {
            socket::sendmsg(fd.as_raw_fd(), &iov, &[], socket::MsgFlags::empty(), None)
        };
        match result {
            Ok(len) => {
                // All data should have been sent. Why would it fail!?
                assert_eq!(len, bytes.len());
                return Ok(());
            }
            Err(NixError::Sys(Errno::EINTR)) => {
                // Try again
            }
            Err(e) => return Err(nix_error_to_io(e)),
        }
    }
}

/// Wrapper around `recvmsg()` for FD-passing
fn receive_fds(fd: RawFd, buffer: &mut [u8], fds: &mut [RawFd]) -> IOResult<(usize, usize)> {
    let mut cmsg = cmsg_space!([RawFd; MAX_FDS]);
    let iov = [uio::IoVec::from_mut_slice(buffer)];

    let msg = loop {
        match socket::recvmsg(
            fd.as_raw_fd(),
            &iov[..],
            Some(&mut cmsg),
            socket::MsgFlags::empty(),
        ) {
            Ok(msg) => break msg,
            Err(NixError::Sys(Errno::EINTR)) => {
                // Try again
            }
            Err(e) => return Err(nix_error_to_io(e)),
        }
    };

    let received_fds = msg.cmsgs().flat_map(|cmsg| match cmsg {
        socket::ControlMessageOwned::ScmRights(s) => s,
        _ => Vec::new(),
    });
    let mut fd_count = 0;
    for (fd, place) in received_fds.zip(fds.iter_mut()) {
        fd_count += 1;
        *place = fd;
    }
    Ok((msg.bytes, fd_count))
}

fn nix_error_to_io(err: NixError) -> IOError {
    use std::io::ErrorKind;
    match err {
        NixError::Sys(errno) => errno.into(),
        NixError::InvalidPath | NixError::InvalidUtf8 => IOError::new(ErrorKind::InvalidInput, err),
        NixError::UnsupportedOperation => IOError::new(ErrorKind::Other, err),
    }
}
