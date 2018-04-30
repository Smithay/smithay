use std::io::{Read, Write};
use std::os::unix::io::RawFd;

use nix::{Error as NixError, Result as NixResult};
use nix::errno::Errno;
use nix::sys::socket;

pub(crate) fn make_pair() -> Result<(RawFd, RawFd), ()> {
    socket::socketpair(
        socket::AddressFamily::Unix,
        socket::SockType::Stream,
        None,
        socket::SockFlag::SOCK_CLOEXEC,
    ).map_err(|_| ())
}

/// Find a free X11 display slot and setup
pub(crate) fn prepare_x11_sockets() -> Result<(u32, [RawFd; 2]), ()> {
    for d in 0..33 {
        // if fails, try the next one
        if let Err(()) = grab_lockfile(d) {
            continue;
        }
        // we got a lockfile, try and create the socket
        if let Ok(fds) = open_x11_sockets_for_display(d) {
            return Ok((d, fds));
        }
        // creating the sockets failed, for some readon ?
        // release the lockfile and try with the next
        release_lockfile(d);
    }
    // If we reach here, all values from 0 to 32 failed
    // we need to stop trying at some point
    return Err(());
}

/// Remove the X11 sockets for a given display number
pub(crate) fn cleanup_x11_sockets(display: u32) {
    let _ = ::std::fs::remove_file(format!("/tmp/.X11-unix/X{}", display));
    let _ = ::std::fs::remove_file(format!("/tmp/.X{}-lock", display));
}

/// Try to grab a lockfile for given X display number
fn grab_lockfile(display: u32) -> Result<(), ()> {
    let filename = format!("/tmp/.X{}-lock", display);
    let lockfile = ::std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&filename);
    match lockfile {
        Ok(mut file) => {
            // we got it, write our PID in it and we're good
            let ret = file.write_fmt(format_args!("{:>10}", ::nix::unistd::Pid::this()));
            if let Err(_) = ret {
                // write to the file failed ? we abandon
                ::std::mem::drop(file);
                let _ = ::std::fs::remove_file(&filename);
                return Err(());
            } else {
                // we got the lockfile and wrote our pid to it, all is good
                return Ok(());
            }
        }
        Err(_) => {
            // we could not open the file, now we try to read it
            // and if it contains the pid of a process that no longer
            // exist (so if a previous x server claimed it and did not
            // exit gracefully and remove it), we claim it
            // if we can't open it, give up
            let mut file = ::std::fs::File::open(&filename).map_err(|_| ())?;
            let mut spid = [0u8; 11];
            file.read_exact(&mut spid).map_err(|_| ())?;
            ::std::mem::drop(file);
            let pid = ::nix::unistd::Pid::from_raw(::std::str::from_utf8(&spid)
                .map_err(|_| ())?
                .trim()
                .parse::<i32>()
                .map_err(|_| ())?);
            if let Err(NixError::Sys(Errno::ESRCH)) = ::nix::sys::signal::kill(pid, None) {
                // no process whose pid equals the contents of the lockfile exists
                // remove the lockfile and try grabbing it again
                let _ = ::std::fs::remove_file(filename);
                return grab_lockfile(display);
            }
            // if we reach here, this lockfile exists and is probably in use, give up
            return Err(());
        }
    }
}

/// Release an X11 lockfile
fn release_lockfile(display: u32) {
    let filename = format!("/tmp/.X{}-lock", display);
    let _ = ::std::fs::remove_file(filename);
}

/// Open the two unix sockets an X server listens on
///
/// SHould only be done after the associated lockfile is aquired!
fn open_x11_sockets_for_display(display: u32) -> NixResult<[RawFd; 2]> {
    let fs_socket = open_socket(
        socket::UnixAddr::new(format!("/tmp/.X11-unix/X{}", display).as_bytes()).unwrap(), // We know this path is not to long, this unwrap cannot fail
    )?;
    let ret = open_socket(
        socket::UnixAddr::new_abstract(format!("/tmp/.X11-unix/X{}", display).as_bytes()).unwrap(), // We know this path is not to long, this unwrap cannot fail
    );
    match ret {
        Ok(abstract_socket) => Ok([fs_socket, abstract_socket]),
        Err(e) => {
            // close the first socket and return the error
            let _ = ::nix::unistd::close(fs_socket);
            Err(e)
        }
    }
}

/// Open an unix socket for listening and bind it to given path
fn open_socket(addr: socket::UnixAddr) -> NixResult<RawFd> {
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
    Ok(fd)
}
