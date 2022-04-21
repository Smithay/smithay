use calloop::EventLoop;
use slog::Drain;

mod handlers;

mod grabs;
mod input;
mod state;
mod winit;

pub use state::Smallvil;
use wayland_server::Display;

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

    crate::winit::run_winit(&mut event_loop, &mut data, log)?;

    std::process::Command::new("weston-terminal").spawn().ok();

    event_loop.run(None, &mut data, move |_| {
        // Smallvil is running
    })?;

    Ok(())
}
