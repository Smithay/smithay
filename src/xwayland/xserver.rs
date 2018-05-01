/*
 * Steps of Xwayland server creation
 *
 * Sockets to create:
 * - a pair for Xwayland to connect to smithay as a wayland client, we use our
 *   end to insert the Xwayland client in the display
 * - a pair for smithay to connect to Xwayland as a WM, we give our end to the
 *   WM and it deals with it
 * - 2 listening sockets on which the Xwayland server will listen. We need to
 *   bind them ouserlves so we know what value put in the $DISPLAY env variable.
 *   This involves some dance with a lockfile to ensure there is no collision with
 *   an other starting xserver
 *   if we listen on display $D, their paths are respectly:
 *   - /tmp/.X11-unix/X$D
 *   - @/tmp/.X11-unix/X$D (abstract socket)
 *
 * The XWayland server is spawned via fork+exec.
 * -> wlroot does a double-fork while weston a single one, why ??
 *    -> https://stackoverflow.com/questions/881388/
 * -> once it is started, it sends us a SIGUSR1, we need to setup a listener
 *    for it and when we receive it we can launch the WM
 * -> we need to track if the Xwayland crashes, to restart it
 *
 * cf https://github.com/swaywm/wlroots/blob/master/xwayland/xwayland.c
 *
 */
use std::cell::RefCell;
use std::rc::Rc;
use std::env;
use std::ffi::CString;
use std::os::unix::io::{AsRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;

use nix::{Error as NixError, Result as NixResult};
use nix::errno::Errno;
use nix::unistd::{fork, ForkResult, Pid};
use nix::sys::signal;

use wayland_server::{Client, Display, LoopToken};
use wayland_server::sources::{SignalEvent, Source};

use super::x11_sockets::{X11Lock, prepare_x11_sockets};

/// The XWayland handle
pub struct XWayland<WM: XWindowManager> {
    inner: Rc<RefCell<Inner<WM>>>,
}

/// Trait to be implemented by you WM for Xwayland
///
/// This is a very low-level trait, only notifying you
/// when the connection with XWayland is up, or when
/// it terminates.
///
/// You WM must be able handle the XWayland server connecting
/// then disconnecting several time in a row, but only a single
/// connection will be active at any given time.
pub trait XWindowManager {
    /// The XWayland server is ready
    ///
    /// Your previlegied connection to it is this `UnixStream`
    fn xwayland_ready(&mut self, connection: UnixStream, client: Client);
    /// The XWayland server has exited
    fn xwayland_exited(&mut self);
}

impl<WM: XWindowManager + 'static> XWayland<WM> {
    /// Start the XWayland server
    pub fn init(wm: WM, token: LoopToken, display: Rc<RefCell<Display>>) -> Result<XWayland<WM>, ()> {
        let inner = Rc::new(RefCell::new(Inner {
            wm,
            token,
            wayland_display: display,
            instance: None,
        }));
        launch(&inner)?;
        Ok(XWayland { inner })
    }
}

impl<WM: XWindowManager> Drop for XWayland<WM> {
    fn drop(&mut self) {
        self.inner.borrow_mut().shutdown();
    }
}

struct XWaylandInstance {
    display_lock: X11Lock,
    wayland_client: Client,
    sigusr1_handler: Option<Source<SignalEvent>>,
    wm_fd: Option<UnixStream>,
    started_at: ::std::time::Instant,
    child_pid: Option<Pid>,
}

// Inner implementation of the XWayland manager
struct Inner<WM: XWindowManager> {
    wm: WM,
    token: LoopToken,
    wayland_display: Rc<RefCell<Display>>,
    instance: Option<XWaylandInstance>,
}

// Launch an XWayland server
//
// Does nothing if there is already a launched instance
fn launch<WM: XWindowManager + 'static>(inner: &Rc<RefCell<Inner<WM>>>) -> Result<(), ()> {
    let mut guard = inner.borrow_mut();
    if guard.instance.is_some() {
        return Ok(());
    }

    let (x_wm_x11, x_wm_me) = UnixStream::pair().map_err(|_| ())?;
    let (wl_x11, wl_me) = UnixStream::pair().map_err(|_| ())?;

    let (lock, x_fds) = prepare_x11_sockets()?;

    // we have now created all the required sockets

    // record launch time
    let creation_time = ::std::time::Instant::now();

    // create the wayland client for XWayland
    let client = unsafe {
        guard
            .wayland_display
            .borrow_mut()
            .create_client(wl_me.into_raw_fd())
    };
    client.set_user_data(Rc::into_raw(inner.clone()) as *const () as *mut ());
    client.set_destructor(client_destroy::<WM>);

    // setup the SIGUSR1 handler
    let my_inner = inner.clone();
    let sigusr1_handler = guard
        .token
        .add_signal_event_source(signal::Signal::SIGUSR1, move |_, ()| {
            xwayland_ready(&my_inner)
        })
        .map_err(|_| ())?;

    // all is ready, we can do the fork dance
    let child_pid = match fork() {
        Ok(ForkResult::Parent { child }) => {
            // we are the main smithay process
            child
        }
        Ok(ForkResult::Child) => {
            // we are the first child
            let ppid = Pid::parent();
            let mut set = signal::SigSet::empty();
            set.add(signal::Signal::SIGUSR1);
            set.add(signal::Signal::SIGCHLD);
            // we can't handle errors here anyway
            let _ = signal::sigprocmask(signal::SigmaskHow::SIG_BLOCK, Some(&set), None);
            match fork() {
                Ok(ForkResult::Parent { child }) => {
                    // we are still the first child
                    let sig = set.wait();
                    // send USR1 to parent
                    let _ = signal::kill(ppid, signal::Signal::SIGUSR1);
                    // Parent will wait for us and know from out
                    // exit status if XWayland launch was a success or not =)
                    if let Ok(signal::Signal::SIGCHLD) = sig {
                        // Xwayland has exited before being ready
                        let _ = ::nix::sys::wait::waitpid(child, None);
                        unsafe { ::nix::libc::exit(1) };
                    }
                    unsafe { ::nix::libc::exit(0) };
                }
                Ok(ForkResult::Child) => {
                    // we are the second child, we exec xwayland
                    match exec_xwayland(lock.display(), wl_x11, x_wm_x11, &x_fds) {
                        Ok(x) => match x {},
                        Err(e) => {
                            // well, what can we do ?
                            eprintln!("[smithay] exec XWayland failed: {:?}", e);
                            unsafe { ::nix::libc::exit(1) };
                        }
                    }
                }
                Err(e) => {
                    // well, what can we do ?
                    eprintln!("[smithay] XWayland second fork failed: {:?}", e);
                    unsafe { ::nix::libc::exit(1) };
                }
            }
        }
        Err(e) => {
            eprintln!("[smithay] XWayland first fork failed: {:?}", e);
            return Err(());
        }
    };

    guard.instance = Some(XWaylandInstance {
        display_lock: lock,
        wayland_client: client,
        sigusr1_handler: Some(sigusr1_handler),
        wm_fd: Some(x_wm_me),
        started_at: creation_time,
        child_pid: Some(child_pid),
    });

    Ok(())
}

impl<WM: XWindowManager> Inner<WM> {
    // Shutdown the XWayland server and cleanup everything
    fn shutdown(&mut self) {
        // don't do anything if not running
        if let Some(mut instance) = self.instance.take() {
            self.wm.xwayland_exited();
            // kill the client
            instance.wayland_client.kill();
            // remove the event source
            if let Some(s) = instance.sigusr1_handler.take() {
                s.remove();
            }
            // All connexions and lockfiles are cleaned by their destructors

            // Remove DISPLAY from the env
            ::std::env::remove_var("DISPLAY");
            // We do like wlroots:
            // > We do not kill the Xwayland process, it dies to broken pipe
            // > after we close our side of the wm/wl fds. This is more reliable
            // > than trying to kill something that might no longer be Xwayland.
        }
    }
}

fn client_destroy<WM: XWindowManager + 'static>(data: *mut ()) {
    let inner = unsafe { Rc::from_raw(data as *const () as *const RefCell<Inner<WM>>) };

    // shutdown the server
    let started_at = inner.borrow().instance.as_ref().map(|i| i.started_at);
    inner.borrow_mut().shutdown();

    // restart it, unless we really just started it, if it crashes right
    // at startup there is no point
    if started_at.map(|t| t.elapsed().as_secs()).unwrap_or(10) > 5 {
        let _ = launch(&inner);
    }
}

fn xwayland_ready<WM: XWindowManager>(inner: &Rc<RefCell<Inner<WM>>>) {
    use nix::sys::wait;
    let mut guard = inner.borrow_mut();
    let inner = &mut *guard;
    // instance should never be None at this point
    let instance = inner.instance.as_mut().unwrap();
    let wm = &mut inner.wm;
    // neither the pid
    let pid = instance.child_pid.unwrap();

    // find out if the launch was a success by waiting on the intermediate child
    let mut success: bool;
    loop {
        match wait::waitpid(pid, None) {
            Ok(wait::WaitStatus::Exited(_, 0)) => {
                // XWayland was correctly started :)
                success = true;
                break;
            }
            Err(NixError::Sys(Errno::EINTR)) => {
                // interupted, retry
                continue;
            }
            _ => {
                // something went wrong :(
                success = false;
                break;
            }
        }
    }

    if success {
        // signal the WM
        wm.xwayland_ready(
            instance.wm_fd.take().unwrap(), // This is a bug if None
            instance.wayland_client.clone(),
        );

        // setup the environemnt
        ::std::env::set_var("DISPLAY", format!(":{}", instance.display_lock.display()));
    }

    // in all cases, cleanup
    if let Some(s) = instance.sigusr1_handler.take() {
        s.remove();
    }
}

enum Void {}

/// Exec xwayland with given sockets on given display
///
/// If this returns, that means that something failed
fn exec_xwayland(
    display: u32,
    wayland_socket: UnixStream,
    wm_socket: UnixStream,
    listen_sockets: &[UnixStream],
) -> NixResult<Void> {
    // uset the CLOEXEC flag from the sockets we need to pass
    // to xwayland
    unset_cloexec(&wayland_socket)?;
    unset_cloexec(&wm_socket)?;
    for socket in listen_sockets {
        unset_cloexec(socket)?;
    }
    // prepare the arguments to Xwayland
    let mut args = vec![
        CString::new("Xwayland").unwrap(),
        CString::new(format!(":{}", display)).unwrap(),
        CString::new("-rootless").unwrap(),
        CString::new("-terminate").unwrap(),
        CString::new("-wm").unwrap(),
        CString::new(format!("{}", wm_socket.as_raw_fd())).unwrap(),
    ];
    for socket in listen_sockets {
        args.push(CString::new("-listen").unwrap());
        args.push(CString::new(format!("{}", socket.as_raw_fd())).unwrap());
    }
    // setup the environment: clear everything except PATH and XDG_RUNTIME_DIR
    for (key, _) in env::vars_os() {
        if key.to_str() == Some("PATH") || key.to_str() == Some("XDG_RUNTIME_DIR") {
            continue;
        }
        env::remove_var(key);
    }
    // the WAYLAND_SOCKET var tells Xwayland where to connect as a wayland client
    env::set_var("WAYLAND_SOCKET", format!("{}", wayland_socket.as_raw_fd()));

    // ignore SIGUSR1, this will make the Xwayland server send us this
    // signal when it is ready apparently
    unsafe {
        use nix::sys::signal::*;
        sigaction(
            Signal::SIGUSR1,
            &SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty()),
        )?;
    }

    // run it
    let ret = ::nix::unistd::execvp(&CString::new("Xwayland").unwrap(), &args)?;
    // small dance to actually return Void
    match ret {}
}

/// Remove the O_CLOEXEC flag from this Fd
///
/// This means that the Fd will *not* be automatically
/// closed when we exec() into Xwayland
fn unset_cloexec<F: AsRawFd>(fd: &F) -> NixResult<()> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    fcntl(fd.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::empty()))?;
    Ok(())
}
