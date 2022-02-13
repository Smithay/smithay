//! XDG Output advertising capabilities
//!
//! This protocol is meant for describing outputs in a way
//! which is more in line with the concept of an output on desktop oriented systems.

use std::{
    ops::Deref as _,
    sync::{Arc, Mutex},
};

use slog::{o, trace};
use wayland_protocols::unstable::xdg_output::v1::server::{
    zxdg_output_manager_v1::{self, ZxdgOutputManagerV1},
    zxdg_output_v1::ZxdgOutputV1,
};
use wayland_server::{protocol::wl_output::WlOutput, Display, Filter, Global, Main};

use crate::utils::{Logical, Physical, Point, Size};

use super::{Mode, Output};

#[derive(Debug)]
struct Inner {
    name: String,
    description: String,
    logical_position: Point<i32, Logical>,

    physical_size: Option<Size<i32, Physical>>,
    scale: i32,

    instances: Vec<ZxdgOutputV1>,
    _log: ::slog::Logger,
}

#[derive(Debug, Clone)]
pub(super) struct XdgOutput {
    inner: Arc<Mutex<Inner>>,
}

impl XdgOutput {
    fn new(output: &super::Inner, log: ::slog::Logger) -> Self {
        trace!(log, "Creating new xdg_output"; "name" => &output.name);

        let description = format!(
            "{} - {} - {}",
            output.physical.make, output.physical.model, output.name
        );

        let physical_size = output.current_mode.map(|mode| mode.size);

        Self {
            inner: Arc::new(Mutex::new(Inner {
                name: output.name.clone(),
                description,
                logical_position: output.location,

                physical_size,
                scale: output.scale,

                instances: Vec::new(),
                _log: log,
            })),
        }
    }

    fn add_instance(&self, xdg_output: Main<ZxdgOutputV1>, wl_output: &WlOutput) {
        let mut inner = self.inner.lock().unwrap();

        xdg_output.logical_position(inner.logical_position.x, inner.logical_position.y);

        if let Some(size) = inner.physical_size {
            let logical_size = size.to_logical(inner.scale);
            xdg_output.logical_size(logical_size.w, logical_size.h);
        }

        if xdg_output.as_ref().version() >= 2 {
            xdg_output.name(inner.name.clone());
            xdg_output.description(inner.description.clone());
        }

        // xdg_output.done() is deprecated since version 3
        if xdg_output.as_ref().version() < 3 {
            xdg_output.done();
        }

        wl_output.done();

        xdg_output.quick_assign(|_, _, _| {});
        xdg_output.assign_destructor(Filter::new(|xdg_output: ZxdgOutputV1, _, _| {
            let inner = &xdg_output.as_ref().user_data().get::<XdgOutput>().unwrap().inner;
            inner
                .lock()
                .unwrap()
                .instances
                .retain(|o| !o.as_ref().equals(xdg_output.as_ref()));
        }));
        xdg_output.as_ref().user_data().set_threadsafe({
            let xdg_output = self.clone();
            move || xdg_output
        });

        inner.instances.push(xdg_output.deref().clone());
    }

    pub(super) fn change_current_state(
        &self,
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
                    instance.logical_size(logical_size.w, logical_size.h);
                }
            }

            if new_location.is_some() {
                instance.logical_position(output.logical_position.x, output.logical_position.y);
            }

            // xdg_output.done() is deprecated since version 3
            if instance.as_ref().version() < 3 {
                instance.done();
            }

            // No need for wl_output.done() here, it will be called by caller (super::Output::change_current_state)
        }
    }
}

/// Initialize a xdg output manager global.
pub fn init_xdg_output_manager<L>(display: &mut Display, logger: L) -> Global<ZxdgOutputManagerV1>
where
    L: Into<Option<::slog::Logger>>,
{
    let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "xdg_output_handler"));

    display.create_global(
        3,
        Filter::new(move |(manager, _version): (Main<ZxdgOutputManagerV1>, _), _, _| {
            let log = log.clone();
            manager.quick_assign(move |_, req, _| match req {
                zxdg_output_manager_v1::Request::GetXdgOutput {
                    id,
                    output: wl_output,
                } => {
                    let output = Output::from_resource(&wl_output).unwrap();
                    let mut inner = output.inner.0.lock().unwrap();

                    if inner.xdg_output.is_none() {
                        inner.xdg_output = Some(XdgOutput::new(&inner, log.clone()));
                    }

                    inner.xdg_output.as_ref().unwrap().add_instance(id, &wl_output);
                }
                zxdg_output_manager_v1::Request::Destroy => {
                    // Nothing to do
                }
                _ => {}
            });
        }),
    )
}
