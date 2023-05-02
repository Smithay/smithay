static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run anvil as a X11 or Wayland client using winit.",
    #[cfg(feature = "udev")]
    "--tty-udev : Run anvil as a tty udev client (requires root if without logind).",
    #[cfg(feature = "x11")]
    "--x11 : Run anvil as an X11 client.",
];

fn main() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt()
            .compact()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().compact().init();
    }

    let arg = ::std::env::args().nth(1);
    let backend_args = ::std::env::args().skip(2);
    match arg.as_ref().map(|s| &s[..]) {
        #[cfg(feature = "winit")]
        Some("--winit") => {
            tracing::info!("Starting anvil with winit backend");
            anvil::winit::run_winit(backend_args);
        }
        #[cfg(feature = "udev")]
        Some("--tty-udev") => {
            tracing::info!("Starting anvil on a tty using udev");
            anvil::udev::run_udev(backend_args);
        }
        #[cfg(feature = "x11")]
        Some("--x11") => {
            tracing::info!("Starting anvil with x11 backend");
            anvil::x11::run_x11(backend_args);
        }
        Some(other) => {
            tracing::error!("Unknown backend: {}", other);
        }
        None => {
            println!("USAGE: anvil --backend");
            println!();
            println!("Possible backends are:");
            for b in POSSIBLE_BACKENDS {
                println!("\t{}", b);
            }
        }
    }
}
