//! Output advertizing capabilities
//!
//! This module provides a type helping you to handle the advertizing
//! of your compositor's output and their capabilities to your client,
//! as well as mapping your clients output request to your physical
//! outputs.
//!
//! # How to use it
//!
//! You need to instanciate an `Output` for each output global you want
//! to advertize to clients.
//!
//! Just insert it in your event loop using the `Output::new(..)` method.
//! It returns a state token that gives you access to the `Output` in order
//! to change it if needed (if the current resolution mode changes for example),
//! it'll automatically forward any changes to the clients.
//!
//! ```
//! # extern crate wayland_server;
//! # extern crate smithay;
//! use smithay::wayland::output::{Output, PhysicalProperties, Mode};
//! use wayland_server::protocol::wl_output;
//!
//! # fn main() {
//! # let (display, mut event_loop) = wayland_server::create_display();
//! // Insert the Output with given name and physical properties
//! let (output_state_token, _output_global) = Output::new(
//!     &mut event_loop, // the event loop
//!     "output-0".into(), // the name of this output,
//!     PhysicalProperties {
//!         width: 200, // width in mm
//!         height: 150, // height in mm,
//!         subpixel: wl_output::Subpixel::HorizontalRgb, // subpixel information
//!         maker: "Screens Inc".into(), // manufacturer of the monitor
//!         model: "Monitor Ultra".into(), // model of the monitor
//!     },
//!     None // insert a logger here
//! );
//! // Now you can configure it
//! {
//!     let output = event_loop.state().get_mut(&output_state_token);
//!     // set the current state
//!     output.change_current_state(
//!         Some(Mode { width: 1902, height: 1080, refresh: 60000 }), // the resolution mode,
//!         Some(wl_output::Transform::Normal), // global screen transformation
//!         Some(1), // global screen scaling factor
//!     );
//!     // set the prefered mode
//!     output.set_preferred(Mode { width: 1920, height: 1080, refresh: 60000 });
//!     // add other supported modes
//!     output.add_mode(Mode { width: 800, height: 600, refresh: 60000 });
//!     output.add_mode(Mode { width: 1024, height: 768, refresh: 60000 });
//! }
//! # }
//! ```

use wayland_server::{Client, EventLoop, EventLoopHandle, Global, Liveness, Resource, StateToken};
use wayland_server::protocol::wl_output;

#[derive(Copy, Clone, PartialEq)]
/// An output mode
///
/// A possible combination of dimensions and refresh rate for an output.
///
/// This should only describe the characteristics of the video driver,
/// not taking into account any global scaling.
pub struct Mode {
    /// The width in pixels
    pub width: i32,
    /// The height in pixels
    pub height: i32,
    /// The refresh rate in mili-Hertz
    ///
    /// `1000` is one fps (frame per second), `2000` is 2 fps, etc...
    pub refresh: i32,
}

/// The physical properties of an output
pub struct PhysicalProperties {
    /// The width in milimeters
    pub width: i32,
    /// The height in milimeters
    pub height: i32,
    /// The subpixel geometry
    pub subpixel: wl_output::Subpixel,
    /// Textual representation of the manufacturer
    pub maker: String,
    /// Textual representation of the model
    pub model: String,
}

/// An output as seen by the clients
///
/// This handle is stored in the events loop, and allows you to notify clients
/// about any change in the properties of this output.
pub struct Output {
    name: String,
    log: ::slog::Logger,
    instances: Vec<wl_output::WlOutput>,
    physical: PhysicalProperties,
    location: (i32, i32),
    transform: wl_output::Transform,
    scale: i32,
    modes: Vec<Mode>,
    current_mode: Option<Mode>,
    preferred_mode: Option<Mode>,
}

impl Output {
    /// Create a new output global with given name and physical properties
    ///
    /// The global is directly registered into the eventloop, and this function
    /// returns the state token allowing you to access it, as well as the global handle,
    /// in case you whish to remove this global in  the future.
    pub fn new<L>(
        evl: &mut EventLoop, name: String, physical: PhysicalProperties, logger: L)
        -> (
            StateToken<Output>,
            Global<wl_output::WlOutput, StateToken<Output>>,
        )
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "output_handler"));

        info!(log, "Creating new wl_output"; "name" => &name);

        let token = evl.state().insert(Output {
            name: name,
            log: log,
            instances: Vec::new(),
            physical: physical,
            location: (0, 0),
            transform: wl_output::Transform::Normal,
            scale: 1,
            modes: Vec::new(),
            current_mode: None,
            preferred_mode: None,
        });

        let global = evl.register_global(3, output_bind, token.clone());

        (token, global)
    }

    fn new_global(&mut self, output: wl_output::WlOutput) {
        trace!(self.log, "New global instanciated.");

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
            let mut flags = wl_output::Mode::empty();
            if Some(mode) == self.current_mode {
                flags |= wl_output::Current;
            }
            if Some(mode) == self.preferred_mode {
                flags |= wl_output::Preferred;
            }
            output.mode(flags, mode.width, mode.height, mode.refresh);
        }
        if output.version() >= 2 {
            output.scale(self.scale);
            output.done();
        }

        self.instances.push(output);
    }

    fn send_geometry(&self, output: &wl_output::WlOutput) {
        output.geometry(
            self.location.0,
            self.location.1,
            self.physical.width,
            self.physical.height,
            self.physical.subpixel,
            self.physical.maker.clone(),
            self.physical.model.clone(),
            self.transform,
        );
    }

    /// Sets the preferred mode of this output
    ///
    /// If the provided mode was not previously known to this output, it is added to its
    /// internal list.
    pub fn set_preferred(&mut self, mode: Mode) {
        self.preferred_mode = Some(mode);
        if self.modes.iter().find(|&m| *m == mode).is_none() {
            self.modes.push(mode);
        }
    }

    /// Adds a mode to the list of known modes to this output
    pub fn add_mode(&mut self, mode: Mode) {
        if self.modes.iter().find(|&m| *m == mode).is_none() {
            self.modes.push(mode);
        }
    }

    /// Removes a mode from the list of known modes
    ///
    /// It will not de-advertize it from existing clients (the protocol does not
    /// allow it), but it won't be advertized to now clients from now on.
    pub fn delete_mode(&mut self, mode: Mode) {
        self.modes.retain(|&m| m != mode);
        if self.current_mode == Some(mode) {
            self.current_mode = None;
        }
        if self.preferred_mode == Some(mode) {
            self.preferred_mode = None;
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
    pub fn change_current_state(&mut self, new_mode: Option<Mode>,
                                new_transform: Option<wl_output::Transform>, new_scale: Option<i32>) {
        if let Some(mode) = new_mode {
            if self.modes.iter().find(|&m| *m == mode).is_none() {
                self.modes.push(mode);
            }
            self.current_mode = new_mode;
        }
        if let Some(transform) = new_transform {
            self.transform = transform;
        }
        if let Some(scale) = new_scale {
            self.scale = scale;
        }
        let mut flags = wl_output::Current;
        if self.preferred_mode == new_mode {
            flags |= wl_output::Preferred;
        }
        for output in &self.instances {
            if let Some(mode) = new_mode {
                output.mode(flags, mode.width, mode.height, mode.refresh);
            }
            if new_transform.is_some() {
                self.send_geometry(output);
            }
            if let Some(scale) = new_scale {
                if output.version() >= 2 {
                    output.scale(scale);
                }
            }
            if output.version() >= 2 {
                output.done();
            }
        }
    }

    /// Chech is given wl_output instance is managed by this `Output`.
    pub fn owns(&self, output: &wl_output::WlOutput) -> bool {
        self.instances.iter().any(|o| o.equals(output))
    }

    /// Cleanup internal `wl_output` instances list
    ///
    /// Clients do not necessarily notify the server on the destruction
    /// of their `wl_output` instances. This can lead to accumulation of
    /// stale values in the internal instances list. This methods delete
    /// them.
    ///
    /// It can be good to call this regularly (but not necessarily very often).
    pub fn cleanup(&mut self) {
        self.instances.retain(|o| o.status() == Liveness::Alive);
    }
}

fn output_bind(evlh: &mut EventLoopHandle, token: &mut StateToken<Output>, _: &Client,
               global: wl_output::WlOutput) {
    evlh.register(&global, output_implementation(), token.clone(), None);
    evlh.state().get_mut(token).new_global(global);
}

fn output_implementation() -> wl_output::Implementation<StateToken<Output>> {
    wl_output::Implementation {
        release: |evlh, token, _, output| {
            evlh.state()
                .get_mut(token)
                .instances
                .retain(|o| !o.equals(output));
        },
    }
}
