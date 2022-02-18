use std::{
    io::{Read, Write},
    os::unix::{io::FromRawFd, net::UnixStream},
};

use slog::{debug, info, warn};

use nix::{errno::Errno, sys::socket};

/// Find a free X11 display slot and setup
pub(crate) fn prepare_x11_sockets(log: ::slog::Logger) -> Result<(X11Lock, [UnixStream; 2]), std::io::Error> {
    for d in 0..33 {
        // if fails, try the next one
        if let Ok(lock) = X11Lock::grab(d, log.clone()) {
            // we got a lockfile, try and create the socket
            if let Ok(sockets) = open_x11_sockets_for_display(d) {
                return Ok((lock, sockets));
            }
        }
    }
    // If we reach here, all values from 0 to 32 failed
    // we need to stop trying at some point
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        "Could not find a free socket for the XServer.",
    ))
}

#[derive(Debug)]
pub(crate) struct X11Lock {
    display: u32,
    log: ::slog::Logger,
}

impl X11Lock {
    /// Try to grab a lockfile for given X display number
    fn grab(display: u32, log: ::slog::Logger) -> Result<X11Lock, ()> {
        debug!(log, "Attempting to aquire an X11 display lock"; "D" => display);
        let filename = format!("/tmp/.X{}-lock", display);
        let lockfile = ::std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&filename);
        match lockfile {
            Ok(mut file) => {
                // we got it, write our PID in it and we're good
                let ret = file.write_fmt(format_args!("{:>10}\n", ::nix::unistd::Pid::this()));
                if ret.is_err() {
                    // write to the file failed ? we abandon
                    ::std::mem::drop(file);
                    let _ = ::std::fs::remove_file(&filename);
                    Err(())
                } else {
                    debug!(log, "X11 lock acquired"; "D" => display);
                    // we got the lockfile and wrote our pid to it, all is good
                    Ok(X11Lock { display, log })
                }
            }
            Err(_) => {
                debug!(log, "Failed to acquire lock"; "D" => display);
                // we could not open the file, now we try to read it
                // and if it contains the pid of a process that no longer
                // exist (so if a previous x server claimed it and did not
                // exit gracefully and remove it), we claim it
                // if we can't open it, give up
                let mut file = ::std::fs::File::open(&filename).map_err(|_| ())?;
                let mut spid = [0u8; 11];
                file.read_exact(&mut spid).map_err(|_| ())?;
                ::std::mem::drop(file);
                let pid = ::nix::unistd::Pid::from_raw(
                    ::std::str::from_utf8(&spid)
                        .map_err(|_| ())?
                        .trim()
                        .parse::<i32>()
                        .map_err(|_| ())?,
                );
                if let Err(Errno::ESRCH) = ::nix::sys::signal::kill(pid, None) {
                    // no process whose pid equals the contents of the lockfile exists
                    // remove the lockfile and try grabbing it again
                    if let Ok(()) = ::std::fs::remove_file(filename) {
                        debug!(log, "Lock was blocked by a defunct X11 server, trying again"; "D" => display);
                        return X11Lock::grab(display, log);
                    } else {
                        // we could not remove the lockfile, abort
                        return Err(());
                    }
                }
                // if we reach here, this lockfile exists and is probably in use, give up
                Err(())
            }
        }
    }

    pub(crate) fn display(&self) -> u32 {
        self.display
    }
}

impl Drop for X11Lock {
    fn drop(&mut self) {
        info!(self.log, "Cleaning up X11 lock.");
        // Cleanup all the X11 files
        if let Err(e) = ::std::fs::remove_file(format!("/tmp/.X11-unix/X{}", self.display)) {
            warn!(self.log, "Failed to remove X11 socket"; "error" => format!("{:?}", e));
        }
        if let Err(e) = ::std::fs::remove_file(format!("/tmp/.X{}-lock", self.display)) {
            warn!(self.log, "Failed to remove X11 lockfile"; "error" => format!("{:?}", e));
        }
    }
}

/// Open the two unix sockets an X server listens on
///
/// Should only be done after the associated lockfile is acquired!
fn open_x11_sockets_for_display(display: u32) -> nix::Result<[UnixStream; 2]> {
    let path = format!("/tmp/.X11-unix/X{}", display);
    let _ = ::std::fs::remove_file(&path);
    // We know this path is not to long, these unwrap cannot fail
    let fs_addr = socket::UnixAddr::new(path.as_bytes()).unwrap();
    let abs_addr = socket::UnixAddr::new_abstract(path.as_bytes()).unwrap();
    let fs_socket = open_socket(fs_addr)?;
    let abstract_socket = open_socket(abs_addr)?;
    Ok([fs_socket, abstract_socket])
}

/// Open an unix socket for listening and bind it to given path
fn open_socket(addr: socket::UnixAddr) -> nix::Result<UnixStream> {
    // create an unix stream socket
    let fd = socket::socket(
        socket::AddressFamily::Unix,
        socket::SockType::Stream,
        socket::SockFlag::SOCK_CLOEXEC,
        None,
    )?;
    // bind it to requested address
    if let Err(e) = socket::bind(fd, &socket::SockAddr::Unix(addr)) {
        let _ = ::nix::unistd::close(fd);
        return Err(e);
    }
    if let Err(e) = socket::listen(fd, 1) {
        let _ = ::nix::unistd::close(fd);
        return Err(e);
    }
    Ok(unsafe { FromRawFd::from_raw_fd(fd) })
}
