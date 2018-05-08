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

mod glium_drawer;
mod shell;
#[cfg(feature = "tty_launch")]
mod udev;
mod window_map;
#[cfg(feature = "winit")]
mod winit;
mod input_handler;

static POSSIBLE_BACKENDS: &'static [&'static str] = &[
    #[cfg(feature = "winit")]
    "--winit",
    #[cfg(feature = "tty_launch")]
    "--tty",
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
        Some("--tty") => {
            info!(log, "Starting anvil on a tty");
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
