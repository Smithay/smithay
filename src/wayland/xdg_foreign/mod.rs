//! Utilities to allow clients to export and import handles to a surface.
//!
//! This module provides automatic management of exporting surfaces from one client and allowing
//! another client to import the surface using a handle. Management of the lifetimes of exports,
//! imports and allowing imported surfaces to be set as the parent of a surface are handled by the
//! module.
//!
//! # How to use it
//!
//! Having a functional xdg_shell global setup is required.
//!
//! With a valid xdg_shell global, simply initialize the xdg_foreign global using the
//! `xdg_foreign_init` function. After that just ensure the `XdgForeignState` returned by the
//! function is kept alive.
//!
//! ```no_run
//! # extern crate wayland_server;
//! # use smithay::wayland::shell::xdg::{xdg_shell_init, XdgRequest};
//! # use smithay::wayland::xdg_foreign::xdg_foreign_init;
//!
//! # let mut display = wayland_server::Display::new();
//! // XDG Shell init
//! let (shell_state, _) = xdg_shell_init(
//!     &mut display,
//!     |event: XdgRequest, dispatch_data| { /* handle the shell requests here */ },
//!     None
//! );
//!
//! let (foreign_state, _, _) = xdg_foreign_init(&mut display, shell_state.clone(), None);
//!
//! // Good to go!
//! ```

use crate::wayland::compositor;
use crate::wayland::shell::is_toplevel_equivalent;
use crate::wayland::shell::legacy::WL_SHELL_SURFACE_ROLE;
use crate::wayland::shell::xdg::ShellState;
use rand::distributions::{Alphanumeric, DistString};
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use wayland_protocols::unstable::xdg_foreign::v2::server::{
    zxdg_exported_v2, zxdg_exporter_v2, zxdg_imported_v2, zxdg_importer_v2,
};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Display, Filter, Global, Main};

/// Manages all exported and imported surfaces.
#[derive(Debug)]
pub struct XdgForeignState {
    _log: ::slog::Logger,
    exports: Vec<Export>,
}

impl XdgForeignState {
    /// Returns true if an export with the given handle is still valid.
    pub fn is_export_valid(&self, handle: &str) -> bool {
        self.exports.iter().any(|export| export.handle == handle)
    }

    /// Returns the surface that an exported handle refers to.
    ///
    /// Returns `None` if no export exists for the handle.
    pub fn get_surface(&self, handle: &str) -> Option<WlSurface> {
        self.exports
            .iter()
            .find(|export| export.handle == handle)
            .map(|export| export.surface.clone())
    }
}

/// Creates new `xdg-foreign` globals.
pub fn xdg_foreign_init<L>(
    display: &mut Display,
    xdg_shell_state: Arc<Mutex<ShellState>>,
    logger: L,
) -> (
    Arc<Mutex<XdgForeignState>>,
    Global<zxdg_exporter_v2::ZxdgExporterV2>,
    Global<zxdg_importer_v2::ZxdgImporterV2>,
)
where
    L: Into<Option<::slog::Logger>>,
{
    let log = crate::slog_or_fallback(logger);

    let state = Arc::new(Mutex::new(XdgForeignState {
        _log: log.new(slog::o!("smithay_module" => "xdg_foreign_handler")),
        exports: vec![],
    }));

    // Borrow checking does not like us cloning the state inside the filter's closure so we clone
    // now and pass the clones into the closure.
    let export_state = state.clone();
    let import_state = state.clone();
    let export_shell = xdg_shell_state.clone();

    let zxdg_exporter_v2_global = display.create_global(
        1,
        Filter::new(move |(exporter, _version), _, _| {
            implement_exporter(exporter, export_state.clone(), export_shell.clone());
        }),
    );

    let zxdg_importer_v2_global = display.create_global(
        1,
        Filter::new(move |(importer, _version), _, _| {
            implement_importer(importer, import_state.clone(), xdg_shell_state.clone());
        }),
    );

    (state, zxdg_exporter_v2_global, zxdg_importer_v2_global)
}

/// An exported surface.
#[derive(Debug)]
struct Export {
    surface: WlSurface,
    handle: String,
    inner: zxdg_exported_v2::ZxdgExportedV2,
    /// All imports made from this export.
    imports: Vec<Import>,
}

impl Export {
    /// Destroys all imports created from this export.
    ///
    /// This does not destroy any relationships that may have been set by the surface.
    fn destroy_imports(&mut self) {
        self.imports.drain(..).for_each(|import| import.inner.destroyed());
    }
}

impl PartialEq for Export {
    fn eq(&self, other: &Self) -> bool {
        // From the documentation of export_toplevel:
        //
        // A surface may be exported multiple times, and each exported handle may be used to create
        // an xdg_imported multiple times.
        //
        // The interpretation used here is that each export is it's own handle to a toplevel
        // surface and multiple different handles may refer to one surface. Therefore equality
        // semantics should compare the surface and the handle.
        self.surface == other.surface && self.handle == other.handle
    }
}

#[derive(Debug)]
struct Import {
    inner: zxdg_imported_v2::ZxdgImportedV2,
    surface: WlSurface,
    /// Child surfaces which have imported this surface as a parent.
    children: Vec<WlSurface>,
}

impl Import {
    fn remove_children(&self, shell: &ShellState) {
        for child in &self.children {
            if let Some(child) = shell.toplevel_surface(child) {
                // Make sure we are still the parent of the child surface?
                if let Some(parent) = child.parent() {
                    if parent == self.surface {
                        // Make the imported surface no longer the parent of this surface.
                        child.set_parent(None);
                    }
                }
            }
        }
    }
}

fn implement_exporter(
    exporter: Main<zxdg_exporter_v2::ZxdgExporterV2>,
    state: Arc<Mutex<XdgForeignState>>,
    shell: Arc<Mutex<ShellState>>,
) -> zxdg_exporter_v2::ZxdgExporterV2 {
    let destructor_state = state.clone();
    let destructor_shell = shell.clone();

    exporter.quick_assign(move |_, request, _| {
        exporter_implementation(request, state.clone(), shell.clone());
    });

    exporter.assign_destructor(Filter::new(
        move |exporter: zxdg_exporter_v2::ZxdgExporterV2, _, _| {
            let state = &mut *destructor_state.lock().unwrap();
            let exports = &mut state.exports;

            // Iterate in reverse to remove elements so we do not need to shift a cursor upon every removal.
            // This would be a whole lot nicer if there were a retain_mut.
            for index in (0..exports.len()).rev() {
                let export = &mut exports[index];

                // If the export is from the client this exporter is destroyed from, then remove it.
                if export.inner.as_ref().same_client_as(exporter.as_ref()) {
                    // Destroy all imports created from this export's handle.
                    export.destroy_imports();

                    for import in &export.imports {
                        import.remove_children(&*destructor_shell.lock().unwrap())
                    }

                    exports.remove(index);
                }
            }
        },
    ));

    exporter.deref().clone()
}

fn exported_implementation(
    exported: Main<zxdg_exported_v2::ZxdgExportedV2>,
    state: Arc<Mutex<XdgForeignState>>,
    shell: Arc<Mutex<ShellState>>,
) {
    exported.assign_destructor(Filter::new(
        move |exported: zxdg_exported_v2::ZxdgExportedV2, _, _| {
            let state = &mut *state.lock().unwrap();

            let exports = &mut state.exports;
            let export = exports
                .iter_mut()
                .find(|export| export.inner == exported)
                .unwrap();

            export.destroy_imports();
            // Remove the export since the client has destroyed it.
            exports.retain(|export| {
                let destroy = export.inner != exported;

                if destroy {
                    let shell = shell.lock().unwrap();

                    // Destroy surface relationships.
                    for import in &export.imports {
                        import.remove_children(&*shell);
                    }
                }

                destroy
            })
        },
    ));
}

fn exporter_implementation(
    request: zxdg_exporter_v2::Request,
    state: Arc<Mutex<XdgForeignState>>,
    shell: Arc<Mutex<ShellState>>,
) {
    match request {
        zxdg_exporter_v2::Request::Destroy => {
            // all is handled by destructor.
        }

        zxdg_exporter_v2::Request::ExportToplevel { id, surface } => {
            // toplevel like would generally be okay, however we cannot rely on wl_shell_surface as
            // a toplevel surface has no tracking for parents, only transient does. That becomes an
            // issue when a wl_shell_surface attempts to import an (z)xdg_toplevel surface and set it
            // as it's parent.
            //
            // Also the presence of a protocol error noting that the surface must be an xdg_toplevel
            // probably means wl_shell_surface was not accounted for in the design. So we throw a
            // protocol error if either surface in the relationship is not an (z)xdg_toplevel.
            if !is_toplevel_equivalent(&surface)
                && compositor::get_role(&surface) != Some(WL_SHELL_SURFACE_ROLE)
            {
                // Protocol error if not a toplevel like
                surface.as_ref().post_error(
                    zxdg_exporter_v2::Error::InvalidSurface as u32,
                    "Surface must be a toplevel equivalent surface".into(),
                );

                return;
            }

            let handle = {
                let state = &mut *state.lock().unwrap();
                // Generate a randomized handle. Only use alphanumerics because some languages do
                // not have the same string capabilities as rust and vice versa.
                let handle = Alphanumeric.sample_string(&mut rand::thread_rng(), 32);
                let exports = &mut state.exports;

                exports.push(Export {
                    surface,
                    handle: handle.clone(),
                    inner: id.deref().clone(),
                    imports: vec![],
                });

                handle
            };

            exported_implementation(id.clone(), state, shell);

            id.deref().handle(handle);
        }

        _ => unreachable!(),
    }
}

fn implement_importer(
    importer: Main<zxdg_importer_v2::ZxdgImporterV2>,
    state: Arc<Mutex<XdgForeignState>>,
    shell_state: Arc<Mutex<ShellState>>,
) -> zxdg_importer_v2::ZxdgImporterV2 {
    let destructor_state = state.clone();
    let destructor_shell = shell_state.clone();

    importer.quick_assign(move |_, request, _| {
        importer_implementation(request, state.clone(), shell_state.clone());
    });

    importer.assign_destructor(Filter::new(
        move |importer: zxdg_importer_v2::ZxdgImporterV2, _, _| {
            let state = &mut *destructor_state.lock().unwrap();

            state.exports.iter_mut().for_each(|export| {
                export
                    .imports
                    // Remove imports from the same client as the importer
                    .retain(|import| {
                        let same_client = import.inner.as_ref().same_client_as(importer.as_ref());

                        if same_client {
                            import.inner.destroyed();
                            import.remove_children(&*destructor_shell.lock().unwrap());

                            false
                        } else {
                            true
                        }
                    });
            });
        },
    ));

    importer.deref().clone()
}

fn importer_implementation(
    request: zxdg_importer_v2::Request,
    state: Arc<Mutex<XdgForeignState>>,
    shell_state: Arc<Mutex<ShellState>>,
) {
    let destructor_state = state.clone();
    let destructor_shell = shell_state.clone();

    match request {
        zxdg_importer_v2::Request::Destroy => {
            // all is handled by destructor.
        }

        zxdg_importer_v2::Request::ImportToplevel { id, handle } => {
            {
                let foreign_state = &mut state.lock().unwrap();
                let exports = &mut foreign_state.exports;

                match exports.iter_mut().find(|export| export.handle == handle) {
                    Some(export) => {
                        let inner = id.deref().clone();

                        export.imports.push(Import {
                            inner,
                            surface: export.surface.clone(),
                            children: vec![],
                        });
                    }

                    // No matching handle was exported, give the client a dead import so the client
                    // knows the import handle is bad
                    None => id.deref().destroyed(),
                }
            }

            id.quick_assign(move |_, request, _| {
                imported_implementation(request, handle.clone(), state.clone(), shell_state.clone());
            });

            id.assign_destructor(Filter::new(
                move |imported: zxdg_imported_v2::ZxdgImportedV2, _, _| {
                    let state = &mut *destructor_state.lock().unwrap();
                    let exports = &mut state.exports;

                    // Remove this import from the list of imports.
                    exports.iter_mut().for_each(|export| {
                        export.imports.retain(|import| {
                            let destroy = import.inner != imported;

                            if destroy {
                                let shell = destructor_shell.lock().unwrap();
                                import.remove_children(&*shell);
                            }

                            destroy
                        })
                    });
                },
            ));
        }

        _ => unreachable!(),
    }
}

fn imported_implementation(
    request: zxdg_imported_v2::Request,
    handle: String,
    state: Arc<Mutex<XdgForeignState>>,
    shell: Arc<Mutex<ShellState>>,
) {
    match request {
        zxdg_imported_v2::Request::Destroy => {
            // all is handled by destructor.
        }

        zxdg_imported_v2::Request::SetParentOf { surface } => {
            // toplevel like would generally be okay, however we cannot rely on wl_shell_surface as
            // a toplevel surface has no tracking for parents, only transient does. That becomes an
            // issue when a wl_shell_surface attempts to import an (z)xdg_toplevel surface and set it
            // as it's parent.
            //
            // Also the presence of a protocol error noting that the surface must be an xdg_toplevel
            // probably means wl_shell_surface was not accounted for in the design. So we throw a
            // protocol error if either surface in the relationship is not an (z)xdg_toplevel.
            if !is_toplevel_equivalent(&surface)
                && compositor::get_role(&surface) != Some(WL_SHELL_SURFACE_ROLE)
            {
                // Protocol error if not a toplevel like surface
                surface.as_ref().post_error(
                    zxdg_imported_v2::Error::InvalidSurface as u32,
                    "Surface must be an xdg_toplevel surface".into(),
                );

                return;
            }

            let shell_state = shell.lock().unwrap();
            let foreign_state = &mut *state.lock().unwrap();
            let toplevel_surface = shell_state.toplevel_surface(&surface).unwrap();
            // Our import is valid, so we can assert the imported surface is a toplevel.
            let imported_parent = foreign_state
                .exports
                .iter()
                .find(|export| export.handle == handle)
                .map(|export| export.surface.clone())
                .unwrap();

            toplevel_surface.set_parent(Some(&imported_parent));
        }

        _ => unreachable!(),
    }
}
