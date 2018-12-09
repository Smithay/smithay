//!
//! Implementation of the `Session` trait through the legacy vt kernel interface.
//!
//! This requires write permissions for the given tty device and any devices opened through this
//! interface. This means it will almost certainly require root permissions and not allow to run
//! the compositor as an unprivileged user. Use this session type *only* as a fallback or for testing,
//! if anything better is available.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize the session you may pass the path to any tty device, that shall be used.
//! If no path is given the tty used to start this compositor (if any) will be used.
//! A new session and its notifier will be returned.
//!
//! ```rust,no_run
//! extern crate smithay;
//!
//! use smithay::backend::session::direct::DirectSession;
//!
//! # fn main() {
//! let (session, mut notifier) = DirectSession::new(None, None).unwrap();
//! # }
//! ```
//!
//! ### Usage of the session
//!
//! The session may be used to open devices manually through the `Session` interface
//! or be passed to other objects that need it to open devices themselves.
//!
//! Examples for those are e.g. the `LibinputInputBackend` (its context might be initialized through a
//! `Session` via the `LibinputSessionInterface`) or the `UdevBackend`.
//!
//! In case you want to pass the same `Session` to multiple objects, `Session` is implement for
//! every `Rc<RefCell<Session>>` or `Arc<Mutex<Session>>`.
//!
//! ### Usage of the session notifier
//!
//! The notifier might be used to pause device access, when the session gets paused (e.g. by
//! switching the tty via `DirectSession::change_vt`) and to automatically enable it again,
//! when the session becomes active again.
//!
//! It is crucial to avoid errors during that state. Examples for object that might be registered
//! for notifications are the `Libinput` context, the `UdevBackend` or a `DrmDevice` (handled
//! automatically by the `UdevBackend`, if not done manually).

use super::{AsErrno, Session, SessionNotifier, SessionObserver};
use nix::{
    fcntl::{self, open, OFlag},
    libc::c_int,
    sys::{
        signal::{self, Signal},
        stat::{dev_t, fstat, major, minor, Mode},
    },
    unistd::{close, dup},
    Error as NixError, Result as NixResult,
};
use std::{
    cell::RefCell,
    io::Error as IoError,
    os::unix::io::RawFd,
    path::Path,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
#[cfg(feature = "backend_udev")]
use udev::Context;
use wayland_server::calloop::{signals::Signals, LoopHandle, Source};

#[allow(dead_code)]
mod tty {
    ioctl_read_bad!(kd_get_mode, 0x4B3B, i16);
    ioctl_write_int_bad!(kd_set_mode, 0x4B3A);
    pub const KD_TEXT: i16 = 0x00;
    pub const KD_GRAPHICS: i16 = 0x00;

    ioctl_read_bad!(kd_get_kb_mode, 0x4B44, i32);
    ioctl_write_int_bad!(kd_set_kb_mode, 0x4B45);
    pub const K_RAW: i32 = 0x00;
    pub const K_XLATE: i32 = 0x01;
    pub const K_MEDIUMRAW: i32 = 0x02;
    pub const K_UNICODE: i32 = 0x03;
    pub const K_OFF: i32 = 0x04;

    ioctl_write_int_bad!(vt_activate, 0x5606);
    ioctl_write_int_bad!(vt_wait_active, 0x5607);
    ioctl_write_ptr_bad!(vt_set_mode, 0x5602, VtMode);
    ioctl_write_int_bad!(vt_rel_disp, 0x5605);
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct VtMode {
        /// vt mode
        pub mode: i8,
        /// if set, hang on writes if not active
        pub waitv: i8,
        /// signal to raise on release req
        pub relsig: i16,
        /// signal to raise on acquisition
        pub acqsig: i16,
        /// unused (set to 0)
        pub frsig: i16,
    }
    pub const VT_AUTO: i8 = 0x00;
    pub const VT_PROCESS: i8 = 0x01;
    pub const VT_ACKACQ: i32 = 0x02;

    extern "C" {
        pub fn __libc_current_sigrtmin() -> i8;
        pub fn __libc_current_sigrtmax() -> i8;
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const TTY_MAJOR: u64 = 4;

#[cfg(not(any(target_os = "linux", target_os = "android")))]
const TTY_MAJOR: u64 = 0;

#[cfg(not(feature = "backend_udev"))]
fn is_tty_device(dev: dev_t, _path: Option<&Path>) -> bool {
    major(dev) == TTY_MAJOR
}

#[cfg(feature = "backend_udev")]
fn is_tty_device(dev: dev_t, path: Option<&Path>) -> bool {
    match path {
        Some(path) => {
            let udev = match Context::new() {
                Ok(context) => context,
                Err(_) => return major(dev) == TTY_MAJOR || minor(dev) != 0,
            };

            let device = match udev.device_from_syspath(path) {
                Ok(device) => device,
                Err(_) => return major(dev) == TTY_MAJOR || minor(dev) != 0,
            };

            let res = if let Some(subsystem) = device.subsystem() {
                subsystem == "tty"
            } else {
                major(dev) == TTY_MAJOR
            };
            res || minor(dev) != 0
        }
        None => major(dev) == TTY_MAJOR || minor(dev) != 0,
    }
}

/// `Session` via the virtual terminal direct kernel interface
pub struct DirectSession {
    tty: RawFd,
    active: Arc<AtomicBool>,
    vt: i32,
    old_keyboard_mode: i32,
    logger: ::slog::Logger,
}

/// `SessionNotifier` via the virtual terminal direct kernel interface
pub struct DirectSessionNotifier {
    tty: RawFd,
    active: Arc<AtomicBool>,
    signals: Vec<Option<Box<SessionObserver>>>,
    signal: Signal,
    logger: ::slog::Logger,
}

impl DirectSession {
    /// Tries to create a new session via the legacy virtual terminal interface.
    ///
    /// If you do not provide a tty device path, it will try to open the currently active tty if any.
    pub fn new<L>(tty: Option<&Path>, logger: L) -> Result<(DirectSession, DirectSessionNotifier)>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = ::slog_or_stdlog(logger)
            .new(o!("smithay_module" => "backend_session", "session_type" => "direct/vt"));

        let fd = tty
            .map(|path| {
                open(
                    path,
                    fcntl::OFlag::O_RDWR | fcntl::OFlag::O_CLOEXEC,
                    Mode::empty(),
                )
                .chain_err(|| ErrorKind::FailedToOpenTTY(String::from(path.to_string_lossy())))
            })
            .unwrap_or_else(|| {
                dup(0 /*stdin*/).chain_err(|| ErrorKind::FailedToOpenTTY(String::from("<stdin>")))
            })?;

        let active = Arc::new(AtomicBool::new(true));

        match DirectSession::setup_tty(tty, fd, logger.clone()) {
            Ok((vt, old_keyboard_mode, signal)) => Ok((
                DirectSession {
                    tty: fd,
                    active: active.clone(),
                    vt,
                    old_keyboard_mode,
                    logger: logger.new(o!("vt" => format!("{}", vt), "component" => "session")),
                },
                DirectSessionNotifier {
                    tty: fd,
                    active,
                    signals: Vec::new(),
                    signal,
                    logger: logger.new(o!("vt" => format!("{}", vt), "component" => "session_notifier")),
                },
            )),
            Err(err) => {
                let _ = close(fd);
                Err(err)
            }
        }
    }

    fn setup_tty(path: Option<&Path>, tty: RawFd, logger: ::slog::Logger) -> Result<(i32, i32, Signal)> {
        let stat = fstat(tty).chain_err(|| ErrorKind::NotRunningFromTTY)?;
        if !is_tty_device(stat.st_dev, path) {
            bail!(ErrorKind::NotRunningFromTTY);
        }

        let vt_num = minor(stat.st_rdev) as i32;
        info!(logger, "Running from tty: {}", vt_num);

        let mut mode = 0;
        unsafe {
            tty::kd_get_mode(tty, &mut mode).chain_err(|| ErrorKind::NotRunningFromTTY)?;
        }
        if mode != tty::KD_TEXT {
            bail!(ErrorKind::TTYAlreadyInGraphicsMode);
        }

        unsafe {
            tty::vt_activate(tty, vt_num as c_int).chain_err(|| ErrorKind::FailedToActivateTTY(vt_num))?;
            tty::vt_wait_active(tty, vt_num as c_int).chain_err(|| ErrorKind::FailedToWaitForTTY(vt_num))?;
        }

        let mut old_keyboard_mode = 0;
        unsafe {
            tty::kd_get_kb_mode(tty, &mut old_keyboard_mode)
                .chain_err(|| ErrorKind::FailedToSaveTTYState(vt_num))?;
            tty::kd_set_kb_mode(tty, tty::K_OFF).chain_err(|| ErrorKind::FailedToSetTTYKbMode(vt_num))?;
            tty::kd_set_mode(tty, tty::KD_GRAPHICS as i32)
                .chain_err(|| ErrorKind::FailedToSetTTYMode(vt_num))?;
        }

        // TODO: Support realtime signals
        // https://github.com/nix-rust/nix/issues/495
        /*
        let signal = if tty::__libc_current_sigrtmin() > tty::__libc_current_sigrtmax() {
            warn!(logger, "Not enough real-time signals available, falling back to USR1");
            nix::sys::signal::SIGUSR2 as i32
        } else {
            tty::__libc_current_sigrtmin()
        };*/
        let signal = signal::SIGUSR2;

        let mode = tty::VtMode {
            mode: tty::VT_PROCESS,
            relsig: signal as i16,
            acqsig: signal as i16,
            ..Default::default()
        };

        unsafe {
            tty::vt_set_mode(tty, &mode).chain_err(|| ErrorKind::FailedToTakeControlOfTTY(vt_num))?;
        }

        Ok((vt_num, old_keyboard_mode, signal))
    }

    /// Get the number of the virtual terminal used by this session
    pub fn vt(&self) -> i32 {
        self.vt
    }
}

impl Session for DirectSession {
    type Error = NixError;

    fn open(&mut self, path: &Path, flags: OFlag) -> NixResult<RawFd> {
        debug!(self.logger, "Opening device: {:?}", path);
        let fd = open(path, flags, Mode::empty())?;
        trace!(self.logger, "Fd num: {:?}", fd);
        Ok(fd)
    }

    fn close(&mut self, fd: RawFd) -> NixResult<()> {
        debug!(self.logger, "Closing device: {:?}", fd);
        close(fd)
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    fn seat(&self) -> String {
        // The VT API can only be used on seat0
        String::from("seat0")
    }

    fn change_vt(&mut self, vt_num: i32) -> NixResult<()> {
        unsafe { tty::vt_activate(self.tty, vt_num).map(|_| ()) }
    }
}

impl AsErrno for NixError {
    fn as_errno(&self) -> Option<i32> {
        match *self {
            NixError::Sys(errno) => Some(errno as i32),
            _ => None,
        }
    }
}

impl Drop for DirectSession {
    fn drop(&mut self) {
        info!(self.logger, "Deallocating tty {}", self.tty);

        if let Err(err) = unsafe { tty::kd_set_kb_mode(self.tty, self.old_keyboard_mode) } {
            warn!(self.logger, "Unable to restore vt keyboard mode. Error: {}", err);
        }
        if let Err(err) = unsafe { tty::kd_set_mode(self.tty, tty::KD_TEXT as i32) } {
            warn!(self.logger, "Unable to restore vt text mode. Error: {}", err);
        }
        if let Err(err) = unsafe {
            tty::vt_set_mode(
                self.tty,
                &tty::VtMode {
                    mode: tty::VT_AUTO,
                    ..Default::default()
                },
            )
        } {
            error!(self.logger, "Failed to reset vt handling. Error: {}", err);
        }
        if let Err(err) = close(self.tty) {
            error!(self.logger, "Failed to close tty file descriptor. Error: {}", err);
        }
    }
}

/// Ids of registered `SessionObserver`s of the `DirectSessionNotifier`
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Id(usize);

impl SessionNotifier for DirectSessionNotifier {
    type Id = Id;

    fn register<S: SessionObserver + 'static>(&mut self, signal: S) -> Self::Id {
        self.signals.push(Some(Box::new(signal)));
        Id(self.signals.len() - 1)
    }
    fn unregister(&mut self, signal: Id) {
        self.signals[signal.0] = None;
    }
}

impl DirectSessionNotifier {
    fn signal_received(&mut self) {
        if self.active.load(Ordering::SeqCst) {
            info!(self.logger, "Session shall become inactive.");
            for signal in &mut self.signals {
                if let Some(ref mut signal) = *signal {
                    signal.pause(None);
                }
            }
            self.active.store(false, Ordering::SeqCst);
            unsafe {
                tty::vt_rel_disp(self.tty, 1).expect("Unable to release tty lock");
            }
            debug!(self.logger, "Session is now inactive");
        } else {
            debug!(self.logger, "Session will become active again");
            unsafe {
                tty::vt_rel_disp(self.tty, tty::VT_ACKACQ).expect("Unable to acquire tty lock");
            }
            for signal in &mut self.signals {
                if let Some(ref mut signal) = *signal {
                    signal.activate(None);
                }
            }
            self.active.store(true, Ordering::SeqCst);
            info!(self.logger, "Session is now active again");
        }
    }
}

/// Bound logind session that is driven by the `wayland_server::EventLoop`.
///
/// See `direct_session_bind` for details.
pub struct BoundDirectSession {
    source: Source<Signals>,
    notifier: Rc<RefCell<DirectSessionNotifier>>,
}

impl BoundDirectSession {
    /// Unbind the direct session from the `EventLoop`
    pub fn unbind(self) -> DirectSessionNotifier {
        let BoundDirectSession { source, notifier } = self;
        source.remove();
        match Rc::try_unwrap(notifier) {
            Ok(notifier) => notifier.into_inner(),
            Err(_) => panic!("Notifier should have been freed from the event loop!"),
        }
    }
}

/// Bind a `DirectSessionNotifier` to an `EventLoop`.
///
/// Allows the `DirectSessionNotifier` to listen for incoming signals signalling the session state.
/// If you don't use this function `DirectSessionNotifier` will not correctly tell you the current
/// session state and call it's `SessionObservers`.
pub fn direct_session_bind<Data: 'static>(
    notifier: DirectSessionNotifier,
    handle: &LoopHandle<Data>,
) -> ::std::result::Result<BoundDirectSession, (IoError, DirectSessionNotifier)> {
    let signal = notifier.signal;
    let source = match Signals::new(&[signal]) {
        Ok(s) => s,
        Err(e) => return Err((e, notifier)),
    };
    let notifier = Rc::new(RefCell::new(notifier));
    let fail_notifier = notifier.clone();
    let source = handle
        .insert_source(source, {
            let notifier = notifier.clone();
            move |_, _| notifier.borrow_mut().signal_received()
        })
        .map_err(move |e| {
            // the backend in the closure should already have been dropped
            let notifier = Rc::try_unwrap(fail_notifier)
                .unwrap_or_else(|_| unreachable!())
                .into_inner();
            (e.into(), notifier)
        })?;
    Ok(BoundDirectSession { source, notifier })
}

error_chain! {
    errors {
        #[doc = "Failed to open tty"]
        FailedToOpenTTY(path: String) {
            description("Failed to open tty"),
            display("Failed to open tty ({:?})", path),
        }

        #[doc = "Not running from a tty"]
        NotRunningFromTTY {
            description("Not running from a tty"),
        }

        #[doc = "tty is already in KB_GRAPHICS mode"]
        TTYAlreadyInGraphicsMode {
            description("The tty is already in KB_GRAPHICS mode"),
            display("The tty is already in graphics mode, is already a compositor running?"),
        }

        #[doc = "Failed to activate open tty"]
        FailedToActivateTTY(num: i32) {
            description("Failed to activate open tty"),
            display("Failed to activate open tty ({:?})", num),
        }

        #[doc = "Failed to wait for tty to become active"]
        FailedToWaitForTTY(num: i32) {
            description("Failed to wait for tty to become active"),
            display("Failed to wait for tty ({:?}) to become active", num),
        }

        #[doc = "Failed to save old tty state"]
        FailedToSaveTTYState(num: i32) {
            description("Failed to save old tty state"),
            display("Failed to save old tty ({:?}) state", num),
        }

        #[doc = "Failed to set tty kb mode"]
        FailedToSetTTYKbMode(num: i32) {
            description("Failed to set tty kb mode to K_OFF"),
            display("Failed to set tty ({:?}) kb mode to K_OFF", num),
        }

        #[doc = "Failed to set tty mode"]
        FailedToSetTTYMode(num: i32) {
            description("Failed to set tty mode to KD_GRAPHICS"),
            display("Failed to set tty ({:?}) mode into graphics mode", num),
        }

        #[doc = "Failed to set tty in process mode"]
        FailedToTakeControlOfTTY(num: i32) {
            description("Failed to set tty mode to VT_PROCESS"),
            display("Failed to take control of tty ({:?})", num),
        }
    }
}
