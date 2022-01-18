//! Wayland backend protocols.
//!
//! In order to support legacy compositors, we support using the Mesa Wayland-DRM protocol to ensure we can
//! use dmabuf backed wl_buffers if linux-dmabuf is not version 4 or later.

#![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#![allow(non_upper_case_globals, non_snake_case, unused_imports)]
#![allow(missing_docs, clippy::all)]

use self::__interfaces::*;
use sctk::reexports::client as wayland_client;
use sctk::reexports::client::backend as wayland_backend;
use wayland_client::protocol::*;

pub mod __interfaces {
    use super::wayland_client::protocol::__interfaces::*;
    use sctk::reexports::client::backend as wayland_backend;

    wayland_scanner::generate_interfaces!("src/backend/wayland/wl_drm.xml");
}

wayland_scanner::generate_client_code!("src/backend/wayland/wl_drm.xml");
