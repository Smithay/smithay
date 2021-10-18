use slog::{crit, o, Drain};

static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run anvil as a X11 or Wayland client using winit.",
    #[cfg(feature = "udev")]
    "--tty-udev : Run anvil as a tty udev client (requires root if without logind).",
    #[cfg(feature = "x11")]
    "--x11 : Run anvil as an X11 client.",
];

fn main() {
    // A logger facility, here we use the terminal here
    let log = if std::env::var("ANVIL_MUTEX_LOG").is_ok() {
        slog::Logger::root(std::sync::Mutex::new(slog_term::term_full().fuse()).fuse(), o!())
    } else {
        slog::Logger::root(
            slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
            o!(),
        )
    };

    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Could not setup log backend");

    let arg = ::std::env::args().nth(1);
    match arg.as_ref().map(|s| &s[..]) {
        #[cfg(feature = "winit")]
        Some("--winit") => {
            slog::info!(log, "Starting anvil with winit backend");
            anvil::winit::run_winit(log);
        }
        #[cfg(feature = "udev")]
        Some("--tty-udev") => {
            slog::info!(log, "Starting anvil on a tty using udev");
            anvil::udev::run_udev(log);
        }
        #[cfg(feature = "x11")]
        Some("--x11") => {
            slog::info!(log, "Starting anvil with x11 backend");
            anvil::x11::run_x11(log);
        }
        Some(other) => {
            crit!(log, "Unknown backend: {}", other);
        }
        None => {
            println!("USAGE: anvil --backend");
            println!();
            println!("Possible backends are:");
            for b in POSSIBLE_BACKENDS {
                println!("\t{}", b);
            }
        }
    }
}
