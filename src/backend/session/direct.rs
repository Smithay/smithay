//!
//! Implementation of the `Session` trait through the legacy vt kernel interface.
//!
//! This requires write permissions for the given tty device and any devices opened through this
//! interface. This means it will almost certainly require root permissions and not allow to run
//! the compositor as an unpriviledged user. Use this session type *only* as a fallback or for testing,
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
//! or be passed to other object that need to open devices themselves.
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
//! It is crutial to avoid errors during that state. Examples for object that might be registered
//! for notifications are the `Libinput` context, the `UdevBackend` or a `DrmDevice` (handled
//! automatically by the `UdevBackend`, if not done manually).
//! ```

use std::io::Result as IoResult;
use std::path::Path;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use nix::{Error as NixError, Result as NixResult};
use nix::fcntl::{self, open, OFlag};
use nix::libc::c_int;
use nix::sys::signal::{self, Signal};
use nix::sys::stat::{dev_t, major, minor, Mode, fstat};
use nix::unistd::{dup, close};
use wayland_server::EventLoopHandle;
use wayland_server::sources::SignalEventSource;

#[cfg(feature = "backend_session_udev")]
use libudev::Context;

use super::{AsErrno, Session, SessionNotifier, SessionObserver};

#[allow(dead_code)]
mod tty {
    ioctl!(bad read kd_get_mode with 0x4B3B; i16);
    ioctl!(bad write_int kd_set_mode with 0x4B3A);
    pub const KD_TEXT: i16 = 0x00;
    pub const KD_GRAPHICS: i16 = 0x00;

    ioctl!(bad read kd_get_kb_mode with 0x4B44; i32);
    ioctl!(bad write_int kd_set_kb_mode with 0x4B45);
    pub const K_RAW: i32 = 0x00;
    pub const K_XLATE: i32 = 0x01;
    pub const K_MEDIUMRAW: i32 = 0x02;
    pub const K_UNICODE: i32 = 0x03;
    pub const K_OFF: i32 = 0x04;

    ioctl!(bad write_int vt_activate with 0x5606);
    ioctl!(bad write_int vt_wait_active with 0x5607);
    ioctl!(bad write_ptr vt_set_mode with 0x5602; VtMode);
    ioctl!(bad write_int vt_rel_disp with 0x5605);
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

    extern {
        pub fn __libc_current_sigrtmin() -> i8;
        pub fn __libc_current_sigrtmax() -> i8;
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const TTY_MAJOR: u64 = 4;

#[cfg(not(any(target_os = "linux", target_os = "android")))]
const TTY_MAJOR: u64 = 0;

#[cfg(not(feature = "backend_session_udev"))]
fn is_tty_device(dev: dev_t, _path: Option<&Path>) -> bool {
    major(dev) == TTY_MAJOR
}

#[cfg(feature = "backend_session_udev")]
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
        },
        None => major(dev) == TTY_MAJOR || minor(dev) != 0
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
    /// Tries to creates a new session via the legacy virtual terminal interface.
    ///
    /// If you do not provide a tty device path, it will try to open the currently active tty if any.
    pub fn new<L>(tty: Option<&Path>, logger: L) -> Result<(DirectSession, DirectSessionNotifier)>
        where
            L: Into<Option<::slog::Logger>>
    {
        let logger = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_session", "session_type" => "direct/vt"));

        let fd = tty
            .map(|path| open(path, fcntl::O_RDWR | fcntl::O_CLOEXEC, Mode::empty())
                .chain_err(|| ErrorKind::FailedToOpenTTY(String::from(path.to_string_lossy()))))
            .unwrap_or(dup(0 /*stdin*/).chain_err(|| ErrorKind::FailedToOpenTTY(String::from("<stdin>"))))?;

        let active = Arc::new(AtomicBool::new(true));

        match DirectSession::setup_tty(tty, fd, logger.clone()) {
            Ok((vt, old_keyboard_mode, signal)) => {
                Ok((DirectSession {
                    tty: fd,
                    active: active.clone(),
                    vt,
                    old_keyboard_mode,
                    logger: logger.new(o!("vt" => format!("{}", vt), "component" => "session")),
                }, DirectSessionNotifier {
                    tty: fd,
                    active,
                    signals: Vec::new(),
                    signal,
                    logger: logger.new(o!("vt" => format!("{}", vt), "component" => "session_notifier"))
                }))
            },
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
            tty::kd_get_kb_mode(tty, &mut old_keyboard_mode).chain_err(|| ErrorKind::FailedToSaveTTYState(vt_num))?;
            tty::kd_set_kb_mode(tty, tty::K_OFF).chain_err(|| ErrorKind::FailedToSetTTYKbMode(vt_num))?;
            tty::kd_set_mode(tty, tty::KD_GRAPHICS as i32).chain_err(|| ErrorKind::FailedToSetTTYMode(vt_num))?;
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
        // The VT api can only be used on seat0
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
        if let Err(err) = unsafe { tty::vt_set_mode(self.tty, &tty::VtMode {
            mode: tty::VT_AUTO,
            ..Default::default()
        }) } {
            error!(self.logger, "Failed to reset vt handling. Error: {}", err);
        }
        if let Err(err) = close(self.tty) {
            error!(self.logger, "Failed to close tty file descriptor. Error: {}", err);
        }
    }
}

impl SessionNotifier for DirectSessionNotifier {
    fn register<S: SessionObserver + 'static>(&mut self, signal: S) -> usize {
        self.signals.push(Some(Box::new(signal)));
        self.signals.len() - 1
    }
    fn unregister(&mut self, signal: usize) {
        self.signals[signal] = None;
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
    fn seat(&self) -> &str {
        "seat0"
    }
}

/// Bind a `DirectSessionNotifier` to an `EventLoop`.
///
/// Allows the `DirectSessionNotifier` to listen for the incoming signals signalling the session state.
/// If you don't use this function `DirectSessionNotifier` will not correctly tell you the current
/// session state.
pub fn direct_session_bind<L>(notifier: DirectSessionNotifier, evlh: &mut EventLoopHandle, _logger: L)
    -> IoResult<SignalEventSource<DirectSessionNotifier>>
where
    L: Into<Option<::slog::Logger>>,
{
    let signal = notifier.signal;

    evlh.add_signal_event_source(|evlh, notifier, _| {
        if notifier.is_active() {
            info!(notifier.logger, "Session shall become inactive");
            for signal in &mut notifier.signals {
                if let &mut Some(ref mut signal) = signal {signal.pause(&mut evlh.state().as_proxy()); }
            }
            notifier.active.store(false, Ordering::SeqCst);
            unsafe {
                tty::vt_rel_disp(notifier.tty, 1).expect("Unable to release tty lock");
            }
            debug!(notifier.logger, "Session is now inactive");
        } else {
            debug!(notifier.logger, "Session will become active again");
            unsafe {
                tty::vt_rel_disp(notifier.tty, tty::VT_ACKACQ).expect("Unable to acquire tty lock");
            }
            for signal in &mut notifier.signals {
                if let &mut Some(ref mut signal) = signal { signal.activate(&mut evlh.state().as_proxy()); }
            }
            notifier.active.store(true, Ordering::SeqCst);
            info!(notifier.logger, "Session is now active again");
        }
    }, notifier, signal)
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
