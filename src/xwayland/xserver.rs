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
 * The XWayland server is spawned via an intermediate shell
 * -> wlroots does a double-fork while weston a single one, why ??
 *    -> https://stackoverflow.com/questions/881388/
 * -> once it is started, it will check if SIGUSR1 is set to ignored. If so,
 *    if will consider its parent as "smart", and send a SIGUSR1 signal when
 *    startup completes. We want to catch this so we can launch the VM.
 * -> we need to track if the XWayland crashes, to restart it
 *
 * cf https://github.com/swaywm/wlroots/blob/master/xwayland/xwayland.c
 *
 * Setting SIGUSR1 handler is complicated in multithreaded program, because
 * Xwayland will send SIGUSR1 to the process, and if a thread cannot handle
 * SIGUSR1, that thread will be killed.
 *
 * Double-fork can tackle this issue, but this is also very complex in a
 * a multithread program, after forking only signal-safe functions can be used.
 * The only workaround is to fork early before any other thread starts, but
 * doing so will expose an unsafe interface.
 *
 * We use an intermediate shell to translate the signal to simple fd IO.
 * We ask sh to setup SIGUSR1 handler, and in a subshell mute SIGUSR1 and exec
 * Xwayland. When the SIGUSR1 is received, it can communicate to us via redirected
 * STDOUT.
 */
use std::{
    any::Any,
    cell::RefCell,
    env,
    io::{Error as IOError, Read, Result as IOResult},
    os::unix::{
        io::{AsRawFd, IntoRawFd, RawFd},
        net::UnixStream,
        process::CommandExt,
    },
    process::{ChildStdout, Command, Stdio},
    rc::Rc,
    sync::Arc,
};

use calloop::{
    channel::{sync_channel, Channel, SyncSender},
    generic::{Fd, Generic},
    Interest, LoopHandle, Mode, RegistrationToken,
};

use slog::{error, info, o};

use nix::Error as NixError;

use wayland_server::{Client, Display, Filter};

use super::x11_sockets::{prepare_x11_sockets, X11Lock};

/// The XWayland handle
pub struct XWayland<Data> {
    inner: Rc<RefCell<Inner<Data>>>,
}

/// Events generated by the XWayland manager
///
/// This is a very low-level interface, only notifying you when the connection
/// with XWayland is up, or when it terminates.
///
/// Your WM code must be able handle the XWayland server connecting then
/// disconnecting several time in a row, but only a single connection will
/// be active at any given time.
pub enum XWaylandEvent {
    /// The XWayland server is ready
    Ready {
        /// Privileged X11 connection to XWayland
        connection: UnixStream,
        /// Wayland client representing XWayland
        client: Client,
    },
    /// The XWayland server exited
    Exited,
}

impl<Data: Any + 'static> XWayland<Data> {
    /// Create a new XWayland manager
    pub fn new<L>(
        handle: LoopHandle<'static, Data>,
        display: Rc<RefCell<Display>>,
        logger: L,
    ) -> (XWayland<Data>, XWaylandSource)
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger);
        // We don't expect to ever have more than 2 messages in flight, if XWayland got ready and then died right away
        let (sender, channel) = sync_channel(2);
        let inner = Rc::new(RefCell::new(Inner {
            handle,
            wayland_display: display,
            instance: None,
            sender,
            log: log.new(o!("smithay_module" => "XWayland")),
        }));
        (XWayland { inner }, XWaylandSource { channel })
    }

    /// Attempt to start the XWayland instance
    ///
    /// If it succeeds, you'll eventually receive an `XWaylandEvent::Ready`
    /// through the source provided by `XWayland::new()` containing an
    /// `UnixStream` representing your WM connection to XWayland, and the
    /// wayland `Client` for XWayland.
    ///
    /// Does nothing if XWayland is already started or starting.
    pub fn start(&self) -> std::io::Result<()> {
        launch(&self.inner)
    }

    /// Shutdown XWayland
    ///
    /// Does nothing if it was not already running, otherwise kills it and you will
    /// later receive a `XWaylandEvent::Exited` event.
    pub fn shutdown(&self) {
        self.inner.borrow_mut().shutdown();
    }
}

impl<Data> Drop for XWayland<Data> {
    fn drop(&mut self) {
        self.inner.borrow_mut().shutdown();
    }
}

struct XWaylandInstance {
    display_lock: X11Lock,
    wayland_client: Option<Client>,
    startup_handler: Option<RegistrationToken>,
    wm_fd: Option<UnixStream>,
    child_stdout: Option<ChildStdout>,
}

// Inner implementation of the XWayland manager
struct Inner<Data> {
    sender: SyncSender<XWaylandEvent>,
    handle: LoopHandle<'static, Data>,
    wayland_display: Rc<RefCell<Display>>,
    instance: Option<XWaylandInstance>,
    log: ::slog::Logger,
}

// Launch an XWayland server
//
// Does nothing if there is already a launched instance
fn launch<Data: Any>(inner: &Rc<RefCell<Inner<Data>>>) -> std::io::Result<()> {
    let mut guard = inner.borrow_mut();
    if guard.instance.is_some() {
        return Ok(());
    }

    info!(guard.log, "Starting XWayland");

    let (x_wm_x11, x_wm_me) = UnixStream::pair()?;
    let (wl_x11, wl_me) = UnixStream::pair()?;

    let (lock, x_fds) = prepare_x11_sockets(guard.log.clone())?;

    // we have now created all the required sockets

    // Setup the associated wayland client to be created in an idle callback, so that we don't need
    // to access the dispatch_data *right now*
    let idle_inner = inner.clone();
    guard.handle.insert_idle(move |data| {
        let mut guard = idle_inner.borrow_mut();
        let guard = &mut *guard;
        if let Some(ref mut instance) = guard.instance {
            // create the wayland client for XWayland
            let client = unsafe {
                guard
                    .wayland_display
                    .borrow_mut()
                    .create_client(wl_me.into_raw_fd(), data)
            };
            client.data_map().insert_if_missing(|| idle_inner.clone());
            client.add_destructor(Filter::new(|e: Arc<_>, _, _| client_destroy::<Data>(&e)));

            instance.wayland_client = Some(client);
        }
    });

    // all is ready, we can do the fork dance
    let child_stdout = match spawn_xwayland(lock.display(), wl_x11, x_wm_x11, &x_fds) {
        Ok(child_stdout) => child_stdout,
        Err(e) => {
            error!(guard.log, "XWayland failed to spawn"; "err" => format!("{:?}", e));
            return Err(e);
        }
    };

    let inner = inner.clone();
    let startup_handler = guard.handle.insert_source(
        Generic::new(Fd(child_stdout.as_raw_fd()), Interest::READ, Mode::Level),
        move |_, _, _| {
            // the closure must be called exactly one time, this cannot panic
            xwayland_ready(&inner);
            Ok(())
        },
    )?;

    guard.instance = Some(XWaylandInstance {
        display_lock: lock,
        startup_handler: Some(startup_handler),
        wayland_client: None,
        wm_fd: Some(x_wm_me),
        child_stdout: Some(child_stdout),
    });

    Ok(())
}

pub struct XWaylandSource {
    channel: Channel<XWaylandEvent>,
}

impl calloop::EventSource for XWaylandSource {
    type Event = XWaylandEvent;
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(
        &mut self,
        readiness: calloop::Readiness,
        token: calloop::Token,
        mut callback: F,
    ) -> std::io::Result<()>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.channel
            .process_events(readiness, token, |event, &mut ()| match event {
                calloop::channel::Event::Msg(msg) => callback(msg, &mut ()),
                calloop::channel::Event::Closed => {}
            })
    }

    fn register(&mut self, poll: &mut calloop::Poll, token: calloop::Token) -> std::io::Result<()> {
        self.channel.register(poll, token)
    }

    fn reregister(&mut self, poll: &mut calloop::Poll, token: calloop::Token) -> std::io::Result<()> {
        self.channel.reregister(poll, token)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> std::io::Result<()> {
        self.channel.unregister(poll)
    }
}

impl<Data> Inner<Data> {
    // Shutdown the XWayland server and cleanup everything
    fn shutdown(&mut self) {
        // don't do anything if not running
        if let Some(mut instance) = self.instance.take() {
            info!(self.log, "Shutting down XWayland.");
            // kill the client
            if let Some(client) = instance.wayland_client {
                client.kill();
            }
            // remove the event source
            if let Some(s) = instance.startup_handler.take() {
                self.handle.kill(s);
            }
            // send error occurs if the user dropped the channel... We cannot do much except ignore.
            let _ = self.sender.send(XWaylandEvent::Exited);

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

fn client_destroy<Data: 'static>(map: &::wayland_server::UserDataMap) {
    let inner = map.get::<Rc<RefCell<Inner<Data>>>>().unwrap();
    // If we are unable to take a lock we are most likely called during
    // a shutdown. This will definitely be the case when the compositor exits
    // and the XWayland instance is dropped.
    if let Ok(mut guard) = inner.try_borrow_mut() {
        guard.shutdown();
    }
}

fn xwayland_ready<Data: 'static>(inner: &Rc<RefCell<Inner<Data>>>) {
    // Lots of re-borrowing to please the borrow-checker
    let mut guard = inner.borrow_mut();
    let guard = &mut *guard;
    // instance should never be None at this point
    let instance = guard.instance.as_mut().unwrap();
    // neither the child_stdout
    let child_stdout = instance.child_stdout.as_mut().unwrap();

    // This reads the one byte that is written when sh receives SIGUSR1
    let mut buffer = [0];
    let success = match child_stdout.read(&mut buffer) {
        Ok(len) => len > 0 && buffer[0] == b'S',
        Err(e) => {
            error!(guard.log, "Checking launch status failed"; "err" => format!("{:?}", e));
            false
        }
    };

    if success {
        // setup the environemnt
        ::std::env::set_var("DISPLAY", format!(":{}", instance.display_lock.display()));

        // signal the WM
        info!(
            guard.log,
            "XWayland is ready on DISPLAY \":{}\", signaling the WM.",
            instance.display_lock.display()
        );
        // send error occurs if the user dropped the channel... We cannot do much except ignore.
        let _ = guard.sender.send(XWaylandEvent::Ready {
            connection: instance.wm_fd.take().unwrap(), // This is a bug if None
            client: instance.wayland_client.clone().unwrap(),
        });
    } else {
        error!(
            guard.log,
            "XWayland crashed at startup, will not try to restart it."
        );
    }

    // in all cases, cleanup
    if let Some(s) = instance.startup_handler.take() {
        guard.handle.kill(s);
    }
}

/// Spawn XWayland with given sockets on given display
///
/// Returns a pipe that outputs 'S' upon successful launch.
fn spawn_xwayland(
    display: u32,
    wayland_socket: UnixStream,
    wm_socket: UnixStream,
    listen_sockets: &[UnixStream],
) -> IOResult<ChildStdout> {
    let mut command = Command::new("sh");

    // We use output stream to communicate because FD is easier to handle than exit code.
    command.stdout(Stdio::piped());

    let mut xwayland_args = format!(":{} -rootless -terminate -wm {}", display, wm_socket.as_raw_fd());
    for socket in listen_sockets {
        xwayland_args.push_str(&format!(" -listen {}", socket.as_raw_fd()));
    }
    // This command let sh to:
    // * Set up signal handler for USR1
    // * Launch Xwayland with USR1 ignored so Xwayland will signal us when it is ready (also redirect
    //   Xwayland's STDOUT to STDERR so its output, if any, won't distract us)
    // * Print "S" and exit if USR1 is received
    command.arg("-c").arg(format!(
        "trap 'echo S' USR1; (trap '' USR1; exec Xwayland {}) 1>&2 & wait",
        xwayland_args
    ));

    // Setup the environment: clear everything except PATH and XDG_RUNTIME_DIR
    command.env_clear();
    for (key, value) in env::vars_os() {
        if key.to_str() == Some("PATH") || key.to_str() == Some("XDG_RUNTIME_DIR") {
            command.env(key, value);
            continue;
        }
    }
    command.env("WAYLAND_SOCKET", format!("{}", wayland_socket.as_raw_fd()));

    unsafe {
        let wayland_socket_fd = wayland_socket.as_raw_fd();
        let wm_socket_fd = wm_socket.as_raw_fd();
        let socket_fds: Vec<_> = listen_sockets.iter().map(|socket| socket.as_raw_fd()).collect();
        command.pre_exec(move || {
            // unset the CLOEXEC flag from the sockets we need to pass
            // to xwayland
            unset_cloexec(wayland_socket_fd)?;
            unset_cloexec(wm_socket_fd)?;
            for &socket in socket_fds.iter() {
                unset_cloexec(socket)?;
            }
            Ok(())
        });
    }

    let mut child = command.spawn()?;
    Ok(child.stdout.take().expect("stdout should be piped"))
}

fn nix_error_to_io(err: NixError) -> IOError {
    use std::io::ErrorKind;
    match err {
        NixError::Sys(errno) => errno.into(),
        NixError::InvalidPath | NixError::InvalidUtf8 => IOError::new(ErrorKind::InvalidInput, err),
        NixError::UnsupportedOperation => IOError::new(ErrorKind::Other, err),
    }
}

/// Remove the `O_CLOEXEC` flag from this `Fd`
///
/// This means that the `Fd` will *not* be automatically
/// closed when we `exec()` into XWayland
fn unset_cloexec(fd: RawFd) -> IOResult<()> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty())).map_err(nix_error_to_io)?;
    Ok(())
}
