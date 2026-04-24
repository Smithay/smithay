use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use wayland_protocols::xdg::activation::v1::server::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, New, Resource};

use super::{ActivationTokenData, TokenBuilder, XdgActivationHandler, XdgActivationTokenData};

use crate::wayland::{Dispatch2, GlobalData, GlobalDispatch2};

impl<D> Dispatch2<xdg_activation_v1::XdgActivationV1, D> for GlobalData
where
    D: Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ActivationTokenData>
        + XdgActivationHandler
        + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _: &Client,
        _: &xdg_activation_v1::XdgActivationV1,
        request: xdg_activation_v1::Request,
        _dh: &DisplayHandle,
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

                if let Some(token_data) = activation_state.known_tokens.get(&token).cloned() {
                    state.request_activation(token, token_data, surface);
                }
            }

            _ => unreachable!(),
        }
    }
}

impl<D> GlobalDispatch2<xdg_activation_v1::XdgActivationV1, D> for GlobalData
where
    D: Dispatch<xdg_activation_v1::XdgActivationV1, GlobalData>
        + Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ActivationTokenData>
        + XdgActivationHandler
        + 'static,
{
    fn bind(
        &self,
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<xdg_activation_v1::XdgActivationV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D> Dispatch2<xdg_activation_token_v1::XdgActivationTokenV1, D> for ActivationTokenData
where
    D: XdgActivationHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        token: &xdg_activation_token_v1::XdgActivationTokenV1,
        request: xdg_activation_token_v1::Request,
        _dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_activation_token_v1::Request::SetSerial { serial, seat } => {
                if self.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                self.build.lock().unwrap().serial = Some((serial.into(), seat));
            }

            xdg_activation_token_v1::Request::SetAppId { app_id } => {
                if self.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                self.build.lock().unwrap().app_id = Some(app_id);
            }

            xdg_activation_token_v1::Request::SetSurface { surface } => {
                if self.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                self.build.lock().unwrap().surface = Some(surface);
            }

            xdg_activation_token_v1::Request::Commit => {
                if self.constructed.load(Ordering::Relaxed) {
                    token.post_error(
                        xdg_activation_token_v1::Error::AlreadyUsed,
                        "The activation token has already been constructed",
                    );
                    return;
                }

                self.constructed.store(true, Ordering::Relaxed);

                let (activation_token, token_data) = {
                    let mut guard = self.build.lock().unwrap();

                    XdgActivationTokenData::new(
                        Some(client.id()),
                        guard.serial.take(),
                        guard.app_id.take(),
                        guard.surface.take(),
                    )
                };

                let valid = state.token_created(activation_token.clone(), token_data.clone());

                *self.token.lock().unwrap() = Some(activation_token.clone());
                if valid {
                    state
                        .activation_state()
                        .known_tokens
                        .insert(activation_token.clone(), token_data);
                }
                token.done(activation_token.to_string());
            }

            xdg_activation_token_v1::Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}
