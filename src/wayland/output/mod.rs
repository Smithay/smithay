//! WlOutput advertising capabilities
//!
//! This module provides additional implementations for [`crate::output::Output`]
//! to handle advertising of your compositor's output and their capabilities to your client,
//! as well as mapping your clients output request to your physical outputs.
//!
//! # How to use it
//!
//! After you have instantiated an [`Output`] you need to use [`Output::create_global`]
//! to advertise a new output global to clients.
//! The resulting `GlobalId` can later be destroyed again to stop advertising it
//! without destroying it's state. E.g. in case the matching physical output got disabled at runtime.
//!
//! If you change the properties of your output (if the current resolution mode changes for example),
//! it'll automatically forward any changes to the clients.
//!
//! Additional protocols may piggy-back on this type.
//! E.g. to also advertise an xdg-output for every wl-output you can use
//! [`OutputManagerState::new_with_xdg_output`].
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! use smithay::delegate_output;
//! use smithay::output::{Output, PhysicalProperties, Scale, Mode, Subpixel};
//! use smithay::utils::Transform;
//! use smithay::wayland::output::OutputHandler;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! // Create the Output with given name and physical properties.
//! let output = Output::new(
//!     "output-0".into(), // the name of this output,
//!     PhysicalProperties {
//!         size: (200, 150).into(),        // dimensions (width, height) in mm
//!         subpixel: Subpixel::HorizontalRgb,  // subpixel information
//!         make: "Screens Inc".into(),     // make of the monitor
//!         model: "Monitor Ultra".into(),  // model of the monitor
//!     },
//! );
//! // create a global, if you want to advertise it to clients
//! let _global = output.create_global::<State>(
//!     &display_handle,      // the display
//! ); // you can drop the global, if you never intend to destroy it.
//! // Now you can configure it
//! output.change_current_state(
//!     Some(Mode { size: (1920, 1080).into(), refresh: 60000 }), // the resolution mode,
//!     Some(Transform::Normal), // global screen transformation
//!     Some(Scale::Integer(1)), // global screen scaling factor
//!     Some((0,0).into()) // output position
//! );
//! // set the preferred mode
//! output.set_preferred(Mode { size: (1920, 1080).into(), refresh: 60000 });
//! // add other supported modes
//! output.add_mode(Mode { size: (800, 600).into(), refresh: 60000 });
//! output.add_mode(Mode { size: (1024, 768).into(), refresh: 60000 });
//!
//! impl OutputHandler for State {}
//! delegate_output!(State);
//! ```

mod handlers;
pub(crate) mod xdg;

use crate::output::{Inner, Mode, Output, OutputData, Scale, Subpixel};

use tracing::info;
use wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_server::{
    backend::{ClientId, GlobalId},
    protocol::{
        wl_output::{Mode as WMode, Subpixel as WlSubpixel, Transform, WlOutput},
        wl_surface,
    },
    Client, DisplayHandle, GlobalDispatch, Resource,
};

use crate::utils::{Logical, Point};

pub use self::handlers::XdgOutputUserData;

/// State of Smithay output manager
#[derive(Debug, Default)]
pub struct OutputManagerState {
    xdg_output_manager: Option<GlobalId>,
}

/// Internal data of a wl_output global
#[derive(Debug)]
pub struct WlOutputData {
    inner: OutputData,
}

/// Events initiated by the clients interacting with outputs
pub trait OutputHandler {
    /// A client bound a new `wl_output` instance.
    fn output_bound(&mut self, _output: Output, _wl_output: WlOutput) {}
}

impl OutputManagerState {
    /// Create new output manager
    pub fn new() -> Self {
        Self {
            xdg_output_manager: None,
        }
    }

    /// Create new output manager with xdg output support
    pub fn new_with_xdg_output<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WlOutput, WlOutputData>,
        D: GlobalDispatch<ZxdgOutputManagerV1, ()>,
        D: 'static,
    {
        let xdg_output_manager = display.create_global::<D, ZxdgOutputManagerV1, _>(3, ());

        Self {
            xdg_output_manager: Some(xdg_output_manager),
        }
    }

    /// Get global id of xdg output manager
    pub fn xdg_output_manager_global(&self) -> Option<GlobalId> {
        self.xdg_output_manager.clone()
    }
}

/// User data for WlOutput
#[derive(Debug, Clone)]
pub struct OutputUserData {
    pub(crate) global_data: OutputData,
}

impl Inner {
    fn send_geometry_to(&self, output: &WlOutput) {
        output.geometry(
            self.location.x,
            self.location.y,
            self.physical.size.w,
            self.physical.size.h,
            self.physical.subpixel.into(),
            self.physical.make.clone(),
            self.physical.model.clone(),
            self.transform.into(),
        );
    }
}

impl From<Subpixel> for WlSubpixel {
    fn from(s: Subpixel) -> Self {
        match s {
            Subpixel::HorizontalBgr => WlSubpixel::HorizontalBgr,
            Subpixel::HorizontalRgb => WlSubpixel::HorizontalRgb,
            Subpixel::None => WlSubpixel::None,
            Subpixel::Unknown => WlSubpixel::Unknown,
            Subpixel::VerticalBgr => WlSubpixel::VerticalBgr,
            Subpixel::VerticalRgb => WlSubpixel::VerticalRgb,
        }
    }
}

impl Output {
    /// Create a new output global.
    ///
    /// The global is directly registered into the event loop, and this function
    /// returns the global handle in case you wish to remove this global in the future.
    ///
    /// Calling this function multiple times without destroying the global in between,
    /// will result in multiple globals, meaning the output will be advertised to clients
    /// multiple times.
    pub fn create_global<D>(&self, display: &DisplayHandle) -> GlobalId
    where
        D: GlobalDispatch<WlOutput, WlOutputData>,
        D: 'static,
    {
        info!(output = self.name(), "Creating new wl_output");
        self.inner.0.lock().unwrap().handle = Some(display.backend_handle().downgrade());
        display.create_global::<D, WlOutput, _>(
            4,
            WlOutputData {
                inner: self.inner.clone(),
            },
        )
    }

    /// Attempt to retrieve a [`Output`] from an existing resource
    pub fn from_resource(output: &WlOutput) -> Option<Output> {
        output.data::<OutputUserData>().map(|ud| Output {
            inner: ud.global_data.clone(),
        })
    }

    pub(crate) fn wl_change_current_state(
        &self,
        new_mode: Option<Mode>,
        new_transform: Option<Transform>,
        new_scale: Option<Scale>,
        new_location: Option<Point<i32, Logical>>,
    ) {
        let inner = self.inner.0.lock().unwrap();
        // XdgOutput has to be updated before WlOutput
        // Because WlOutput::done() has to allways be called last
        if let Some(xdg_output) = inner.xdg_output.as_ref() {
            xdg_output.change_current_state(new_mode, new_scale, new_location, new_transform);
        }

        let mut flags = WMode::Current;
        if inner.preferred_mode == new_mode {
            flags |= WMode::Preferred;
        }

        for output in &inner.instances {
            if let Some(mode) = new_mode {
                output.mode(flags, mode.size.w, mode.size.h, mode.refresh);
            }
            if new_transform.is_some() || new_location.is_some() {
                inner.send_geometry_to(output);
            }
            if let Some(scale) = new_scale {
                if output.version() >= 2 {
                    output.scale(scale.integer_scale());
                }
            }
            if output.version() >= 2 {
                output.done();
            }
        }
    }

    /// Check is given [`wl_output`](WlOutput) instance is managed by this [`Output`].
    pub fn owns(&self, output: &WlOutput) -> bool {
        self.inner
            .0
            .lock()
            .unwrap()
            .instances
            .iter()
            .any(|o| o.id() == output.id())
    }

    /// This function returns all managed [WlOutput] matching the provided [Client]
    pub fn client_outputs(&self, client: &Client) -> Vec<WlOutput> {
        self.client_outputs_internal(client.id())
    }

    fn client_outputs_internal(&self, client: ClientId) -> Vec<WlOutput> {
        let data = self.inner.0.lock().unwrap();
        data.instances
            .iter()
            .filter(|output| {
                data.handle
                    .as_ref()
                    .and_then(|handle| handle.upgrade())
                    .and_then(|handle| handle.get_client(output.id()).ok())
                    .map(|output_client| output_client == client)
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    /// Sends `wl_surface.enter` for the provided surface
    /// with the matching client output
    pub fn enter(&self, surface: &wl_surface::WlSurface) {
        let client = self
            .inner
            .0
            .lock()
            .unwrap()
            .handle
            .as_ref()
            .and_then(|handle| handle.upgrade())
            .and_then(|handle| handle.get_client(surface.id()).ok());
        if let Some(client) = client {
            let weak = surface.downgrade();
            let mut inner = self.inner.0.lock().unwrap();
            inner.surfaces.retain(|s| s.upgrade().is_ok());
            if !inner.surfaces.contains(&weak) {
                inner.surfaces.insert(weak);
                drop(inner);

                for output in self.client_outputs_internal(client) {
                    surface.enter(&output);
                }
            }
        }
    }

    /// Sends `wl_surface.leave` for the provided surface
    /// with the matching client output
    pub fn leave(&self, surface: &wl_surface::WlSurface) {
        let client = self
            .inner
            .0
            .lock()
            .unwrap()
            .handle
            .as_ref()
            .and_then(|handle| handle.upgrade())
            .and_then(|handle| handle.get_client(surface.id()).ok());
        if let Some(client) = client {
            let weak = surface.downgrade();
            let mut inner = self.inner.0.lock().unwrap();
            inner.surfaces.retain(|s| s.upgrade().is_ok());
            if inner.surfaces.contains(&weak) {
                inner.surfaces.remove(&weak);
                drop(inner);

                for output in self.client_outputs_internal(client) {
                    surface.leave(&output);
                }
            }
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_output {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_output::WlOutput: $crate::wayland::output::WlOutputData
        ] => $crate::wayland::output::OutputManagerState);
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_manager_v1::ZxdgOutputManagerV1: ()
        ] => $crate::wayland::output::OutputManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_output::WlOutput: $crate::wayland::output::OutputUserData
        ] => $crate::wayland::output::OutputManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_v1::ZxdgOutputV1: $crate::wayland::output::XdgOutputUserData
        ] => $crate::wayland::output::OutputManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_manager_v1::ZxdgOutputManagerV1: ()
        ] => $crate::wayland::output::OutputManagerState);
    };
}
