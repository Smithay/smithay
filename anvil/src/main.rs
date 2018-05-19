#[macro_use]
extern crate glium;
extern crate rand;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
#[macro_use(define_roles)]
extern crate smithay;
extern crate xkbcommon;

use slog::Drain;
use smithay::wayland_server::Display;

#[macro_use]
mod shaders;
mod glium_drawer;
mod input_handler;
#[cfg(feature = "tty_launch")]
mod raw_drm;
mod shell;
mod shm_load;
#[cfg(feature = "udev")]
mod udev;
mod window_map;
#[cfg(feature = "winit")]
mod winit;

static POSSIBLE_BACKENDS: &'static [&'static str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run anvil as a X11 or Wayland client using winit.",
    #[cfg(feature = "tty_launch")]
    "--tty-raw : Run anvil as a raw DRM client (requires root).",
    #[cfg(feature = "udev")]
    "--tty-udev : Run anvil as a tty udev client (requires root if without logind).",
];

fn main() {
    // A logger facility, here we use the terminal here
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let (mut display, mut event_loop) = Display::new();

    let arg = ::std::env::args().skip(1).next();
    match arg.as_ref().map(|s| &s[..]) {
        #[cfg(feature = "winit")]
        Some("--winit") => {
            info!(log, "Starting anvil with winit backend");
            if let Err(()) = winit::run_winit(&mut display, &mut event_loop, log.clone()) {
                crit!(log, "Failed to initialize winit backend.");
            }
        }
        #[cfg(feature = "tty_launch")]
        Some("--tty-raw") => {
            info!(log, "Starting anvil on a tty using raw DRM");
            if let Err(()) = raw_drm::run_raw_drm(display, event_loop, log.clone()) {
                crit!(log, "Failed to initialize tty backend.");
            }
        }
        #[cfg(feature = "udev")]
        Some("--tty-udev") => {
            info!(log, "Starting anvil on a tty using udev");
            if let Err(()) = udev::run_udev(display, event_loop, log.clone()) {
                crit!(log, "Failed to initialize tty backend.");
            }
        }
        _ => {
            println!("USAGE: anvil --backend");
            println!("");
            println!("Possible backends are:");
            for b in POSSIBLE_BACKENDS {
                println!("\t{}", b);
            }
        }
    }
}
