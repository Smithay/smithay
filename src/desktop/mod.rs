//! Desktop management helpers
//!
//! This module contains helpers to organize and interact with desktop-style shells.
//!
//! It is therefore a lot more opinionated than for example the [xdg-shell handler](crate::wayland::shell::xdg::XdgShellHandler)
//! and tightly integrates with some protocols (e.g. xdg-shell).
//!
//! The usage of this module is therefor entirely optional and depending on your use-case you might also only want
//! to use a limited set of the helpers provided.
//!
//! ## Helpers
//!
//! ### [`Window`]
//!
//! A window represents what is typically understood by the end-user as a single application window.
//!
//! Currently it abstracts over xdg-shell toplevels and Xwayland surfaces (TODO).
//! It provides a bunch of methods to calculate and retrieve its size, manage itself, attach additional user_data
//! as well as a [drawing function](`draw_window`) to ease rendering it's related surfaces.
//!
//! Note that a [`Window`] on it's own has no position. For that it needs to be placed inside a [`Space`].
//!
//! ### [`Space`]
//!
//! A space represents a two-dimensional plane of undefined dimensions.
//! [`Window`]s and [`Output`](crate::wayland::output::Output)s can be mapped onto it.
//!
//! Windows get a position and stacking order through mapping. Outputs become views of a part of the [`Space`]
//! and can be rendered via [`Space::render_output`]. Rendering results of spaces are automatically damage-tracked.
//!
//! ### Layer Shell
//!
//! A [`LayerSurface`] represents a surface as provided by e.g. the layer-shell protocol.
//! It provides similar helper methods as a [`Window`] does to toplevel surfaces.
//!
//! Each [`Output`](crate::wayland::output::Output) can be associated a [`LayerMap`] by calling [`layer_map_for_output`],
//! which [`LayerSurface`]s can be mapped upon. Associated layer maps are automatically rendered by [`Space::render_output`],
//! but a [draw function](`draw_layer_surface`) is also provided for manual layer-surface management.
//!
//! ### Popups
//!
//! Provides a [`PopupManager`], which can be used to automatically keep track of popups and their
//! relations to one-another. Popups are then automatically rendered with their matching toplevel surfaces,
//! when either [`draw_window`], [`draw_layer_surface`] or [`Space::render_output`] is called.
//!
//! ## Remarks
//!
//! Note that the desktop abstractions are concerned with easing rendering different clients and therefore need to be able
//! to manage client buffers to do so. If you plan to use the provided drawing functions, you need to use
//! [`on_commit_buffer_handler`](crate::backend::renderer::utils::on_commit_buffer_handler).

#[cfg(feature = "wayland_frontend")]
pub(crate) mod layer;
#[cfg(feature = "wayland_frontend")]
mod popup;
pub mod space;
#[cfg(feature = "wayland_frontend")]
pub mod utils;
#[cfg(feature = "wayland_frontend")]
mod window;

#[cfg(feature = "wayland_frontend")]
pub use self::layer::{draw_layer_surface, layer_map_for_output, LayerMap, LayerSurface};
#[cfg(feature = "wayland_frontend")]
pub use self::popup::*;
pub use self::space::Space;
#[cfg(feature = "wayland_frontend")]
pub use self::window::*;
