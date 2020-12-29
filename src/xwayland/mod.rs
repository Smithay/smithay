//! XWayland utilities
//!
//! This module contains helpers to manage XWayland from your compositor, in order
//! to support running X11 apps.
//!
//! The starting point is the [`XWayland`](struct.XWayland.html) struct, which represents the
//! running `XWayland` instance. Dropping it will shutdown XWayland.
//!
//! You need to provide an implementation of the `XWindowManager` trait which gives you
//! access to the X11 WM connection and the `Client` associated with XWayland. You'll need
//! to treat XWayland (and all its X11 apps) as one special client, and play the role of
//! an X11 Window Manager.
//!
//! Smithay does not provide any helper for doing that yet, but it is planned.

mod launch_helper;
mod x11_sockets;
mod xserver;

pub use self::launch_helper::LaunchHelper;
pub use self::xserver::{XWayland, XWindowManager};
