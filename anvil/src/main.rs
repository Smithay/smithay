#![warn(rust_2018_idioms)]

#[macro_use]
extern crate glium;
#[macro_use]
extern crate slog;
#[macro_use(define_roles)]
extern crate smithay;

use slog::Drain;
use smithay::reexports::{
    calloop::{generic::Generic, mio::Interest, EventLoop},
    wayland_server::Display,
};

#[macro_use]
mod shaders;
mod buffer_utils;
mod glium_drawer;
mod input_handler;
mod shell;
mod shm_load;
#[cfg(feature = "udev")]
mod udev;
mod window_map;
#[cfg(feature = "winit")]
mod winit;

static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run anvil as a X11 or Wayland client using winit.",
    #[cfg(feature = "udev")]
    "--tty-udev : Run anvil as a tty udev client (requires root if without logind).",
];

pub struct AnvilState {
    pub need_wayland_dispatch: bool,
    pub running: bool,
}

impl Default for AnvilState {
    fn default() -> AnvilState {
        AnvilState {
            need_wayland_dispatch: false,
            running: true,
        }
    }
}

fn main() {
    // A logger facility, here we use the terminal here
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let mut event_loop = EventLoop::<AnvilState>::new().unwrap();
    let mut display = Display::new();

    // Glue for event dispatching
    let mut wayland_event_source = Generic::from_raw_fd(display.get_poll_fd());
    wayland_event_source.set_interest(Interest::READABLE);
    let _wayland_source =
        event_loop
            .handle()
            .insert_source(wayland_event_source, |_, state: &mut AnvilState| {
                state.need_wayland_dispatch = true;
            });

    let arg = ::std::env::args().nth(1);
    match arg.as_ref().map(|s| &s[..]) {
        #[cfg(feature = "winit")]
        Some("--winit") => {
            info!(log, "Starting anvil with winit backend");
            if let Err(()) = winit::run_winit(&mut display, &mut event_loop, log.clone()) {
                crit!(log, "Failed to initialize winit backend.");
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
            println!();
            println!("Possible backends are:");
            for b in POSSIBLE_BACKENDS {
                println!("\t{}", b);
            }
        }
    }
}
