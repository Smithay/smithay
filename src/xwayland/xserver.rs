/*
 * Steps of XWayland server creation
 *
 * Sockets to create:
 * - a pair for XWayland to connect to smithay as a wayland client, we use our
 *   end to insert the XWayland client in the display
 * - a pair for smithay to connect to XWayland as a WM, we give our end to the
 *   WM and it deals with it
 * - 2 listening sockets on which the XWayland server will listen. We need to
 *   bind them ourselves so we know what value put in the $DISPLAY env variable.
 *   This involves some dance with a lockfile to ensure there is no collision with
 *   an other starting xserver
 *   if we listen on display $D, their paths are respectively:
 *   - /tmp/.X11-unix/X$D
 *   - @/tmp/.X11-unix/X$D (abstract socket)
 *
 * The XWayland server is spawned via fork+exec.
 * -> wlroots does a double-fork while weston a single one, why ??
 *    -> https://stackoverflow.com/questions/881388/
 * -> once it is started, it sends us a SIGUSR1, we need to setup a listener
 *    for it and when we receive it we can launch the WM
 * -> we need to track if the XWayland crashes, to restart it
 *
 * cf https://github.com/swaywm/wlroots/blob/master/xwayland/xwayland.c
 *
 * Since fork is not safe to call in a multi-threaded process, a process is
 * forked of early (LaunchHelper). Via a shared FD, a command to actually launch
 * Xwayland can be sent to that process. The process then does the fork and
 * reports back when Xwayland successfully started (=SIGUSR1 was received) with
 * another write on the pipe.
 */
use std::{
    any::Any,
    cell::RefCell,
    env,
    ffi::CString,
    os::unix::{
        io::{AsRawFd, IntoRawFd, RawFd},
        net::UnixStream,
    },
    rc::Rc,
    sync::Arc,
};

use calloop::{
    generic::{Fd, Generic},
    Interest, LoopHandle, Mode, Source,
};

use nix::Result as NixResult;

use wayland_server::{Client, Display, Filter};

use super::{
    x11_sockets::{prepare_x11_sockets, X11Lock},
    LaunchHelper,
};

/// The XWayland handle
pub struct XWayland<WM: XWindowManager> {
    inner: Rc<RefCell<Inner<WM>>>,
}

/// Trait to be implemented by you WM for XWayland
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
    /// Your privileged connection to it is this `UnixStream`
    fn xwayland_ready(&mut self, connection: UnixStream, client: Client);
    /// The XWayland server has exited
    fn xwayland_exited(&mut self);
}

impl<WM: XWindowManager + 'static> XWayland<WM> {
    /// Start the XWayland server
    pub fn init<L, T: Any, Data: 'static>(
        wm: WM,
        handle: LoopHandle<Data>,
        display: Rc<RefCell<Display>>,
        data: &mut T,
        logger: L,
        helper: LaunchHelper,
    ) -> Result<XWayland<WM>, ()>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger);
        let inner = Rc::new(RefCell::new(Inner {
            wm,
            kill_source: {
                let handle = handle.clone();
                Box::new(move |source| handle.kill(source))
            },
            source_maker: Box::new(move |inner, fd| {
                handle
                    .insert_source(
                        Generic::new(Fd(fd), Interest::Readable, Mode::Level),
                        move |evt, _, _| {
                            debug_assert!(evt.readable);
                            xwayland_ready(&inner);
                            Ok(())
                        },
                    )
                    .map_err(|_| ())
            }),
            wayland_display: display,
            instance: None,
            helper,
            log: log.new(o!("smithay_module" => "XWayland")),
        }));
        launch(&inner, data)?;
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
    startup_handler: Option<Source<Generic<Fd>>>,
    wm_fd: Option<UnixStream>,
    started_at: ::std::time::Instant,
}

type SourceMaker<WM> = dyn FnMut(Rc<RefCell<Inner<WM>>>, RawFd) -> Result<Source<Generic<Fd>>, ()>;

// Inner implementation of the XWayland manager
struct Inner<WM: XWindowManager> {
    wm: WM,
    source_maker: Box<SourceMaker<WM>>,
    wayland_display: Rc<RefCell<Display>>,
    instance: Option<XWaylandInstance>,
    kill_source: Box<dyn Fn(Source<Generic<Fd>>)>,
    helper: LaunchHelper,
    log: ::slog::Logger,
}

// Launch an XWayland server
//
// Does nothing if there is already a launched instance
fn launch<WM: XWindowManager + 'static, T: Any>(
    inner: &Rc<RefCell<Inner<WM>>>,
    data: &mut T,
) -> Result<(), ()> {
    let mut guard = inner.borrow_mut();
    if guard.instance.is_some() {
        return Ok(());
    }

    info!(guard.log, "Starting XWayland");

    let (x_wm_x11, x_wm_me) = UnixStream::pair().map_err(|_| ())?;
    let (wl_x11, wl_me) = UnixStream::pair().map_err(|_| ())?;

    let (lock, x_fds) = prepare_x11_sockets(guard.log.clone())?;

    // we have now created all the required sockets

    // record launch time
    let creation_time = ::std::time::Instant::now();

    // create the wayland client for XWayland
    let client = unsafe {
        guard
            .wayland_display
            .borrow_mut()
            .create_client(wl_me.into_raw_fd(), data)
    };
    client.data_map().insert_if_missing(|| inner.clone());
    client.add_destructor(Filter::new(|e: Arc<_>, _, mut data| {
        client_destroy::<WM, T>(&e, data.get().unwrap())
    }));

    // all is ready, we can do the fork dance
    match guard.helper.launch(lock.display(), wl_x11, x_wm_x11, &x_fds) {
        Ok(()) => {}
        Err(e) => {
            error!(guard.log, "Could not initiate launch of Xwayland"; "err" => format!("{:?}", e));
            return Err(());
        }
    }

    let status_fd = guard.helper.status_fd();
    let startup_handler = (&mut *guard.source_maker)(inner.clone(), status_fd)?;

    guard.instance = Some(XWaylandInstance {
        display_lock: lock,
        wayland_client: client,
        startup_handler: Some(startup_handler),
        wm_fd: Some(x_wm_me),
        started_at: creation_time,
    });

    Ok(())
}

impl<WM: XWindowManager> Inner<WM> {
    // Shutdown the XWayland server and cleanup everything
    fn shutdown(&mut self) {
        // don't do anything if not running
        if let Some(mut instance) = self.instance.take() {
            info!(self.log, "Shutting down XWayland.");
            self.wm.xwayland_exited();
            // kill the client
            instance.wayland_client.kill();
            // remove the event source
            if let Some(s) = instance.startup_handler.take() {
                (self.kill_source)(s);
            }
            // All connections and lockfiles are cleaned by their destructors

            // Remove DISPLAY from the env
            ::std::env::remove_var("DISPLAY");
            // We do like wlroots:
            // > We do not kill the XWayland process, it dies to broken pipe
            // > after we close our side of the wm/wl fds. This is more reliable
            // > than trying to kill something that might no longer be XWayland.
        }
    }
}

fn client_destroy<WM: XWindowManager + 'static, T: Any>(map: &::wayland_server::UserDataMap, data: &mut T) {
    let inner = map.get::<Rc<RefCell<Inner<WM>>>>().unwrap();

    // shutdown the server
    let started_at = inner.borrow().instance.as_ref().map(|i| i.started_at);
    inner.borrow_mut().shutdown();

    // restart it, unless we really just started it, if it crashes right
    // at startup there is no point
    if started_at.map(|t| t.elapsed().as_secs()).unwrap_or(10) > 5 {
        warn!(inner.borrow().log, "XWayland crashed, restarting.");
        let _ = launch(&inner, data);
    } else {
        warn!(
            inner.borrow().log,
            "XWayland crashed less than 5 seconds after its startup, not restarting."
        );
    }
}

fn xwayland_ready<WM: XWindowManager>(inner: &Rc<RefCell<Inner<WM>>>) {
    let mut guard = inner.borrow_mut();
    let inner = &mut *guard;
    // instance should never be None at this point
    let instance = inner.instance.as_mut().unwrap();
    let wm = &mut inner.wm;

    let success = match inner.helper.was_launch_succesful() {
        Ok(s) => s,
        Err(e) => {
            error!(inner.log, "Checking launch status failed"; "err" => format!("{:?}", e));
            false
        }
    };

    if success {
        // setup the environemnt
        ::std::env::set_var("DISPLAY", format!(":{}", instance.display_lock.display()));

        // signal the WM
        info!(inner.log, "XWayland is ready, signaling the WM.");
        wm.xwayland_ready(
            instance.wm_fd.take().unwrap(), // This is a bug if None
            instance.wayland_client.clone(),
        );
    } else {
        error!(
            inner.log,
            "XWayland crashed at startup, will not try to restart it."
        );
    }

    // in all cases, cleanup
    if let Some(s) = instance.startup_handler.take() {
        (inner.kill_source)(s);
    }
}

pub(crate) enum Void {}

/// Exec XWayland with given sockets on given display
///
/// If this returns, that means that something failed
pub(crate) fn exec_xwayland(
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
    // prepare the arguments to XWayland
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
    // the WAYLAND_SOCKET var tells XWayland where to connect as a wayland client
    env::set_var("WAYLAND_SOCKET", format!("{}", wayland_socket.as_raw_fd()));

    // ignore SIGUSR1, this will make the XWayland server send us this
    // signal when it is ready apparently
    unsafe {
        use nix::sys::signal::*;
        sigaction(
            Signal::SIGUSR1,
            &SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty()),
        )?;
    }

    // run it
    let ret = ::nix::unistd::execvp(
        &CString::new("Xwayland").unwrap(),
        &args.iter().map(AsRef::as_ref).collect::<Vec<_>>(),
    )?;
    // small dance to actually return Void
    match ret {}
}

/// Remove the `O_CLOEXEC` flag from this `Fd`
///
/// This means that the `Fd` will *not* be automatically
/// closed when we `exec()` into XWayland
fn unset_cloexec<F: AsRawFd>(fd: &F) -> NixResult<()> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    fcntl(fd.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::empty()))?;
    Ok(())
}
