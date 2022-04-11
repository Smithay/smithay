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
//! use smithay::wayland::output::{Output, PhysicalProperties, Scale, Mode};
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
//!     Some(Scale::Integer(1)), // global screen scaling factor
//!     Some((0,0).into()) // output position
//! );
//! // set the preferred mode
//! output.set_preferred(Mode { size: (1920, 1080).into(), refresh: 60000 });
//! // add other supported modes
//! output.add_mode(Mode { size: (800, 600).into(), refresh: 60000 });
//! output.add_mode(Mode { size: (1024, 768).into(), refresh: 60000 });
//! ```

pub mod wlr_configuration;
pub mod xdg;

use std::{
    ops::Deref as _,
    sync::{Arc, Mutex},
};

use wayland_server::protocol::{
    wl_output::{Subpixel, Transform},
    wl_surface,
};
use wayland_server::{
    protocol::wl_output::{Mode as WMode, WlOutput},
    Client, Display, Filter, Global, Main, UserDataMap,
};

use slog::{info, o, trace, warn};

use crate::utils::{Logical, Physical, Point, Raw, Size};

use self::xdg::XdgOutput;

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

/// Describes the scale advertised to clients.
#[derive(Debug, Clone, Copy)]
pub enum Scale {
    /// Integer based scaling
    Integer(i32),
    /// Fractional scaling
    ///
    /// *Note*: Protocols only supporting integer-scales (e.g. wl_output),
    /// will be rounded **up** to the next integer value. To control the value
    /// send to those clients use `Scale::Custom` instead.
    Fractional(f64),
    /// Split scale values advertised to clients
    Custom {
        /// Integer value for protocols not supporting fractional scaling
        advertised_integer: i32,
        /// Fractional scaling value used elsewhere
        fractional: f64,
    },
}

impl Scale {
    /// Returns the integer scale as advertised for this `Scale` variant
    pub fn integer_scale(&self) -> i32 {
        match self {
            Scale::Integer(scale) => *scale,
            Scale::Fractional(scale) => scale.ceil() as i32,
            Scale::Custom {
                advertised_integer, ..
            } => *advertised_integer,
        }
    }

    /// Returns the fractional scale (calculated if necessary)
    pub fn fractional_scale(&self) -> f64 {
        match self {
            Scale::Integer(scale) => *scale as f64,
            Scale::Fractional(scale) => *scale,
            Scale::Custom { fractional, .. } => *fractional,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Inner {
    name: String,
    description: String,
    instances: Vec<WlOutput>,
    physical: PhysicalProperties,
    location: Point<i32, Logical>,
    transform: Transform,
    scale: Scale,
    modes: Vec<Mode>,
    current_mode: Option<Mode>,
    preferred_mode: Option<Mode>,

    pub(crate) xdg_output: Option<XdgOutput>,
    pub(crate) log: ::slog::Logger,
}

type InnerType = Arc<(Mutex<Inner>, UserDataMap)>;

impl Inner {
    fn new_global(&mut self, output: WlOutput) {
        trace!(self.log, "New global instantiated.");

        if self.modes.is_empty() {
            warn!(self.log, "Output is used with no modes set"; "name" => &self.name);
        }
        if self.current_mode.is_none() {
            warn!(self.log, "Output is used with no current mod set"; "name" => &self.name);
        }
        if self.preferred_mode.is_none() {
            warn!(self.log, "Output is used with not preferred mode set"; "name" => &self.name);
        }

        self.send_geometry(&output);
        for &mode in &self.modes {
            let mut flags = WMode::empty();
            if Some(mode) == self.current_mode {
                flags |= WMode::Current;
            }
            if Some(mode) == self.preferred_mode {
                flags |= WMode::Preferred;
            }
            output.mode(flags, mode.size.w, mode.size.h, mode.refresh);
        }
        if output.as_ref().version() >= 2 {
            output.scale(self.scale.integer_scale());
            output.done();
        }

        self.instances.push(output);
    }

    fn send_geometry(&self, output: &WlOutput) {
        output.geometry(
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
    pub(crate) inner: InnerType,
}

impl Output {
    /// Creates a new output state with the given name and physical properties.
    pub fn new<L>(name: String, physical: PhysicalProperties, logger: L) -> Output
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "output_handler"));

        info!(log, "Creating new wl_output"; "name" => &name);

        let description = format!("{} - {} - {}", physical.make, physical.model, name);

        let inner = Arc::new((
            Mutex::new(Inner {
                name,
                description,
                instances: Vec::new(),
                physical,
                location: (0, 0).into(),
                transform: Transform::Normal,
                scale: Scale::Integer(1),
                modes: Vec::new(),
                current_mode: None,
                preferred_mode: None,
                xdg_output: None,
                log,
            }),
            UserDataMap::default(),
        ));

        Output { inner }
    }

    /// Create a new output global.
    ///
    /// The global is directly registered into the event loop, and this function
    /// returns the global handle in case you wish to remove this global in the future.
    ///
    /// Calling this function multiple times without destroying the global in between,
    /// will result in multiple globals, meaning the output will be advertised to clients
    /// multiple times.
    pub fn create_global(&self, display: &mut Display) -> Global<WlOutput> {
        let inner = self.inner.clone();
        display.create_global(
            3,
            Filter::new(move |(output, _version): (Main<WlOutput>, _), _, _| {
                output.assign_destructor(Filter::new(|output: WlOutput, _, _| {
                    let inner = output.as_ref().user_data().get::<InnerType>().unwrap();
                    inner
                        .0
                        .lock()
                        .unwrap()
                        .instances
                        .retain(|o| !o.as_ref().equals(output.as_ref()));
                }));
                output.as_ref().user_data().set_threadsafe({
                    let inner = inner.clone();
                    move || inner
                });
                inner.0.lock().unwrap().new_global(output.deref().clone());
            }),
        )
    }

    /// Attempt to retrieve a [`Output`] from an existing resource
    pub fn from_resource(output: &WlOutput) -> Option<Output> {
        output
            .as_ref()
            .user_data()
            .get::<InnerType>()
            .cloned()
            .map(|inner| Output { inner })
    }

    /// Sets the preferred mode of this output
    ///
    /// If the provided mode was not previously known to this output, it is added to its
    /// internal list.
    pub fn set_preferred(&self, mode: Mode) {
        let mut inner = self.inner.0.lock().unwrap();
        inner.preferred_mode = Some(mode);
        if inner.modes.iter().all(|&m| m != mode) {
            inner.modes.push(mode);
        }
    }

    /// Adds a mode to the list of known modes to this output
    pub fn add_mode(&self, mode: Mode) {
        let mut inner = self.inner.0.lock().unwrap();
        if inner.modes.iter().all(|&m| m != mode) {
            inner.modes.push(mode);
        }
    }

    /// Returns the currently advertised mode of the output
    pub fn current_mode(&self) -> Option<Mode> {
        self.inner.0.lock().unwrap().current_mode
    }

    /// Returns the preferred mode of the output
    pub fn preferred_mode(&self) -> Option<Mode> {
        self.inner.0.lock().unwrap().preferred_mode
    }

    /// Returns the currently advertised transformation of the output
    pub fn current_transform(&self) -> Transform {
        self.inner.0.lock().unwrap().transform
    }

    /// Returns the currenly set scale of the output
    pub fn current_scale(&self) -> Scale {
        self.inner.0.lock().unwrap().scale
    }

    /// Returns the currenly advertised location of the output
    pub fn current_location(&self) -> Point<i32, Logical> {
        self.inner.0.lock().unwrap().location
    }

    /// Returns the name of the output
    pub fn name(&self) -> String {
        self.inner.0.lock().unwrap().name.clone()
    }

    /// Returns the description of the output, if xdg-output is initialized
    pub fn description(&self) -> String {
        self.inner.0.lock().unwrap().description.clone()
    }

    /// Returns the physical properties of the output
    pub fn physical_properties(&self) -> PhysicalProperties {
        self.inner.0.lock().unwrap().physical.clone()
    }

    /// Returns the currently advertised modes of the output
    pub fn modes(&self) -> Vec<Mode> {
        self.inner.0.lock().unwrap().modes.clone()
    }

    /// Removes a mode from the list of known modes
    ///
    /// It will not de-advertise it from existing clients (the protocol does not
    /// allow it), but it won't be advertised to now clients from now on.
    pub fn delete_mode(&self, mode: Mode) {
        let mut inner = self.inner.0.lock().unwrap();
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
        new_mode: Option<Mode>,
        new_transform: Option<Transform>,
        new_scale: Option<Scale>,
        new_location: Option<Point<i32, Logical>>,
    ) {
        let mut inner = self.inner.0.lock().unwrap();
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
            xdg_output.change_current_state(new_mode, new_scale, new_location);
        }

        for output in &inner.instances {
            if let Some(mode) = new_mode {
                output.mode(flags, mode.size.w, mode.size.h, mode.refresh);
            }
            if new_transform.is_some() || new_location.is_some() {
                inner.send_geometry(output);
            }
            if let Some(scale) = new_scale {
                if output.as_ref().version() >= 2 {
                    output.scale(scale.integer_scale());
                }
            }
            if output.as_ref().version() >= 2 {
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
            .any(|o| o.as_ref().equals(output.as_ref()))
    }

    /// This function allows to run a [FnMut] on every
    /// [WlOutput] matching the same [Client] as provided
    pub fn with_client_outputs<F>(&self, client: Client, f: F)
    where
        F: FnMut(&WlOutput),
    {
        self.inner
            .0
            .lock()
            .unwrap()
            .instances
            .iter()
            .filter(|output| match output.as_ref().client() {
                Some(output_client) => output_client.equals(&client),
                None => false,
            })
            .for_each(f)
    }

    /// Sends `wl_surface.enter` for the provided surface
    /// with the matching client output
    pub fn enter(&self, surface: &wl_surface::WlSurface) {
        if let Some(client) = surface.as_ref().client() {
            self.with_client_outputs(client, |output| surface.enter(output))
        }
    }

    /// Sends `wl_surface.leave` for the provided surface
    /// with the matching client output
    pub fn leave(&self, surface: &wl_surface::WlSurface) {
        if let Some(client) = surface.as_ref().client() {
            self.with_client_outputs(client, |output| surface.leave(output))
        }
    }

    /// Returns the user data of this output
    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }
}

impl PartialEq for Output {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Output {}
