//! Utilities for handling shell surfaces with the `wl_shell` protocol
//!
//! This module provides automatic handling of shell surfaces objects, by being registered
//! as a global handler for `wl_shell`. This protocol is deprecated in favor of `xdg_shell`,
//! thus this module is provided as a compatibility layer with older clients. As a consequence,
//! you can as a compositor-writer decide to only support its functionality in a best-effort
//! maneer: as this global is part of the core protocol, you are still required to provide
//! some support for it.
//!
//! ## Why use this implementation
//!
//! This implementation can track for you the various shell surfaces defined by the
//! clients by handling the `wl_shell` protocol.
//!
//! It allows you to easily access a list of all shell surfaces defined by your clients
//! access their associated metadata and underlying `wl_surface`s.
//!
//! This handler only handles the protocol exchanges with the client to present you the
//! information in a coherent and relatively easy to use manner. All the actual drawing
//! and positioning logic of windows is out of its scope.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this handler, simple use the [`wl_shell_init`] function provided in this module.
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::wayland::shell::legacy::{wl_shell_init, ShellRequest};
//!
//! # let mut display = wayland_server::Display::new();
//! let (shell_state, _) = wl_shell_init(
//!     &mut display,
//!     // your implementation
//!     |event: ShellRequest| { /* ... */ },
//!     None  // put a logger if you want
//! );
//!
//! // You're now ready to go!
//! ```

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use crate::wayland::{compositor, Serial};

use wayland_server::{
    protocol::{wl_output, wl_seat, wl_shell, wl_shell_surface, wl_surface},
    Display, Filter, Global,
};

use super::PingError;

mod wl_handlers;

/// Metadata associated with the `wl_surface` role
#[derive(Debug)]
pub struct ShellSurfaceAttributes {
    /// Title of the surface
    pub title: String,
    /// Class of the surface
    pub class: String,
    pending_ping: Option<Serial>,
}

/// A handle to a shell surface
#[derive(Debug, Clone)]
pub struct ShellSurface {
    wl_surface: wl_surface::WlSurface,
    shell_surface: wl_shell_surface::WlShellSurface,
}

impl ShellSurface {
    /// Is the shell surface referred by this handle still alive?
    pub fn alive(&self) -> bool {
        self.shell_surface.as_ref().is_alive() && self.wl_surface.as_ref().is_alive()
    }

    /// Do this handle and the other one actually refer to the same shell surface?
    pub fn equals(&self, other: &Self) -> bool {
        self.shell_surface.as_ref().equals(&other.shell_surface.as_ref())
    }

    /// Access the underlying `wl_surface` of this toplevel surface
    ///
    /// Returns `None` if the toplevel surface actually no longer exists.
    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        if self.alive() {
            Some(&self.wl_surface)
        } else {
            None
        }
    }

    /// Send a ping request to this shell surface
    ///
    /// You'll receive the reply as a [`ShellRequest::Pong`] request
    ///
    /// A typical use is to start a timer at the same time you send this ping
    /// request, and cancel it when you receive the pong. If the timer runs
    /// down to 0 before a pong is received, mark the client as unresponsive.
    ///
    /// Fails if this shell client already has a pending ping or is already dead.
    pub fn send_ping(&self, serial: Serial) -> Result<(), PingError> {
        if !self.alive() {
            return Err(PingError::DeadSurface);
        }
        compositor::with_states(&self.wl_surface, |states| {
            let mut data = states
                .data_map
                .get::<Mutex<ShellSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if let Some(pending_ping) = data.pending_ping {
                return Err(PingError::PingAlreadyPending(pending_ping));
            }
            data.pending_ping = Some(serial);
            Ok(())
        })
        .unwrap()?;
        self.shell_surface.ping(serial.into());
        Ok(())
    }

    /// Send a configure event to this toplevel surface to suggest it a new configuration
    pub fn send_configure(&self, size: (u32, u32), edges: wl_shell_surface::Resize) {
        self.shell_surface.configure(edges, size.0 as i32, size.1 as i32)
    }

    /// Signal a popup surface that it has lost focus
    pub fn send_popup_done(&self) {
        self.shell_surface.popup_done()
    }
}

/// Possible kinds of shell surface of the `wl_shell` protocol
#[derive(Debug)]
pub enum ShellSurfaceKind {
    /// Toplevel, a regular window displayed somewhere in the compositor space
    Toplevel,
    /// Transient, this surface has a parent surface
    ///
    /// These are sub-windows of an application (for example a configuration window),
    /// and as such should only be visible in their parent window is, and on top of it.
    Transient {
        /// The surface considered as parent
        parent: wl_surface::WlSurface,
        /// Location relative to the parent
        location: (i32, i32),
        /// Wether this window should be marked as inactive
        inactive: bool,
    },
    /// Fullscreen surface, covering an entire output
    Fullscreen {
        /// Method used for fullscreen
        method: wl_shell_surface::FullscreenMethod,
        /// Framerate (relevant only for driver fullscreen)
        framerate: u32,
        /// Requested output if any
        output: Option<wl_output::WlOutput>,
    },
    /// A popup surface
    ///
    /// Short-lived surface, typically referred as "tooltips" in many
    /// contexts.
    Popup {
        /// The parent surface of this popup
        parent: wl_surface::WlSurface,
        /// The serial of the input event triggering the creation of this
        /// popup
        serial: Serial,
        /// Wether this popup should be marked as inactive
        inactive: bool,
        /// Location of the popup relative to its parent
        location: (i32, i32),
        /// Seat associated this the input that triggered the creation of the
        /// popup. Used to define when the "popup done" event is sent.
        seat: wl_seat::WlSeat,
    },
    /// A maximized surface
    ///
    /// Like a toplevel surface, but as big as possible on a single output
    /// while keeping any relevant desktop-environment interface visible.
    Maximized {
        /// Requested output for maximization
        output: Option<wl_output::WlOutput>,
    },
}

/// A request triggered by a `wl_shell_surface`
#[derive(Debug)]
pub enum ShellRequest {
    /// A new shell surface was created
    ///
    /// by default it has no kind and this should not be displayed
    NewShellSurface {
        /// The created surface
        surface: ShellSurface,
    },
    /// A pong event
    ///
    /// The surface responded to its pending ping. If you receive this
    /// event, smithay has already checked that the responded serial was valid.
    Pong {
        /// The surface that sent the pong
        surface: ShellSurface,
    },
    /// Start of an interactive move
    ///
    /// The surface requests that an interactive move is started on it
    Move {
        /// The surface requesting the move
        surface: ShellSurface,
        /// Serial of the implicit grab that initiated the move
        serial: Serial,
        /// Seat associated with the move
        seat: wl_seat::WlSeat,
    },
    /// Start of an interactive resize
    ///
    /// The surface requests that an interactive resize is started on it
    Resize {
        /// The surface requesting the resize
        surface: ShellSurface,
        /// Serial of the implicit grab that initiated the resize
        serial: Serial,
        /// Seat associated with the resize
        seat: wl_seat::WlSeat,
        /// Direction of the resize
        edges: wl_shell_surface::Resize,
    },
    /// The surface changed its kind
    SetKind {
        /// The surface
        surface: ShellSurface,
        /// Its new kind
        kind: ShellSurfaceKind,
    },
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
#[derive(Debug)]
pub struct ShellState {
    known_surfaces: Vec<ShellSurface>,
}

impl ShellState {
    /// Cleans the internal surface storage by removing all dead surfaces
    pub(crate) fn cleanup_surfaces(&mut self) {
        self.known_surfaces.retain(|s| s.alive());
    }

    /// Access all the shell surfaces known by this handler
    pub fn surfaces(&self) -> &[ShellSurface] {
        &self.known_surfaces[..]
    }
}

/// Create a new `wl_shell` global
pub fn wl_shell_init<L, Impl>(
    display: &mut Display,
    implementation: Impl,
    logger: L,
) -> (Arc<Mutex<ShellState>>, Global<wl_shell::WlShell>)
where
    L: Into<Option<::slog::Logger>>,
    Impl: FnMut(ShellRequest) + 'static,
{
    let _log = crate::slog_or_fallback(logger);

    let implementation = Rc::new(RefCell::new(implementation));

    let state = Arc::new(Mutex::new(ShellState {
        known_surfaces: Vec::new(),
    }));
    let state2 = state.clone();

    let global = display.create_global(
        1,
        Filter::new(move |(shell, _version), _, _data| {
            self::wl_handlers::implement_shell(shell, implementation.clone(), state2.clone());
        }),
    );

    (state, global)
}
