use slog::{trace, warn};
use wayland_protocols::unstable::xdg_output::v1::server::{
    zxdg_output_manager_v1::{self, ZxdgOutputManagerV1},
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_server::{
    protocol::wl_output::{self, Mode as WMode, WlOutput},
    DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase, Dispatch,
    GlobalDispatch, Resource,
};

use super::{xdg::XdgOutput, Output, OutputGlobalData, OutputManagerState, OutputUserData};

/*
 * Wl Output
 */

impl DelegateGlobalDispatchBase<WlOutput> for OutputManagerState {
    type GlobalData = OutputGlobalData;
}

impl<D> DelegateGlobalDispatch<WlOutput, D> for OutputManagerState
where
    D: GlobalDispatch<WlOutput, GlobalData = OutputGlobalData>,
    D: Dispatch<WlOutput, UserData = OutputUserData>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        dh: &mut wayland_server::DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: wayland_server::New<WlOutput>,
        global_data: &Self::GlobalData,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let output = data_init.init(
            resource,
            OutputUserData {
                global_data: global_data.clone(),
            },
        );

        let mut inner = global_data.inner.0.lock().unwrap();

        trace!(inner.log, "New WlOutput global instantiated."; "name" => &inner.name);

        if inner.modes.is_empty() {
            warn!(inner.log, "Output is used with no modes set"; "name" => &inner.name);
        }
        if inner.current_mode.is_none() {
            warn!(inner.log, "Output is used with no current mod set"; "name" => &inner.name);
        }
        if inner.preferred_mode.is_none() {
            warn!(inner.log, "Output is used with not preferred mode set"; "name" => &inner.name);
        }

        inner.send_geometry_to(dh, &output);

        for &mode in &inner.modes {
            let mut flags = WMode::empty();
            if Some(mode) == inner.current_mode {
                flags |= WMode::Current;
            }
            if Some(mode) == inner.preferred_mode {
                flags |= WMode::Preferred;
            }
            output.mode(dh, flags, mode.size.w, mode.size.h, mode.refresh);
        }

        if output.version() >= 4 {
            output.name(dh, inner.name.clone());
            output.description(dh, inner.description.clone())
        }

        if output.version() >= 2 {
            output.scale(dh, inner.scale);
            output.done(dh);
        }

        inner.instances.push(output);
    }
}

impl DelegateDispatchBase<WlOutput> for OutputManagerState {
    type UserData = OutputUserData;
}

impl<D> DelegateDispatch<WlOutput, D> for OutputManagerState
where
    D: Dispatch<WlOutput, UserData = OutputUserData>,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlOutput,
        _request: wl_output::Request,
        _data: &Self::UserData,
        _dhandle: &mut wayland_server::DisplayHandle<'_>,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        object_id: wayland_server::backend::ObjectId,
        data: &Self::UserData,
    ) {
        data.global_data
            .inner
            .0
            .lock()
            .unwrap()
            .instances
            .retain(|o| o.id() != object_id);
    }
}

/*
 * XDG Output
 */

impl DelegateGlobalDispatchBase<ZxdgOutputManagerV1> for OutputManagerState {
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<ZxdgOutputManagerV1, D> for OutputManagerState
where
    D: GlobalDispatch<ZxdgOutputManagerV1, GlobalData = ()>,
    D: Dispatch<ZxdgOutputManagerV1, UserData = ()>,
    D: Dispatch<ZxdgOutputV1, UserData = XdgOutputUserData>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &mut wayland_server::DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: wayland_server::New<ZxdgOutputManagerV1>,
        _global_data: &Self::GlobalData,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl DelegateDispatchBase<ZxdgOutputManagerV1> for OutputManagerState {
    type UserData = ();
}

impl<D> DelegateDispatch<ZxdgOutputManagerV1, D> for OutputManagerState
where
    D: Dispatch<ZxdgOutputManagerV1, UserData = ()>,
    D: Dispatch<ZxdgOutputV1, UserData = XdgOutputUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZxdgOutputManagerV1,
        request: zxdg_output_manager_v1::Request,
        _data: &Self::UserData,
        dh: &mut wayland_server::DisplayHandle<'_>,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zxdg_output_manager_v1::Request::GetXdgOutput {
                id,
                output: wl_output,
            } => {
                let output = Output::from_resource(&wl_output).unwrap();
                let mut inner = output.data.inner.0.lock().unwrap();

                let xdg_output = XdgOutput::new(&inner, inner.log.clone());

                if inner.xdg_output.is_none() {
                    inner.xdg_output = Some(xdg_output.clone());
                }

                let id = data_init.init(id, XdgOutputUserData { xdg_output });

                inner
                    .xdg_output
                    .as_ref()
                    .unwrap()
                    .add_instance(dh, &id, &wl_output);
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
}

impl DelegateDispatchBase<ZxdgOutputV1> for OutputManagerState {
    type UserData = XdgOutputUserData;
}

impl<D> DelegateDispatch<ZxdgOutputV1, D> for OutputManagerState
where
    D: Dispatch<ZxdgOutputV1, UserData = XdgOutputUserData>,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZxdgOutputV1,
        _request: zxdg_output_v1::Request,
        _data: &Self::UserData,
        _dhandle: &mut wayland_server::DisplayHandle<'_>,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        object_id: wayland_server::backend::ObjectId,
        data: &Self::UserData,
    ) {
        data.xdg_output
            .inner
            .lock()
            .unwrap()
            .instances
            .retain(|o| o.id() != object_id);
    }
}
