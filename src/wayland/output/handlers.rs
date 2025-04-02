use std::sync::{atomic::Ordering, Arc};

use atomic_float::AtomicF64;
use tracing::{trace, warn, warn_span};
use wayland_protocols::xdg::xdg_output::zv1::server::{
    zxdg_output_manager_v1::{self, ZxdgOutputManagerV1},
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_server::{
    protocol::wl_output::{self, Mode as WMode, WlOutput},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::wayland::compositor::CompositorHandler;

use super::{xdg::XdgOutput, Output, OutputHandler, OutputManagerState, OutputUserData, WlOutputData};

/*
 * Wl Output
 */

impl<D> GlobalDispatch<WlOutput, WlOutputData, D> for OutputManagerState
where
    D: GlobalDispatch<WlOutput, WlOutputData>,
    D: Dispatch<WlOutput, OutputUserData>,
    D: OutputHandler,
    D: CompositorHandler,
    D: 'static,
{
    fn bind(
        state: &mut D,
        _dh: &DisplayHandle,
        client: &Client,
        resource: New<WlOutput>,
        global_data: &WlOutputData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let client_scale = state.client_compositor_state(client).clone_client_scale();
        let output = data_init.init(
            resource,
            OutputUserData {
                output: global_data.output.downgrade(),
                last_client_scale: AtomicF64::new(client_scale.load(Ordering::Acquire)),
                client_scale,
            },
        );

        let mut inner = global_data.output.inner.0.lock().unwrap();

        let span = warn_span!("output_bind", name = inner.name);
        let _enter = span.enter();

        trace!("New WlOutput global instantiated");

        if inner.modes.is_empty() {
            warn!("Output is used with no modes set");
        }
        if inner.current_mode.is_none() {
            warn!("Output is used with no current mod set");
        }
        if inner.preferred_mode.is_none() {
            warn!("Output is used with not preferred mode set");
        }

        inner.send_geometry_to(&output);

        for &mode in &inner.modes {
            let mut flags = WMode::empty();
            if Some(mode) == inner.current_mode {
                flags |= WMode::Current;
            }
            if Some(mode) == inner.preferred_mode {
                flags |= WMode::Preferred;
            }
            output.mode(flags, mode.size.w, mode.size.h, mode.refresh);
        }

        if output.version() >= 4 {
            output.name(inner.name.clone());
            output.description(inner.description.clone())
        }

        if output.version() >= 2 {
            output.scale(inner.scale.integer_scale());
            output.done();
        }

        // Send enter for surfaces already on this output.
        for surface in &inner.surfaces {
            if let Ok(surface) = surface.upgrade() {
                if surface.client().as_ref() == Some(client) {
                    surface.enter(&output);
                }
            }
        }

        inner.instances.push(output.downgrade());

        drop(inner);
        state.output_bound(global_data.output.clone(), output);
    }
}

impl<D> Dispatch<WlOutput, OutputUserData, D> for OutputManagerState
where
    D: Dispatch<WlOutput, OutputUserData>,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &WlOutput,
        _request: wl_output::Request,
        _data: &OutputUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        output: &WlOutput,
        data: &OutputUserData,
    ) {
        if let Some(o) = data.output.upgrade() {
            o.inner
                .0
                .lock()
                .unwrap()
                .instances
                .retain(|o| o.id() != output.id());
        }
    }
}

/*
 * XDG Output
 */

impl<D> GlobalDispatch<ZxdgOutputManagerV1, (), D> for OutputManagerState
where
    D: GlobalDispatch<ZxdgOutputManagerV1, ()>,
    D: Dispatch<ZxdgOutputManagerV1, ()>,
    D: Dispatch<ZxdgOutputV1, XdgOutputUserData>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZxdgOutputManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ZxdgOutputManagerV1, (), D> for OutputManagerState
where
    D: Dispatch<ZxdgOutputManagerV1, ()>,
    D: Dispatch<ZxdgOutputV1, XdgOutputUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        client: &Client,
        _resource: &ZxdgOutputManagerV1,
        request: zxdg_output_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zxdg_output_manager_v1::Request::GetXdgOutput {
                id,
                output: wl_output,
            } => {
                let output = Output::from_resource(&wl_output).unwrap();
                let mut inner = output.inner.0.lock().unwrap();

                let xdg_output = XdgOutput::new(&inner);

                if inner.xdg_output.is_none() {
                    inner.xdg_output = Some(xdg_output.clone());
                }

                let client_scale = state.client_compositor_state(client).clone_client_scale();
                let id = data_init.init(
                    id,
                    XdgOutputUserData {
                        xdg_output,
                        last_client_scale: AtomicF64::new(client_scale.load(Ordering::Acquire)),
                        client_scale,
                    },
                );

                inner.xdg_output.as_ref().unwrap().add_instance(&id, &wl_output);
            }
            zxdg_output_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

/// User data of Xdg Output
#[derive(Debug)]
pub struct XdgOutputUserData {
    xdg_output: XdgOutput,
    pub(super) last_client_scale: AtomicF64,
    pub(super) client_scale: Arc<AtomicF64>,
}

impl<D> Dispatch<ZxdgOutputV1, XdgOutputUserData, D> for OutputManagerState
where
    D: Dispatch<ZxdgOutputV1, XdgOutputUserData>,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ZxdgOutputV1,
        _request: zxdg_output_v1::Request,
        _data: &XdgOutputUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        xdg_output: &ZxdgOutputV1,
        data: &XdgOutputUserData,
    ) {
        data.xdg_output
            .inner
            .lock()
            .unwrap()
            .instances
            .retain(|o| o.id() != xdg_output.id());
    }
}
