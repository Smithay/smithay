//! XWayland utilities
//!
//! This module contains helpers to manage XWayland from your compositor, in order
//! to support running X11 apps.
//!
//! The starting point is the [`XWayland`] struct, which represents the
//! running XWayland instance. Dropping it will shutdown XWayland.
//!
//! You need to provide an implementation of a X11 Window Manager for XWayland to
//! function properly. You'll need to treat XWayland (and all its X11 apps) as one
//! special client, and play the role of an X11 Window Manager.
//!
//! Smithay does not provide any helper for doing that yet, but it is planned.
mod x11_sockets;
mod xserver;
pub mod xwm;

pub use self::xserver::{XWayland, XWaylandClientData, XWaylandEvent};
pub use self::xwm::{X11Surface, X11Wm, XwmHandler};
