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
    sync::{Arc, Mutex},
};

use rand::distr::{Alphanumeric, SampleString};
use wayland_protocols::xdg::foreign::zv2::server::{
    zxdg_exporter_v2::ZxdgExporterV2, zxdg_imported_v2::ZxdgImportedV2, zxdg_importer_v2::ZxdgImporterV2,
};
use wayland_server::{DisplayHandle, GlobalDispatch, backend::GlobalId, protocol::wl_surface::WlSurface};

use crate::wayland::{
    GlobalData, compositor,
    shell::xdg::{ToplevelSurface, XdgShellHandler},
    xdg_foreign::handlers::invalidate_relationship_for,
};

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
pub struct XdgForeignHandle(String);

impl XdgForeignHandle {
    fn new() -> Self {
        Self(Alphanumeric.sample_string(&mut rand::rng(), 32))
    }

    /// Returns the string representation of the handle.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Destroys a compositor-side toplevel export.
    pub fn unexport<D>(self, data: &mut D)
    where
        D: XdgForeignHandler + XdgShellHandler,
    {
        invalidate_relationship_for(data, &self, None);
        data.xdg_foreign_state().exported.remove(&self);
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
    requested_child: Option<(WlSurface, ZxdgImportedV2)>,
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
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: XdgForeignHandler,
        D: GlobalDispatch<ZxdgExporterV2, GlobalData>,
        D: GlobalDispatch<ZxdgImporterV2, GlobalData>,
    {
        let exporter = display.create_global::<D, ZxdgExporterV2, _>(1, GlobalData);
        let importer = display.create_global::<D, ZxdgImporterV2, _>(1, GlobalData);

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

    /// Creates a compositor-side toplevel export.
    ///
    /// Exports `toplevel`.  The compositor can use the returned `XdgForeignHandle` to send the
    /// handle string to another client, allowing it to import `toplevel`.
    ///
    /// The returned handle will be automatically destroyed when `toplevel`'s underlying surface is
    /// destroyed, but can also be destroyed using [`XdgForeignHandle::unexport`].
    pub fn export_toplevel<D>(&mut self, toplevel: &ToplevelSurface) -> XdgForeignHandle
    where
        D: XdgForeignHandler + XdgShellHandler,
    {
        let handle = XdgForeignHandle::new();
        self.exported.insert(
            handle.clone(),
            ExportedState {
                exported_surface: toplevel.wl_surface().clone(),
                requested_child: None,
                imported_by: HashSet::new(),
            },
        );

        compositor::add_destruction_hook::<D, _>(toplevel.wl_surface(), {
            let handle = Arc::new(Mutex::new(Some(handle.clone())));
            move |data, _| {
                if let Some(handle) = handle.lock().unwrap().take() {
                    handle.unexport(data);
                }
            }
        });

        handle
    }
}
