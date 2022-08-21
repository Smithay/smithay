#![warn(rust_2018_idioms)]
// If no backend is enabled, a large portion of the codebase is unused.
// So silence this useless warning for the CI.
#![cfg_attr(
    not(any(feature = "winit", feature = "x11", feature = "udev")),
    allow(dead_code, unused_imports)
)]

#[macro_use]
extern crate slog;
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    // ptr::NonNull,
};

use cfg_if::cfg_if;
#[cfg(feature = "udev")]
pub mod cursor;
pub mod drawing;
pub mod input_handler;
pub mod render;
pub mod shell;
pub mod state;
#[cfg(feature = "udev")]
pub mod udev;
#[cfg(feature = "winit")]
pub mod winit;
#[cfg(feature = "x11")]
pub mod x11;
#[cfg(feature = "xwayland")]
pub mod xwayland;

use ndk::native_activity::NativeActivity;
pub use state::{AnvilState, CalloopData, ClientState};
// main.rs

use slog::{o, Drain};

#[cfg(not(target_os = "android"))]
use slog::crit;

cfg_if! {
    if #[cfg(not(target_os = "android"))] {
        static POSSIBLE_BACKENDS: &[&str] = &[
            #[cfg(feature = "winit")]
            "--winit : Run anvil as a X11 or Wayland client using winit.",
            #[cfg(feature = "udev")]
            "--tty-udev : Run anvil as a tty udev client (requires root if without logind).",
            #[cfg(feature = "x11")]
            "--x11 : Run anvil as an X11 client.",
        ];
    }
}

#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    // A logger facility, here we use the terminal here
    let log = if std::env::var("ANVIL_MUTEX_LOG").is_ok() {
        slog::Logger::root(std::sync::Mutex::new(slog_term::term_full().fuse()).fuse(), o!())
    } else {
        slog::Logger::root(
            slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
            o!(),
        )
    };

    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Could not setup log backend");

    cfg_if! {
        if #[cfg(not(target_os = "android"))] {
            let arg = ::std::env::args().nth(1);
            slog_stdlog::init().expect("Could not setup log backend");
            match arg.as_ref().map(|s| &s[..]) {
                #[cfg(feature = "winit")]
                Some("--winit") => {
                    slog::info!(log, "Starting anvil with winit backend");
                    crate::winit::run_winit(log);
                }
                #[cfg(feature = "udev")]
                Some("--tty-udev") => {
                    slog::info!(log, "Starting anvil on a tty using udev");
                    crate::udev::run_udev(log);
                }
                #[cfg(feature = "x11")]
                Some("--x11") => {
                    slog::info!(log, "Starting anvil with x11 backend");
                    crate::x11::run_x11(log);
                }
                Some(other) => {
                    crit!(log, "Unknown backend: {}", other);
                }
                None => {
                    slog::info!(log, "USAGE: anvil --backend");
                    slog::info!(log, );
                    slog::info!(log, "Possible backends are:");
                    for b in POSSIBLE_BACKENDS {
                        slog::info!(log, "\t{}", b);
                    }
                }
            }
        } else {
            slog::info!(log, "Starting anvil with android+winit backend");
            let activity = ndk_glue::native_activity();
            // For ndk_sys 0.6.x
            // let android_context = ndk_context::android_context();
            // let activity = unsafe { NativeActivity::from_ptr(NonNull::new(android_context.context() as *mut ndk_sys::ANativeActivity).unwrap()) };
            let data_dir = PathBuf::from(activity.internal_data_path().to_string_lossy().into_owned());
            let cache_dir = data_dir.join("cache");

            if !cache_dir.exists() {
                fs::create_dir(&cache_dir).unwrap();
            } else if !cache_dir.is_dir() {
                panic!("Cache dir {} is not a directory!!!", cache_dir.display());
            }
            let runtime_dir = cache_dir.join("run");

            if !runtime_dir.exists() {
                fs::create_dir_all(runtime_dir).unwrap();
            }
            std::env::set_var("XDG_RUNTIME_DIR", cache_dir.join("run"));

            let assets = activity.asset_manager();
            let version = env!("CARGO_PKG_VERSION");
            let lockfile_path = cache_dir.join(".version-lockfile");
            if {
                match File::open(lockfile_path) {
                        Ok(mut file) => {
                            let mut contents = String::new();
                file.read_to_string(&mut contents).unwrap();

                &contents != version
                        },
                        Err(err) => {
                            use std::io::ErrorKind::NotFound;

                            if err.kind() == NotFound {
                                false
                            } else {
                                panic!("{:#?}", err)
                            }
                        }
                }

            } {
                let x11_assets = assets.open_dir(&CString::new("x11").unwrap()).unwrap();
                let x11_path = cache_dir.join("x11");
                for path in x11_assets {
                    let mut asset = assets.open(&path).unwrap();
                    let mut file = File::create(x11_path.join(path.to_string_lossy().into_owned())).unwrap();
                    file.write(asset.get_buffer().unwrap()).unwrap();
                }

                let mut lockfile = File::create(cache_dir.join(".version-lockfile")).unwrap();

                write!(lockfile, "{version}").unwrap();
            }


            crate::winit::run_winit(log);
        }
    }
}
