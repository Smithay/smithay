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

fn main() {
    // A logger facility, here we use the terminal here
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let (mut display, mut event_loop) = Display::new();

    #[cfg(feature = "winit")]
    {
        if let Ok(()) = winit::run_winit(&mut display, &mut event_loop, log.clone()) {
            return;
        }
        warn!(log, "Failed to initialize winit backend, skipping.");
    }

    #[cfg(feature = "tty_launch")]
    {
        if let Ok(()) = udev::run_udev(display, event_loop, log.clone()) {
            return;
        }
        warn!(log, "Failed to initialize udev backend, skipping.");
    }

    error!(log, "Failed to initialize any backend.");
}
