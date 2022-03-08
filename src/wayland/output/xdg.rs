//! XDG Output advertising capabilities
//!
//! This protocol is meant for describing outputs in a way
//! which is more in line with the concept of an output on desktop oriented systems.

use std::sync::{Arc, Mutex};

use slog::{o, trace};
use wayland_protocols::unstable::xdg_output::v1::server::zxdg_output_v1::ZxdgOutputV1;
use wayland_server::{protocol::wl_output::WlOutput, DisplayHandle, Resource};

use crate::utils::{Logical, Physical, Point, Size};

use super::Mode;

#[derive(Debug)]
pub(super) struct Inner {
    name: String,
    description: String,
    logical_position: Point<i32, Logical>,

    physical_size: Option<Size<i32, Physical>>,
    scale: i32,

    pub instances: Vec<ZxdgOutputV1>,
    _log: ::slog::Logger,
}

#[derive(Debug, Clone)]
pub(super) struct XdgOutput {
    pub(super) inner: Arc<Mutex<Inner>>,
}

impl XdgOutput {
    pub(super) fn new(output: &super::Inner, log: ::slog::Logger) -> Self {
        let log = log.new(o!("smithay_module" => "xdg_output_handler"));

        trace!(log, "Creating new xdg_output"; "name" => &output.name);

        let physical_size = output.current_mode.map(|mode| mode.size);

        Self {
            inner: Arc::new(Mutex::new(Inner {
                name: output.name.clone(),
                description: output.description.clone(),
                logical_position: output.location,

                physical_size,
                scale: output.scale,

                instances: Vec::new(),
                _log: log,
            })),
        }
    }

    pub(super) fn add_instance(
        &self,
        dh: &mut DisplayHandle<'_>,
        xdg_output: &ZxdgOutputV1,
        wl_output: &WlOutput,
    ) {
        let mut inner = self.inner.lock().unwrap();

        xdg_output.logical_position(dh, inner.logical_position.x, inner.logical_position.y);

        if let Some(size) = inner.physical_size {
            let logical_size = size.to_logical(inner.scale);
            xdg_output.logical_size(dh, logical_size.w, logical_size.h);
        }

        if xdg_output.version() >= 2 {
            xdg_output.name(dh, inner.name.clone());
            xdg_output.description(dh, inner.description.clone());
        }

        // xdg_output.done() is deprecated since version 3
        if xdg_output.version() < 3 {
            xdg_output.done(dh);
        }

        wl_output.done(dh);

        inner.instances.push(xdg_output.clone());
    }

    pub(super) fn change_current_state(
        &self,
        dh: &mut DisplayHandle<'_>,
        new_mode: Option<Mode>,
        new_scale: Option<i32>,
        new_location: Option<Point<i32, Logical>>,
    ) {
        let mut output = self.inner.lock().unwrap();

        if let Some(new_mode) = new_mode {
            output.physical_size = Some(new_mode.size);
        }
        if let Some(new_scale) = new_scale {
            output.scale = new_scale;
        }
        if let Some(new_location) = new_location {
            output.logical_position = new_location;
        }

        for instance in output.instances.iter() {
            if new_mode.is_some() | new_scale.is_some() {
                if let Some(size) = output.physical_size {
                    let logical_size = size.to_logical(output.scale);
                    instance.logical_size(dh, logical_size.w, logical_size.h);
                }
            }

            if new_location.is_some() {
                instance.logical_position(dh, output.logical_position.x, output.logical_position.y);
            }

            // xdg_output.done() is deprecated since version 3
            if instance.version() < 3 {
                instance.done(dh);
            }

            // No need for wl_output.done() here, it will be called by caller (super::Output::change_current_state)
        }
    }
}
