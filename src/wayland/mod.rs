//! Protocol-related utilities
//!
//! This module contains several handlers to manage the Wayland protocol
//! and the clients.
//!
//! ## General structure
//!
//! Most utilities provided in this module work in the same way:
//!
//! - A module specific `*State` struct will take the wayland display as argument and
//!   insert one or more globals into it through its constructor.
//! - The module-`State` will have to be stored inside your global compositor state.
//!   (The same type you parametrized [`wayland_server::Display`] over.)
//! - You need to implement a module-specific `*Handler`-trait for your compositor state.
//!   This implementation will be called when wayland events require custom handling.
//! - Call the matching `delegate_*!` macro from smithay on your state to implement
//!   some required `wayland_server` traits.
//! - If you want to remove a previously inserted global, just drop the `*State`.
//!
//! ## Provided helpers
//!
//! ### Core functionality
//!
//! The most fundamental module is the [`compositor`] module, which provides the necessary
//! logic to handle the fundamental component by which clients build their windows: surfaces.
//! Following this, the [`shell`] module contains the logic allowing clients to use their
//! surface to build concrete windows with the usual interactions. Different kind of shells
//! exist, but in general you will want to support at least the [`xdg`](shell::xdg) variant,
//! which is the standard used by most applications.
//!
//! Then, the [`seat`] module contains logic related to input handling. These helpers are used
//! to forward input (such as pointer action or keystrokes) to clients, and manage the input
//! focus of clients. Tightly coupled with it is the [`selection`] module, which handles
//! cross-client interactions such as accessing the clipboard, or drag'n'drop actions.
//!
//! The [`shm`] module provides the necessary logic for client to provide buffers defining the
//! contents of their windows using shared memory. This is the main mechanism used by clients
//! that are not hardware accelerated. As a complement, the [`dmabuf`] module provides support
//! hardware-accelerated clients; it is tightly linked to the
//! [`backend::allocator`](crate::backend::allocator) module.
//!
//! The [`output`] module helps forwarding to clients information about the display monitors that
//! are available. This notably plays a key role in HiDPI handling, and more generally notifying
//! clients about whether they are currently visible or not (allowing them to stop drawing if they
//! are not, for example).
//!

pub mod buffer;
pub mod compositor;
pub mod content_type;
pub mod cursor_shape;
pub mod dmabuf;
#[cfg(feature = "backend_drm")]
pub mod drm_lease;
pub mod fractional_scale;
pub mod idle_inhibit;
pub mod idle_notify;
pub mod input_method;
pub mod keyboard_shortcuts_inhibit;
pub mod output;
pub mod pointer_constraints;
pub mod pointer_gestures;
pub mod presentation;
pub mod relative_pointer;
pub mod seat;
pub mod security_context;
pub mod selection;
pub mod session_lock;
pub mod shell;
pub mod shm;
pub mod socket;
pub mod tablet_manager;
pub mod text_input;
pub mod viewporter;
pub mod virtual_keyboard;
pub mod xdg_activation;
pub mod xdg_foreign;
#[cfg(feature = "xwayland")]
pub mod xwayland_keyboard_grab;
#[cfg(feature = "xwayland")]
pub mod xwayland_shell;
