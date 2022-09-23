//! Output
//!
//! This module provides a type helping you to abstract over various
//! properties, that make up an [`Output`] of your compositor.
//!
//! # How to use it
//!
//! You need to instantiate an [`Output`].
//!
//! To advertise a new output global to wayland clients take a look at [`crate::wayland::output`],
//! you need to have the `wayland_frontend` feature enabled to use it.
//!
//! Additionally outputs are used to by the desktop abstractions (see [`crate::desktop`], needs the
//! `desktop` feature) to represent views into your window grid.
//!
//! You can use the returned [`Output`] to change
//! the properties of your output (if the current resolution mode changes for example).
//! These may influence how contents will be rendered to your output, when used in conjunction
//! with the desktop abstractions.
//!
//! You can attach additional properties to your `Output`s by using [`Output::user_data`].
//!
//! ```
//! # extern crate smithay;
//! use smithay::output::{Output, PhysicalProperties, Scale, Mode, Subpixel};
//! use smithay::utils::Transform;
//!
//! // Create the Output with given name and physical properties.
//! let output = Output::new(
//!     "output-0".into(), // the name of this output,
//!     PhysicalProperties {
//!         size: (200, 150).into(),        // dimensions (width, height) in mm
//!         subpixel: Subpixel::HorizontalRgb,  // subpixel information
//!         make: "Screens Inc".into(),     // make of the monitor
//!         model: "Monitor Ultra".into(),  // model of the monitor
//!     },
//!     None // insert a logger here
//! );
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
//! ```

use std::{
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};

use slog::{info, o};

#[cfg(feature = "wayland_frontend")]
use crate::wayland::output::xdg::XdgOutput;
#[cfg(feature = "wayland_frontend")]
use wayland_server::{backend::WeakHandle, protocol::wl_output::WlOutput};

use crate::utils::{user_data::UserDataMap, Logical, Physical, Point, Raw, Size, Transform};

/// An output mode
///
/// A possible combination of dimensions and refresh rate for an output.
///
/// This should only describe the characteristics of the video driver,
/// not taking into account any global scaling.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Mode {
    /// The size of the mode, in pixels
    pub size: Size<i32, Physical>,
    /// The refresh rate in millihertz
    ///
    /// `1000` is one fps (frame per second), `2000` is 2 fps, etc...
    pub refresh: i32,
}

/// Subpixel geometry information
///
/// This enumeration describes how the physical pixels on an output are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subpixel {
    /// Unknown subpixel geometry
    Unknown,
    /// No subpixel geometry
    None,
    /// Subpixels are arranged horizontally starting with
    /// red, then green, last blue
    HorizontalRgb,
    /// Subpixels are arranged horizontally starting with
    /// blue, then green, last red
    HorizontalBgr,
    /// Subpixels are arranged vertically starting with
    /// red, then green, last blue
    VerticalRgb,
    /// Subpixels are arranged verically starting with
    /// blue, then green, last red
    VerticalBgr,
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
    pub(crate) name: String,
    pub(crate) description: String,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) instances: Vec<WlOutput>,
    #[cfg(feature = "wayland_frontend")]
    pub(crate) handle: Option<WeakHandle>,
    pub(crate) physical: PhysicalProperties,
    pub(crate) location: Point<i32, Logical>,
    pub(crate) transform: Transform,
    pub(crate) scale: Scale,
    pub(crate) modes: Vec<Mode>,
    pub(crate) current_mode: Option<Mode>,
    pub(crate) preferred_mode: Option<Mode>,

    #[cfg(feature = "wayland_frontend")]
    pub(crate) xdg_output: Option<XdgOutput>,
    #[allow(dead_code)]
    pub(crate) log: ::slog::Logger,
}

/// Clonable Data of an Outut
#[derive(Debug, Clone)]
pub struct OutputData {
    pub(crate) inner: Arc<(Mutex<Inner>, UserDataMap)>,
}

/// An abstract output.
///
/// This handle is stored in the event loop, and allows you to notify clients
/// about any change in the properties of this output.
#[derive(Debug, Clone)]
pub struct Output {
    pub(crate) data: OutputData,
}

impl Output {
    /// Create a new output with given name and physical properties.
    pub fn new<L>(name: String, physical: PhysicalProperties, logger: L) -> Output
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "output_handler"));

        info!(log, "Creating new wl_output"; "name" => &name);

        let data = OutputData {
            inner: Arc::new((
                Mutex::new(Inner {
                    name: name.clone(),
                    description: format!("{} - {} - {}", physical.make, physical.model, name),
                    #[cfg(feature = "wayland_frontend")]
                    instances: Vec::new(),
                    #[cfg(feature = "wayland_frontend")]
                    handle: None,
                    physical,
                    location: (0, 0).into(),
                    transform: Transform::Normal,
                    scale: Scale::Integer(1),
                    modes: Vec::new(),
                    current_mode: None,
                    preferred_mode: None,
                    #[cfg(feature = "wayland_frontend")]
                    xdg_output: None,
                    log,
                }),
                UserDataMap::default(),
            )),
        };

        Output { data }
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

    /// Returns the currenly set scale of the output
    pub fn current_scale(&self) -> Scale {
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
        new_mode: Option<Mode>,
        new_transform: Option<Transform>,
        new_scale: Option<Scale>,
        new_location: Option<Point<i32, Logical>>,
    ) {
        {
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
            if let Some(new_location) = new_location {
                inner.location = new_location;
            }
        }

        #[cfg(feature = "wayland_frontend")]
        self.wl_change_current_state(new_mode, new_transform.map(Into::into), new_scale, new_location)
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

impl Hash for Output {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.data.inner).hash(state);
    }
}
