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

    let mut args = std::env::args().skip(1);
    let flag = args.next();
    let arg = args.next();

    match (flag.as_deref(), arg) {
        (Some("-c") | Some("--command"), Some(command)) => {
            std::process::Command::new(command).spawn().ok();
        }
        _ => {
            std::process::Command::new(detect_terminal()).spawn().ok();
        }
    }

    event_loop.run(None, &mut data, move |_| {
        // Smallvil is running
    })?;

    Ok(())
}

fn detect_terminal() -> String {
    use std::fs::read_link;

    const SYMLINK: &str = "/usr/bin/x-terminal-emulator";

    if let Ok(found) = read_link(SYMLINK) {
        return read_link(&found).unwrap_or(found).to_string_lossy().to_string();
    }

    "/usr/bin/gnome-terminal".to_string()
}
