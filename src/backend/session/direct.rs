use std::io::Result as IoResult;
use std::path::Path;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use nix::{Error as NixError, Result as NixResult};
use nix::fcntl::{self, open};
use nix::libc::c_int;
use nix::sys::signal::{self, Signal};
use nix::sys::stat::{dev_t, major, minor, Mode, fstat};
use nix::unistd::{dup, close};
use wayland_server::EventLoopHandle;
use wayland_server::sources::SignalEventSource;

#[cfg(feature = "backend_session_udev")]
use libudev::Context;

use super::{Session, SessionNotifier, SessionObserver};

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
    pub const VT_ACKACQ: i8 = 0x02;

    extern {
        pub fn __libc_current_sigrtmin() -> i8;
        pub fn __libc_current_sigrtmax() -> i8;
    }
}


// on freebsd and dragonfly
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
const DRM_MAJOR: u64 = 145;

// on netbsd
#[cfg(target_os = "netbsd")]
const DRM_MAJOR: u64 = 34;

// on openbsd (32 & 64 bit)
#[cfg(all(target_os = "openbsd", target_pointer_width = "32"))]
const DRM_MAJOR: u64 = 88;
#[cfg(all(target_os = "openbsd", target_pointer_width = "64"))]
const DRM_MAJOR: u64 = 87;

// on linux/android
#[cfg(any(target_os = "linux", target_os = "android"))]
const DRM_MAJOR: u64 = 226;

#[cfg(any(target_os = "linux", target_os = "android"))]
const TTY_MAJOR: u64 = 4;

#[cfg(not(any(target_os = "linux", target_os = "android")))]
const TTY_MAJOR: u64 = 0;

#[cfg(not(feature = "backend_session_udev"))]
fn is_drm_device(dev: dev_t, _path: &Path) -> bool {
    major(dev) == DRM_MAJOR
}

#[cfg(not(feature = "backend_session_udev"))]
fn is_tty_device(dev: dev_t, _path: Option<&Path>) -> bool {
    major(dev) == TTY_MAJOR
}

#[cfg(feature = "backend_session_udev")]
fn is_drm_device(dev: dev_t, path: &Path) -> bool {
    let udev = match Context::new() {
        Ok(context) => context,
        Err(_) => return major(dev) == DRM_MAJOR,
    };

    let device = match udev.device_from_syspath(path) {
        Ok(device) => device,
        Err(_) => return major(dev) == DRM_MAJOR,
    };

    if let Some(subsystem) = device.subsystem() {
        subsystem == "drm"
    } else {
        major(dev) == DRM_MAJOR
    }
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

pub struct DirectSession {
    tty: RawFd,
    active: Arc<AtomicBool>,
    vt: i32,
    old_keyboard_mode: i32,
    logger: ::slog::Logger,
}

pub struct DirectSessionNotifier {
    active: Arc<AtomicBool>,
    signals: Vec<Option<Box<SessionObserver>>>,
    signal: Signal,
    logger: ::slog::Logger,
}

impl DirectSession {
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

        let vt_num = minor(stat.st_dev) as i32 - 1;
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
            nix::sys::signal::SIGUSR1 as i32
        } else {
            tty::__libc_current_sigrtmin()
        };*/
        let signal = signal::SIGUSR1;

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
}

impl Session for DirectSession {
    type Error = NixError;

    fn open(&mut self, path: &Path) -> NixResult<RawFd> {
        open(path, fcntl::O_RDWR | fcntl::O_CLOEXEC | fcntl::O_NOCTTY | fcntl::O_NONBLOCK, Mode::empty())
    }

    fn close(&mut self, fd: RawFd) -> NixResult<()> {
        close(fd)
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    fn seat(&self) -> &str {
        // The VT api can only be used on seat0
        return "seat0"
    }

    fn change_vt(&mut self, vt_num: i32) -> NixResult<()> {
        unsafe { tty::vt_activate(self.tty, vt_num).map(|_| ()) }
    }
}

impl Drop for DirectSession {
    fn drop(&mut self) {
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

pub fn direct_session_bind<L>(notifier: DirectSessionNotifier, evlh: &mut EventLoopHandle, _logger: L)
    -> IoResult<SignalEventSource<DirectSessionNotifier>>
where
    L: Into<Option<::slog::Logger>>,
{
    let signal = notifier.signal;

    evlh.add_signal_event_source(|evlh, notifier, _| {
        if notifier.is_active() {
            for signal in &mut notifier.signals {
                if let &mut Some(ref mut signal) = signal {signal.pause(&mut evlh.state().as_proxy()); }
            }
            notifier.active.store(false, Ordering::SeqCst);
        } else {
            for signal in &mut notifier.signals {
                if let &mut Some(ref mut signal) = signal { signal.activate(&mut evlh.state().as_proxy()); }
            }
            notifier.active.store(true, Ordering::SeqCst);
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
