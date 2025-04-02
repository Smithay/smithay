//! XDG Output advertising capabilities
//!
//! This protocol is meant for describing outputs in a way
//! which is more in line with the concept of an output on desktop oriented systems.

use std::sync::{atomic::Ordering, Arc, Mutex};

use tracing::trace;
use wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_v1::ZxdgOutputV1;
use wayland_server::{protocol::wl_output::WlOutput, Resource, Weak};

use crate::utils::{Logical, Physical, Point, Size, Transform};

use super::{Mode, OutputUserData, Scale, XdgOutputUserData};

#[derive(Debug)]
pub(crate) struct Inner {
    name: String,
    description: String,
    pub(super) logical_position: Point<i32, Logical>,

    pub(super) physical_size: Option<Size<i32, Physical>>,
    pub(super) scale: Scale,
    transform: Transform,

    pub instances: Vec<Weak<ZxdgOutputV1>>,
}

#[derive(Debug, Clone)]
pub(crate) struct XdgOutput {
    pub(crate) inner: Arc<Mutex<Inner>>,
}

impl XdgOutput {
    pub(super) fn new(output: &super::Inner) -> Self {
        trace!(name = output.name, "Creating new xdg_output");

        let physical_size = output.current_mode.map(|mode| mode.size);

        Self {
            inner: Arc::new(Mutex::new(Inner {
                name: output.name.clone(),
                description: output.description.clone(),
                logical_position: output.location,

                physical_size,
                scale: output.scale,
                transform: output.transform,

                instances: Vec::new(),
            })),
        }
    }

    pub(super) fn add_instance(&self, xdg_output: &ZxdgOutputV1, wl_output: &WlOutput) {
        let mut inner = self.inner.lock().unwrap();
        let client_scale = wl_output
            .data::<OutputUserData>()
            .unwrap()
            .client_scale
            .load(Ordering::Acquire);

        let logical_position = inner.logical_position.to_client_precise_round(client_scale);
        xdg_output.logical_position(logical_position.x, logical_position.y);

        if let Some(size) = inner.physical_size {
            let logical_size = size
                .to_f64()
                .to_logical(inner.scale.fractional_scale())
                .to_client(client_scale)
                .to_i32_round();
            let transformed_size = inner.transform.transform_size(logical_size);
            xdg_output.logical_size(transformed_size.w, transformed_size.h);
        }

        if xdg_output.version() >= 2 {
            xdg_output.name(inner.name.clone());
            xdg_output.description(inner.description.clone());
        }

        // xdg_output.done() is deprecated since version 3
        if xdg_output.version() < 3 {
            xdg_output.done();
        }

        wl_output.done();

        inner.instances.push(xdg_output.downgrade());
    }

    pub(super) fn change_current_state(
        &self,
        new_mode: Option<Mode>,
        new_scale: Option<Scale>,
        new_location: Option<Point<i32, Logical>>,
        new_transform: Option<impl Into<Transform>>,
    ) {
        let mut output = self.inner.lock().unwrap();

        let new_transform = new_transform.map(|x| x.into());

        if let Some(new_mode) = new_mode {
            output.physical_size = Some(new_mode.size);
        }
        if let Some(new_scale) = new_scale {
            output.scale = new_scale;
        }
        if let Some(new_location) = new_location {
            output.logical_position = new_location;
        }
        if let Some(new_transform) = new_transform {
            output.transform = new_transform;
        }

        for instance in output.instances.iter() {
            let Ok(instance) = instance.upgrade() else {
                continue;
            };

            let data = instance.data::<XdgOutputUserData>().unwrap();
            let client_scale = data.client_scale.load(Ordering::Acquire);
            let scale_changed = client_scale != data.last_client_scale.swap(client_scale, Ordering::AcqRel);

            if new_mode.is_some() || new_scale.is_some() || new_transform.is_some() || scale_changed {
                if let Some(size) = output.physical_size {
                    let logical_size = size
                        .to_f64()
                        .to_logical(output.scale.fractional_scale())
                        .to_client(client_scale)
                        .to_i32_round();
                    let transformed_size = output.transform.transform_size(logical_size);
                    instance.logical_size(transformed_size.w, transformed_size.h);
                }
            }

            if new_location.is_some() || scale_changed {
                let logical_position = output.logical_position.to_client_precise_round(client_scale);
                instance.logical_position(logical_position.x, logical_position.y);
            }

            // xdg_output.done() is deprecated since version 3
            if instance.version() < 3 {
                instance.done();
            }

            // No need for wl_output.done() here, it will be called by caller (super::Output::change_current_state)
        }
    }
}
