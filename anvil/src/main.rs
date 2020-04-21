#![warn(rust_2018_idioms)]

#[macro_use]
extern crate glium;
#[macro_use]
extern crate slog;
#[macro_use(define_roles)]
extern crate smithay;

use std::{cell::RefCell, rc::Rc};

use slog::Drain;
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

#[macro_use]
mod shaders;
mod buffer_utils;
mod glium_drawer;
mod input_handler;
mod shell;
mod shm_load;
mod state;
#[cfg(feature = "udev")]
mod udev;
mod window_map;
#[cfg(feature = "winit")]
mod winit;

use state::AnvilState;

static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run anvil as a X11 or Wayland client using winit.",
    #[cfg(feature = "udev")]
    "--tty-udev : Run anvil as a tty udev client (requires root if without logind).",
];

fn main() {
    // A logger facility, here we use the terminal here
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let mut event_loop = EventLoop::<AnvilState>::new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let arg = ::std::env::args().nth(1);
    match arg.as_ref().map(|s| &s[..]) {
        #[cfg(feature = "winit")]
        Some("--winit") => {
            info!(log, "Starting anvil with winit backend");
            if let Err(()) = winit::run_winit(display, &mut event_loop, log.clone()) {
                crit!(log, "Failed to initialize winit backend.");
            }
        }
        #[cfg(feature = "udev")]
        Some("--tty-udev") => {
            info!(log, "Starting anvil on a tty using udev");
            if let Err(()) = udev::run_udev(display, &mut event_loop, log.clone()) {
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
