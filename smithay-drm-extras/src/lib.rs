//! # Smithay DRM Extras
//!
//! This crate contains some extra abstractions and helpers over DRM
//!
//! - [`display_info`] is responsible for extraction of information from DRM connectors
//! - [`drm_scanner`] is responsible for detecting connector connected and
//!   disconnected events, as well as mapping CRTC to them.
//!
//! ### Features
//! - `display_info` - If enabled `display_info` functionality is enabled through `libdisplay-info` integration

#![warn(missing_docs, missing_debug_implementations)]

#[cfg(feature = "display-info")]
pub mod display_info;
pub mod drm_scanner;
