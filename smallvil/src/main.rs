#![allow(irrefutable_let_patterns)]

mod handlers;

mod grabs;
mod input;
mod state;
mod winit;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::Smallvil;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let mut event_loop: EventLoop<Smallvil> = EventLoop::try_new()?;

    let display: Display<Smallvil> = Display::new()?;

    let mut state = Smallvil::new(&mut event_loop, display);

    // Open a Wayland/X11 window for our nested compositor
    crate::winit::init_winit(&mut event_loop, &mut state)?;

    // Set WAYLAND_DISPLAY to our socket name, so child processes connect to Smallvil rather
    // than the host compositor
    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    // Spawn a test client, that will run under Smallvil
    spawn_client();

    event_loop.run(None, &mut state, move |_| {
        // Smallvil is running
    })?;

    Ok(())
}

fn init_logging() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
}

fn spawn_client() {
    let mut args = std::env::args().skip(1);
    let flag = args.next();
    let arg = args.next();

    match (flag.as_deref(), arg) {
        (Some("-c") | Some("--command"), Some(command)) => {
            std::process::Command::new(command).spawn().ok();
        }
        _ => {
            std::process::Command::new("weston-terminal").spawn().ok();
        }
    }
}
