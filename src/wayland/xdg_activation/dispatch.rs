use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use wayland_protocols::staging::xdg_activation::v1::server::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_server::{
    backend::{ClientId, ObjectId},
    Client, DataInit, DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch,
    DelegateGlobalDispatchBase, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use super::{
    ActivationTokenData, TokenBuilder, XdgActivationHandler, XdgActivationState, XdgActivationTokenData,
};

impl DelegateDispatchBase<xdg_activation_v1::XdgActivationV1> for XdgActivationState {
    type UserData = ();
}

impl<D> DelegateDispatch<xdg_activation_v1::XdgActivationV1, D> for XdgActivationState
where
    D: Dispatch<xdg_activation_v1::XdgActivationV1, UserData = Self::UserData>
        + Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, UserData = ActivationTokenData>
        + XdgActivationHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _: &Client,
        _: &xdg_activation_v1::XdgActivationV1,
        request: xdg_activation_v1::Request,
        _: &Self::UserData,
        _: &mut DisplayHandle<'_>,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_activation_v1::Request::Destroy => {}

            xdg_activation_v1::Request::GetActivationToken { id } => {
                data_init.init(
                    id,
                    ActivationTokenData {
                        constructed: AtomicBool::new(false),
                        build: Mutex::new(TokenBuilder {
                            serial: None,
                            app_id: None,
                            surface: None,
                        }),
                        token: Mutex::new(None),
                    },
                );
            }

            xdg_activation_v1::Request::Activate { token, surface } => {
                let token = token.into();

                let activation_state = state.activation_state();

                if let Some(token_data) = activation_state.pending_tokens.remove(&token) {
                    activation_state
                        .activation_requests
                        .insert(token.clone(), (token_data.clone(), surface.clone()));
                    state.request_activation(token, token_data, surface);
                }
            }

            _ => unreachable!(),
        }
    }
}

impl DelegateGlobalDispatchBase<xdg_activation_v1::XdgActivationV1> for XdgActivationState {
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<xdg_activation_v1::XdgActivationV1, D> for XdgActivationState
where
    D: GlobalDispatch<xdg_activation_v1::XdgActivationV1, GlobalData = Self::GlobalData>
        + Dispatch<xdg_activation_v1::XdgActivationV1, UserData = Self::UserData>
        + Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, UserData = ActivationTokenData>
        + XdgActivationHandler
        + 'static,
{
    fn bind(
        _: &mut D,
        _: &mut DisplayHandle<'_>,
        _: &Client,
        resource: New<xdg_activation_v1::XdgActivationV1>,
        _: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl DelegateDispatchBase<xdg_activation_token_v1::XdgActivationTokenV1> for XdgActivationState {
    type UserData = ActivationTokenData;
}

impl<D> DelegateDispatch<xdg_activation_token_v1::XdgActivationTokenV1, D> for XdgActivationState
where
    D: Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, UserData = Self::UserData>
        + XdgActivationHandler,
{
    fn request(
        state: &mut D,
        _: &Client,
        token: &xdg_activation_token_v1::XdgActivationTokenV1,
        request: xdg_activation_token_v1::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
        _: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_activation_token_v1::Request::SetSerial { serial, seat } => {
                if data.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        dh,
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                data.build.lock().unwrap().serial = Some((serial.into(), seat));
            }

            xdg_activation_token_v1::Request::SetAppId { app_id } => {
                if data.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        dh,
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                data.build.lock().unwrap().app_id = Some(app_id);
            }

            xdg_activation_token_v1::Request::SetSurface { surface } => {
                if data.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        dh,
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                data.build.lock().unwrap().surface = Some(surface);
            }

            xdg_activation_token_v1::Request::Commit => {
                if data.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        dh,
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                data.constructed.store(true, Ordering::Relaxed);

                let (activation_token, token_data) = {
                    let mut guard = data.build.lock().unwrap();

                    XdgActivationTokenData::new(
                        guard.serial.take(),
                        guard.app_id.take(),
                        guard.surface.take(),
                    )
                };

                *data.token.lock().unwrap() = Some(activation_token.clone());
                state
                    .activation_state()
                    .pending_tokens
                    .insert(activation_token.clone(), token_data);
                token.done(dh, activation_token.to_string());
            }

            xdg_activation_token_v1::Request::Destroy => {}

            _ => unreachable!(),
        }
    }

    fn destroyed(state: &mut D, _: ClientId, _: ObjectId, data: &Self::UserData) {
        let guard = data.token.lock().unwrap();

        if let Some(token) = &*guard {
            let activation_state = state.activation_state();

            activation_state.pending_tokens.remove(token);

            if let Some((token_data, surface)) = activation_state.activation_requests.remove(token) {
                state.destroy_activation(token.clone(), token_data, surface);
            }
        }
    }
}
