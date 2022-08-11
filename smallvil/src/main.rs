#![allow(irrefutable_let_patterns)]

use slog::Drain;

mod handlers;

mod grabs;
mod input;
mod state;
mod winit;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::Smallvil;

pub struct CalloopData {
    state: Smallvil,
    display: Display<Smallvil>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log = ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), slog::o!());
    slog_stdlog::init()?;

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new()?;

    let mut display: Display<Smallvil> = Display::new()?;
    let state = Smallvil::new(&mut event_loop, &mut display, log.clone());

    let mut data = CalloopData { state, display };

    crate::winit::init_winit(&mut event_loop, &mut data, log)?;

    let mut args = std::env::args();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--client" => {
                if let Some(client) = args.next() {
                    std::process::Command::new(client).spawn().ok();
                } else {
                    std::process::Command::new("weston-terminal").spawn().ok();
                }
            }
            _ => {}
        }
    }

    event_loop.run(None, &mut data, move |_| {
        // Smallvil is running
    })?;

    Ok(())
}
