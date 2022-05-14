//! Output advertising capabilities
//!
//! This module provides a type helping you to abstract over various
//! properties, that make up an [`Output`] of your compositor.
//!
//! Additionally the [`Output`] may handle advertising of your compositor's
//! output and their capabilities to your client, as well as mapping your
//! clients output request to your physical outputs.
//!
//! # How to use it
//!
//! You need to instantiate an [`Output`].
//! To advertise a new output global to clients you then need to use [`Output::create_global`].
//! The resulting `Global<WlOutput>` can later be destroyed again to stop advertising it
//! without destroying it's state. E.g. in case the matching physical output got disabled at runtime.
//!
//! You can use the returned [`Output`] to change
//! the properties of your output (if the current resolution mode changes for example),
//! it'll automatically forward any changes to the clients.
//!
//! Additional protocols may piggy-back on this type.
//! E.g. to also advertise an xdg-output for every wl-output you can use
//! [`xdg::init_xdg_output_manager`].
//!
//! You can attach additional properties to your `Output`s by using [`Output::user_data`].
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! use smithay::wayland::output::{Output, PhysicalProperties, Mode};
//! use wayland_server::protocol::wl_output;
//!
//! # let mut display = wayland_server::Display::new();
//! // Create the Output with given name and physical properties
//! let output = Output::new(
//!     "output-0".into(), // the name of this output,
//!     PhysicalProperties {
//!         size: (200, 150).into(),        // dimensions (width, height) in mm
//!         subpixel: wl_output::Subpixel::HorizontalRgb,  // subpixel information
//!         make: "Screens Inc".into(),     // make of the monitor
//!         model: "Monitor Ultra".into(),  // model of the monitor
//!     },
//!     None // insert a logger here
//! );
//! // create a global, if you want to advertise it to clients
//! let _output_global = output.create_global(
//!     &mut display,      // the display//!
//! ); // you can drop the global, if you never intend to destroy it.
//! // Now you can configure it
//! output.change_current_state(
//!     Some(Mode { size: (1920, 1080).into(), refresh: 60000 }), // the resolution mode,
//!     Some(wl_output::Transform::Normal), // global screen transformation
//!     Some(1), // global screen scaling factor
//!     Some((0,0).into()) // output position
//! );
//! // set the preferred mode
//! output.set_preferred(Mode { size: (1920, 1080).into(), refresh: 60000 });
//! // add other supported modes
//! output.add_mode(Mode { size: (800, 600).into(), refresh: 60000 });
//! output.add_mode(Mode { size: (1024, 768).into(), refresh: 60000 });
//! ```

mod handlers;
pub mod xdg;

use std::sync::{Arc, Mutex};

use wayland_protocols::unstable::xdg_output::v1::server::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_server::{
    backend::GlobalId,
    protocol::{
        wl_output::{Subpixel, Transform},
        wl_surface,
    },
    DisplayHandle, GlobalDispatch, Resource,
};
use wayland_server::{
    protocol::wl_output::{Mode as WMode, WlOutput},
    Client, Display,
};

use slog::{info, o};

use crate::utils::{user_data::UserDataMap, Logical, Physical, Point, Raw, Size};

use self::xdg::XdgOutput;

/// State of Smithay output manager
#[derive(Debug, Default)]
pub struct OutputManagerState {
    xdg_output_manager: Option<GlobalId>,
}

impl OutputManagerState {
    /// Create new output manager
    pub fn new() -> Self {
        Self {
            xdg_output_manager: None,
        }
    }

    /// Create new output manager with xdg output support
    pub fn new_with_xdg_output<D>(display: &mut Display<D>) -> Self
    where
        D: GlobalDispatch<WlOutput, GlobalData = OutputGlobalData>,
        D: GlobalDispatch<ZxdgOutputManagerV1, GlobalData = ()>,
        D: 'static,
    {
        let xdg_output_manager = display.create_global::<ZxdgOutputManagerV1>(3, ());

        Self {
            xdg_output_manager: Some(xdg_output_manager),
        }
    }

    /// Get global id of xdg output manager
    pub fn xdg_output_manager_global(&self) -> Option<GlobalId> {
        self.xdg_output_manager.clone()
    }
}

/// An output mode
///
/// A possible combination of dimensions and refresh rate for an output.
///
/// This should only describe the characteristics of the video driver,
/// not taking into account any global scaling.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Mode {
    /// The size of the mode, in pixels
    pub size: Size<i32, Physical>,
    /// The refresh rate in millihertz
    ///
    /// `1000` is one fps (frame per second), `2000` is 2 fps, etc...
    pub refresh: i32,
}

/// The physical properties of an output
#[derive(Debug, Clone)]
pub struct PhysicalProperties {
    /// The size of the monitor, in millimeters
    pub size: Size<i32, Raw>,
    /// The subpixel geometry
    pub subpixel: Subpixel,
    /// Textual representation of the make
    pub make: String,
    /// Textual representation of the model
    pub model: String,
}

#[derive(Debug)]
pub(crate) struct Inner {
    name: String,
    description: String,
    instances: Vec<WlOutput>,
    physical: PhysicalProperties,
    location: Point<i32, Logical>,
    transform: Transform,
    scale: i32,
    modes: Vec<Mode>,
    current_mode: Option<Mode>,
    preferred_mode: Option<Mode>,

    pub(crate) xdg_output: Option<XdgOutput>,
    pub(crate) log: ::slog::Logger,
}

/// Data for WlOutput global
#[derive(Debug, Clone)]
pub struct OutputGlobalData {
    pub(crate) inner: Arc<(Mutex<Inner>, UserDataMap)>,
}

/// User data for WlOutput
#[derive(Debug, Clone)]
pub struct OutputUserData {
    pub(crate) global_data: OutputGlobalData,
}

impl Inner {
    fn send_geometry_to(&self, dh: &mut DisplayHandle<'_>, output: &WlOutput) {
        output.geometry(
            dh,
            self.location.x,
            self.location.y,
            self.physical.size.w,
            self.physical.size.h,
            self.physical.subpixel,
            self.physical.make.clone(),
            self.physical.model.clone(),
            self.transform,
        );
    }
}

/// An abstract output.
///
/// This handle is stored in the event loop, and allows you to notify clients
/// about any change in the properties of this output.
#[derive(Debug, Clone)]
pub struct Output {
    pub(crate) data: OutputGlobalData,
}

impl Output {
    /// Create a new output global with given name and physical properties.
    ///
    /// The global is directly registered into the event loop, and this function
    /// returns the state token allowing you to access it, as well as the global handle,
    /// in case you wish to remove this global in the future.
    pub fn new<L, D>(
        display: &mut Display<D>,
        name: String,
        physical: PhysicalProperties,
        logger: L,
    ) -> (Output, GlobalId)
    where
        L: Into<Option<::slog::Logger>>,
        D: GlobalDispatch<WlOutput, GlobalData = OutputGlobalData>,
        D: 'static,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "output_handler"));

        info!(log, "Creating new wl_output"; "name" => &name);

        let data = OutputGlobalData {
            inner: Arc::new((
                Mutex::new(Inner {
                    name: name.clone(),
                    description: format!("{} - {} - {}", physical.make, physical.model, name),
                    instances: Vec::new(),
                    physical,
                    location: (0, 0).into(),
                    transform: Transform::Normal,
                    scale: 1,
                    modes: Vec::new(),
                    current_mode: None,
                    preferred_mode: None,
                    xdg_output: None,
                    log,
                }),
                UserDataMap::default(),
            )),
        };

        let output = Output { data: data.clone() };

        let global = display.create_global::<WlOutput>(4, data);

        (output, global)
    }

    /// Attempt to retrieve a [`Output`] from an existing resource
    pub fn from_resource(output: &WlOutput) -> Option<Output> {
        output.data::<OutputUserData>().map(|ud| Output {
            data: ud.global_data.clone(),
        })
    }

    /// Sets the preferred mode of this output
    ///
    /// If the provided mode was not previously known to this output, it is added to its
    /// internal list.
    pub fn set_preferred(&self, mode: Mode) {
        let mut inner = self.data.inner.0.lock().unwrap();
        inner.preferred_mode = Some(mode);
        if inner.modes.iter().all(|&m| m != mode) {
            inner.modes.push(mode);
        }
    }

    /// Adds a mode to the list of known modes to this output
    pub fn add_mode(&self, mode: Mode) {
        let mut inner = self.data.inner.0.lock().unwrap();
        if inner.modes.iter().all(|&m| m != mode) {
            inner.modes.push(mode);
        }
    }

    /// Returns the currently advertised mode of the output
    pub fn current_mode(&self) -> Option<Mode> {
        self.data.inner.0.lock().unwrap().current_mode
    }

    /// Returns the preferred mode of the output
    pub fn preferred_mode(&self) -> Option<Mode> {
        self.data.inner.0.lock().unwrap().preferred_mode
    }

    /// Returns the currently advertised transformation of the output
    pub fn current_transform(&self) -> Transform {
        self.data.inner.0.lock().unwrap().transform
    }

    /// Returns the currenly advertised scale of the output
    pub fn current_scale(&self) -> i32 {
        self.data.inner.0.lock().unwrap().scale
    }

    /// Returns the currenly advertised location of the output
    pub fn current_location(&self) -> Point<i32, Logical> {
        self.data.inner.0.lock().unwrap().location
    }

    /// Returns the name of the output
    pub fn name(&self) -> String {
        self.data.inner.0.lock().unwrap().name.clone()
    }

    /// Returns the description of the output, if xdg-output is initialized
    pub fn description(&self) -> String {
        self.data.inner.0.lock().unwrap().description.clone()
    }

    /// Returns the physical properties of the output
    pub fn physical_properties(&self) -> PhysicalProperties {
        self.data.inner.0.lock().unwrap().physical.clone()
    }

    /// Returns the currently advertised modes of the output
    pub fn modes(&self) -> Vec<Mode> {
        self.data.inner.0.lock().unwrap().modes.clone()
    }

    /// Removes a mode from the list of known modes
    ///
    /// It will not de-advertise it from existing clients (the protocol does not
    /// allow it), but it won't be advertised to now clients from now on.
    pub fn delete_mode(&self, mode: Mode) {
        let mut inner = self.data.inner.0.lock().unwrap();
        inner.modes.retain(|&m| m != mode);
        if inner.current_mode == Some(mode) {
            inner.current_mode = None;
        }
        if inner.preferred_mode == Some(mode) {
            inner.preferred_mode = None;
        }
    }

    /// Change the current state of this output
    ///
    /// You can changed the current mode, transform status, location or scale of this output. Providing
    /// `None` to any of these field means that the value does not change.
    ///
    /// If the provided mode was not previously known to this output, it is added to its
    /// internal list.
    ///
    /// By default, transform status is `Normal`, and scale is `1`.
    pub fn change_current_state(
        &self,
        dh: &mut DisplayHandle<'_>,
        new_mode: Option<Mode>,
        new_transform: Option<Transform>,
        new_scale: Option<i32>,
        new_location: Option<Point<i32, Logical>>,
    ) {
        let mut inner = self.data.inner.0.lock().unwrap();
        if let Some(mode) = new_mode {
            if inner.modes.iter().all(|&m| m != mode) {
                inner.modes.push(mode);
            }
            inner.current_mode = new_mode;
        }
        if let Some(transform) = new_transform {
            inner.transform = transform;
        }
        if let Some(scale) = new_scale {
            inner.scale = scale;
        }
        let mut flags = WMode::Current;
        if inner.preferred_mode == new_mode {
            flags |= WMode::Preferred;
        }
        if let Some(new_location) = new_location {
            inner.location = new_location;
        }

        // XdgOutput has to be updated before WlOutput
        // Because WlOutput::done() has to allways be called last
        if let Some(xdg_output) = inner.xdg_output.as_ref() {
            xdg_output.change_current_state(dh, new_mode, new_scale, new_location);
        }

        for output in &inner.instances {
            if let Some(mode) = new_mode {
                output.mode(dh, flags, mode.size.w, mode.size.h, mode.refresh);
            }
            if new_transform.is_some() || new_location.is_some() {
                inner.send_geometry_to(dh, output);
            }
            if let Some(scale) = new_scale {
                if output.version() >= 2 {
                    output.scale(dh, scale);
                }
            }
            if output.version() >= 2 {
                output.done(dh);
            }
        }
    }

    /// Check is given [`wl_output`](WlOutput) instance is managed by this [`Output`].
    pub fn owns(&self, output: &WlOutput) -> bool {
        self.data
            .inner
            .0
            .lock()
            .unwrap()
            .instances
            .iter()
            .any(|o| o.id() == output.id())
    }

    /// This function allows to run a [FnMut] on every
    /// [WlOutput] matching the same [Client] as provided
    pub fn with_client_outputs<F>(&self, dh: &mut DisplayHandle<'_>, client: &Client, mut f: F)
    where
        F: FnMut(&mut DisplayHandle<'_>, &WlOutput),
    {
        let list: Vec<_> = self
            .data
            .inner
            .0
            .lock()
            .unwrap()
            .instances
            .iter()
            .filter(|output| {
                dh.get_client(output.id())
                    .map(|output_client| &output_client == client)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        for o in list {
            f(dh, &o);
        }
    }

    /// Sends `wl_surface.enter` for the provided surface
    /// with the matching client output
    pub fn enter(&self, dh: &mut DisplayHandle<'_>, surface: &wl_surface::WlSurface) {
        if let Ok(client) = dh.get_client(surface.id()) {
            self.with_client_outputs(dh, &client, |dh, output| surface.enter(dh, output))
        }
    }

    /// Sends `wl_surface.leave` for the provided surface
    /// with the matching client output
    pub fn leave(&self, dh: &mut DisplayHandle<'_>, surface: &wl_surface::WlSurface) {
        if let Ok(client) = dh.get_client(surface.id()) {
            self.with_client_outputs(dh, &client, |dh, output| surface.leave(dh, output))
        }
    }

    /// Returns the user data of this output
    pub fn user_data(&self) -> &UserDataMap {
        &self.data.inner.1
    }
}

impl PartialEq for Output {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data.inner, &other.data.inner)
    }
}

impl Eq for Output {}
