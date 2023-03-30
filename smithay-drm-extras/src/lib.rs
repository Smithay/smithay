//! # Smithay DRM Extras
//!
//! This crate contains some extra abstractions and helpers over DRM
//!
//! - [`edid`] is responsible for extraction of information from DRM connectors
//! - [`drm_scanner`] is responsible for detecting connector connected and
//!   disconnected events, as well as mapping CRTC to them.
//!
//! ### Features
//! - `generate-hwdata` - If enabled [hwdata](https://github.com/vcrhonek/hwdata) code will be regenerated using `hwdata` system package

#![warn(missing_docs, missing_debug_implementations)]

pub mod drm_scanner;
pub mod edid;
mod hwdata;
