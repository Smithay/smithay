#![warn(rust_2018_idioms)]

#[macro_use]
extern crate slog;

use std::{cell::RefCell, rc::Rc};

use slog::Drain;
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

mod drawing;
mod input_handler;
mod shell;
mod state;
#[cfg(feature = "udev")]
mod udev;
#[cfg(feature = "udev")]
mod raw;
mod window_map;
#[cfg(feature = "winit")]
mod winit;
#[cfg(feature = "xwayland")]
mod xwayland;

use state::AnvilState;

static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run anvil as a X11 or Wayland client using winit.",
    #[cfg(feature = "udev")]
    "--tty-udev : Run anvil as a tty udev client (requires root if without logind).",
    // dont advertise the "raw" backend
];

fn main() {
    // A logger facility, here we use the terminal here
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        //std::sync::Mutex::new(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let arg = ::std::env::args().nth(1);
    match arg.as_ref().map(|s| &s[..]) {
        #[cfg(feature = "winit")]
        Some("--winit") => {
            info!(log, "Starting anvil with winit backend");
            let mut event_loop = EventLoop::try_new().unwrap();
            let display = Rc::new(RefCell::new(Display::new()));
            if let Err(()) = winit::run_winit(display, &mut event_loop, log.clone()) {
                crit!(log, "Failed to initialize winit backend.");
            }
        }
        #[cfg(feature = "udev")]
        Some("--tty-udev") => {
            info!(log, "Starting anvil on a tty using udev");
            let mut event_loop = EventLoop::try_new().unwrap();
            let display = Rc::new(RefCell::new(Display::new()));
            if let Err(()) = udev::run_udev(display, &mut event_loop, log.clone()) {
                crit!(log, "Failed to initialize tty backend.");
            }
        }
        #[cfg(all(feature = "udev", debug_assertions))]
        Some("--raw") => {
            let device = ::std::env::args().nth(2).expect("Raw backend can only be used with a drm node argument");
            info!(log, "Starting raw backend on {:?}", device);
            let mut event_loop = EventLoop::try_new().unwrap();
            let display = Rc::new(RefCell::new(Display::new()));
            if let Err(()) = raw::run_raw(display, &mut event_loop, device, log.clone()) {
                crit!(log, "Failed to initialize tty backend.");
            }
        }
        _ => {
            println!("USAGE: anvil --backend");
            println!();
            println!("Possible backends are:");
            for b in POSSIBLE_BACKENDS {
                println!("\t{}", b);
            }
        }
    }
}
