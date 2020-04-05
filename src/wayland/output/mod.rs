//! Output advertising capabilities
//!
//! This module provides a type helping you to handle the advertising
//! of your compositor's output and their capabilities to your client,
//! as well as mapping your clients output request to your physical
//! outputs.
//!
//! # How to use it
//!
//! You need to instantiate an [`Output`](::wayland::output::Output)
//! for each output global you want to advertise to clients.
//!
//! Just add it to your Display using the [`Output::new(..)`](::wayland::output::Output::new)
//! method. You can use the returned [`Output`](::wayland::output::Output) to change
//! the properties of your output (if the current resolution mode changes for example),
//! it'll automatically forward any changes to the clients.
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! use smithay::wayland::output::{Output, PhysicalProperties, Mode};
//! use wayland_server::protocol::wl_output;
//!
//! # let mut display = wayland_server::Display::new();
//! // Create the Output with given name and physical properties
//! let (output, _output_global) = Output::new(
//!     &mut display,      // the display
//!     "output-0".into(), // the name of this output,
//!     PhysicalProperties {
//!         width: 200,                     // width in mm
//!         height: 150,                    // height in mm,
//!         subpixel: wl_output::Subpixel::HorizontalRgb,  // subpixel information
//!         make: "Screens Inc".into(),     // make of the monitor
//!         model: "Monitor Ultra".into(),  // model of the monitor
//!     },
//!     None // insert a logger here
//! );
//! // Now you can configure it
//! output.change_current_state(
//!     Some(Mode { width: 1902, height: 1080, refresh: 60000 }), // the resolution mode,
//!     Some(wl_output::Transform::Normal), // global screen transformation
//!     Some(1), // global screen scaling factor
//! );
//! // set the preferred mode
//! output.set_preferred(Mode { width: 1920, height: 1080, refresh: 60000 });
//! // add other supported modes
//! output.add_mode(Mode { width: 800, height: 600, refresh: 60000 });
//! output.add_mode(Mode { width: 1024, height: 768, refresh: 60000 });
//! ```

use std::{
    ops::Deref as _,
    sync::{Arc, Mutex},
};

use wayland_server::protocol::wl_output::{Subpixel, Transform};
use wayland_server::{
    protocol::wl_output::{Mode as WMode, WlOutput},
    Display, Filter, Global, Main,
};

/// An output mode
///
/// A possible combination of dimensions and refresh rate for an output.
///
/// This should only describe the characteristics of the video driver,
/// not taking into account any global scaling.
#[derive(Copy, Clone, PartialEq)]
pub struct Mode {
    /// The width in pixels
    pub width: i32,
    /// The height in pixels
    pub height: i32,
    /// The refresh rate in millihertz
    ///
    /// `1000` is one fps (frame per second), `2000` is 2 fps, etc...
    pub refresh: i32,
}

/// The physical properties of an output
pub struct PhysicalProperties {
    /// The width in millimeters
    pub width: i32,
    /// The height in millimeters
    pub height: i32,
    /// The subpixel geometry
    pub subpixel: Subpixel,
    /// Textual representation of the make
    pub make: String,
    /// Textual representation of the model
    pub model: String,
}

struct Inner {
    name: String,
    log: ::slog::Logger,
    instances: Vec<WlOutput>,
    physical: PhysicalProperties,
    location: (i32, i32),
    transform: Transform,
    scale: i32,
    modes: Vec<Mode>,
    current_mode: Option<Mode>,
    preferred_mode: Option<Mode>,
}

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
            output.mode(flags, mode.width, mode.height, mode.refresh);
        }
        if output.as_ref().version() >= 2 {
            output.scale(self.scale);
            output.done();
        }

        self.instances.push(output);
    }

    fn send_geometry(&self, output: &WlOutput) {
        output.geometry(
            self.location.0,
            self.location.1,
            self.physical.width,
            self.physical.height,
            self.physical.subpixel,
            self.physical.make.clone(),
            self.physical.model.clone(),
            self.transform,
        );
    }
}

/// An output as seen by the clients
///
/// This handle is stored in the event loop, and allows you to notify clients
/// about any change in the properties of this output.
pub struct Output {
    inner: Arc<Mutex<Inner>>,
}

impl Output {
    /// Create a new output global with given name and physical properties
    ///
    /// The global is directly registered into the event loop, and this function
    /// returns the state token allowing you to access it, as well as the global handle,
    /// in case you wish to remove this global in the future.
    pub fn new<L>(
        display: &mut Display,
        name: String,
        physical: PhysicalProperties,
        logger: L,
    ) -> (Output, Global<WlOutput>)
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "output_handler"));

        info!(log, "Creating new wl_output"; "name" => &name);

        let inner = Arc::new(Mutex::new(Inner {
            name,
            log,
            instances: Vec::new(),
            physical,
            location: (0, 0),
            transform: Transform::Normal,
            scale: 1,
            modes: Vec::new(),
            current_mode: None,
            preferred_mode: None,
        }));

        let output = Output { inner: inner.clone() };

        let global = display.create_global(
            3,
            Filter::new(move |(output, _version): (Main<WlOutput>, _), _, _| {
                output.assign_destructor(Filter::new(|output: WlOutput, _, _| {
                    let inner = output.as_ref().user_data().get::<Arc<Mutex<Inner>>>().unwrap();
                    inner
                        .lock()
                        .unwrap()
                        .instances
                        .retain(|o| !o.as_ref().equals(&output.as_ref()));
                }));
                output.as_ref().user_data().set_threadsafe({
                    let inner = inner.clone();
                    move || inner
                });
                inner.lock().unwrap().new_global(output.deref().clone());
            }),
        );

        (output, global)
    }

    /// Sets the preferred mode of this output
    ///
    /// If the provided mode was not previously known to this output, it is added to its
    /// internal list.
    pub fn set_preferred(&self, mode: Mode) {
        let mut inner = self.inner.lock().unwrap();
        inner.preferred_mode = Some(mode);
        if inner.modes.iter().find(|&m| *m == mode).is_none() {
            inner.modes.push(mode);
        }
    }

    /// Adds a mode to the list of known modes to this output
    pub fn add_mode(&self, mode: Mode) {
        let mut inner = self.inner.lock().unwrap();
        if inner.modes.iter().find(|&m| *m == mode).is_none() {
            inner.modes.push(mode);
        }
    }

    /// Removes a mode from the list of known modes
    ///
    /// It will not de-advertise it from existing clients (the protocol does not
    /// allow it), but it won't be advertised to now clients from now on.
    pub fn delete_mode(&self, mode: Mode) {
        let mut inner = self.inner.lock().unwrap();
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
    /// You can changed the current mode, transform status or scale of this output. Providing
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
        new_scale: Option<i32>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(mode) = new_mode {
            if inner.modes.iter().find(|&m| *m == mode).is_none() {
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
        for output in &inner.instances {
            if let Some(mode) = new_mode {
                output.mode(flags, mode.width, mode.height, mode.refresh);
            }
            if new_transform.is_some() {
                inner.send_geometry(output);
            }
            if let Some(scale) = new_scale {
                if output.as_ref().version() >= 2 {
                    output.scale(scale);
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
            .lock()
            .unwrap()
            .instances
            .iter()
            .any(|o| o.as_ref().equals(output.as_ref()))
    }
}
