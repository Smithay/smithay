//! Implementation `xdg_foreign` protocol
//!
//! ```rs
//! # extern crate wayland_server;
//! #
//! use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};
//! use smithay::{
//!     delegate_xdg_foreign,
//!     wayland::xdg_foreign::{XdgForeignHandler, XdgForeignState}
//! };
//!
//! pub struct State {
//!     xdg_foreign_state: XdgForeignState,
//! }
//!
//! impl XdgForeignHandler for State {
//!     fn xdg_foreignHandler_state(&mut self) -> &mut XdgForeignState {
//!         &mut self.xdg_foreign_state
//!     }
//! }
//!
//! // Delegate xdg foreign handling for State to XdgForeignState.
//! delegate_xdg_foreign!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! let state = State {
//!     xdg_foreign_state: XdgForeignState::new::<State>(&display_handle),
//! };
//! ```

use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
};

use rand::distributions::{Alphanumeric, DistString};
use wayland_protocols::xdg::foreign::zv2::server::{
    zxdg_exporter_v2::ZxdgExporterV2, zxdg_imported_v2::ZxdgImportedV2, zxdg_importer_v2::ZxdgImporterV2,
};
use wayland_server::{backend::GlobalId, protocol::wl_surface::WlSurface, DisplayHandle};

mod handlers;

/// A trait implemented to be notified of activation requests using the xdg foreign protocol.
pub trait XdgForeignHandler: 'static {
    /// Returns the xdg foreign state.
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState;
}

/// The handle contains the unique handle of exported surface.
/// It may be shared with any client, which then can use it to import the surface by calling xdg_importer.import_toplevel.
/// A handle may be used to import the surface multiple times.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct XdgForeignHandle(String);

impl XdgForeignHandle {
    fn new() -> Self {
        Self(Alphanumeric.sample_string(&mut rand::thread_rng(), 32))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for XdgForeignHandle {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

/// User data of xdg_exported
#[derive(Debug)]
pub struct XdgExportedUserData {
    handle: XdgForeignHandle,
}

/// User data of xdg_imported
#[derive(Debug)]
pub struct XdgImportedUserData {
    handle: XdgForeignHandle,
}

#[derive(Debug)]
struct ExportedState {
    exported_surface: WlSurface,
    requested_parent: Option<(WlSurface, ZxdgImportedV2)>,
    imported_by: HashSet<ZxdgImportedV2>,
}

/// Tracks the list of exported surfaces
#[derive(Debug)]
pub struct XdgForeignState {
    exported: HashMap<XdgForeignHandle, ExportedState>,
    exporter: GlobalId,
    importer: GlobalId,
}

impl XdgForeignState {
    /// Creates a new xdg activation global.
    ///
    /// In order to use this abstraction, your `D` type needs to implement [`XdgForeignHandler`].
    pub fn new<D: XdgForeignHandler>(display: &DisplayHandle) -> Self {
        let exporter = display.create_delegated_global::<D, ZxdgExporterV2, _, Self>(1, ());
        let importer = display.create_delegated_global::<D, ZxdgImporterV2, _, Self>(1, ());

        Self {
            exported: HashMap::new(),
            exporter,
            importer,
        }
    }

    /// Returns the xdg_exporter global.
    pub fn exporter_global(&self) -> GlobalId {
        self.exporter.clone()
    }

    /// Returns the xdg_importer global.
    pub fn importer_global(&self) -> GlobalId {
        self.importer.clone()
    }
}
