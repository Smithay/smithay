use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
};

use tracing::{debug, info, warn};

use rustix::{io::Errno, net::SocketAddrUnix};

/// Find a free X11 display slot and setup
pub(crate) fn prepare_x11_sockets(
    display: Option<u32>,
    open_abstract_socket: bool,
) -> Result<(X11Lock, Vec<UnixStream>), std::io::Error> {
    match display {
        Some(d) => {
            if let Ok(lock) = X11Lock::grab(d) {
                // we got a lockfile, try and create the socket
                match open_x11_sockets_for_display(d, open_abstract_socket) {
                    Ok(sockets) => return Ok((lock, sockets)),
                    Err(err) => return Err(std::io::Error::from(err)),
                };
            }
        }
        None => {
            for d in 0..33 {
                // if fails, try the next one
                if let Ok(lock) = X11Lock::grab(d) {
                    // we got a lockfile, try and create the socket
                    match open_x11_sockets_for_display(d, open_abstract_socket) {
                        Ok(sockets) => return Ok((lock, sockets)),
                        Err(err) => warn!(display = d, "Failed to create sockets: {}", err),
                    }
                }
            }
            // If we reach here, all values from 0 to 32 failed
            // we need to stop trying at some point
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        "Could not find a free socket for the XServer.",
    ))
}

#[derive(Debug)]
pub(crate) struct X11Lock {
    display: u32,
}

impl X11Lock {
    /// Try to grab a lockfile for given X display number
    fn grab(number: u32) -> Result<X11Lock, ()> {
        debug!(display = number, "Attempting to aquire an X11 display lock");
        let filename = format!("/tmp/.X{}-lock", number);
        let lockfile = ::std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&filename);
        match lockfile {
            Ok(mut file) => {
                // we got it, write our PID in it and we're good
                let ret = file.write_fmt(format_args!(
                    "{:>10}\n",
                    rustix::process::Pid::as_raw(Some(rustix::process::getpid()))
                ));
                if ret.is_err() {
                    // write to the file failed ? we abandon
                    ::std::mem::drop(file);
                    let _ = ::std::fs::remove_file(&filename);
                    Err(())
                } else {
                    debug!(display = number, "X11 lock acquired");
                    // we got the lockfile and wrote our pid to it, all is good
                    Ok(X11Lock { display: number })
                }
            }
            Err(_) => {
                debug!(display = number, "Failed to acquire lock");
                // we could not open the file, now we try to read it
                // and if it contains the pid of a process that no longer
                // exist (so if a previous x server claimed it and did not
                // exit gracefully and remove it), we claim it
                // if we can't open it, give up
                let mut file = ::std::fs::File::open(&filename).map_err(|_| ())?;
                let mut spid = [0u8; 11];
                file.read_exact(&mut spid).map_err(|_| ())?;
                ::std::mem::drop(file);
                let pid = rustix::process::Pid::from_raw(
                    ::std::str::from_utf8(&spid)
                        .map_err(|_| ())?
                        .trim()
                        .parse::<i32>()
                        .map_err(|_| ())?,
                )
                .ok_or(())?;
                if let Err(Errno::SRCH) = rustix::process::test_kill_process(pid) {
                    // no process whose pid equals the contents of the lockfile exists
                    // remove the lockfile and try grabbing it again
                    if let Ok(()) = ::std::fs::remove_file(filename) {
                        debug!(
                            display = number,
                            "Lock was blocked by a defunct X11 server, trying again"
                        );
                        return X11Lock::grab(number);
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

    pub(crate) fn display_number(&self) -> u32 {
        self.display
    }
}

impl Drop for X11Lock {
    fn drop(&mut self) {
        info!("Cleaning up X11 lock.");
        // Cleanup all the X11 files
        if let Err(e) = ::std::fs::remove_file(format!("/tmp/.X11-unix/X{}", self.display)) {
            warn!(error = ?e, "Failed to remove X11 socket");
        }
        if let Err(e) = ::std::fs::remove_file(format!("/tmp/.X{}-lock", self.display)) {
            warn!(error = ?e, "Failed to remove X11 lockfile");
        }
    }
}

/// Open the two unix sockets an X server listens on
///
/// Should only be done after the associated lockfile is acquired!
#[cfg(target_os = "linux")]
fn open_x11_sockets_for_display(
    display: u32,
    open_abstract_socket: bool,
) -> rustix::io::Result<Vec<UnixStream>> {
    let path = format!("/tmp/.X11-unix/X{}", display);
    let _ = ::std::fs::remove_file(&path);
    // We know this path is not too long, these unwrap cannot fail
    let fs_addr = SocketAddrUnix::new(path.as_bytes()).unwrap();
    let mut sockets = vec![open_socket(fs_addr)?];
    if open_abstract_socket {
        let abs_addr = SocketAddrUnix::new_abstract_name(path.as_bytes()).unwrap();
        sockets.push(open_socket(abs_addr)?);
    }
    Ok(sockets)
}

/// Open the two unix sockets an X server listens on
///
/// Should only be done after the associated lockfile is acquired!
#[cfg(not(target_os = "linux"))]
fn open_x11_sockets_for_display(
    display: u32,
    _open_abstract_socket: bool,
) -> rustix::io::Result<Vec<UnixStream>> {
    let path = format!("/tmp/.X11-unix/X{}", display);
    let _ = ::std::fs::remove_file(&path);
    // We know this path is not too long, these unwrap cannot fail
    let fs_addr = SocketAddrUnix::new(path.as_bytes()).unwrap();
    Ok(vec![open_socket(fs_addr)?])
}

/// Open an unix socket for listening and bind it to given path
fn open_socket(addr: SocketAddrUnix) -> rustix::io::Result<UnixStream> {
    // create an unix stream socket
    let fd = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC,
        None,
    )?;
    // bind it to requested address
    rustix::net::bind_unix(&fd, &addr)?;
    rustix::net::listen(&fd, 1)?;
    Ok(UnixStream::from(fd))
}
