use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use wayland_protocols::staging::xdg_activation::v1::server::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_server::{
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    DispatchData, Filter, Main,
};

use crate::wayland::Serial;

use super::{XdgActivationEvent, XdgActivationState, XdgActivationToken, XdgActivationTokenData};

type Impl = dyn FnMut(&Mutex<XdgActivationState>, XdgActivationEvent, DispatchData<'_>);

/// New xdg activation global
pub(super) fn implement_activation_global(
    global: Main<xdg_activation_v1::XdgActivationV1>,
    state: Arc<Mutex<XdgActivationState>>,
    implementation: Rc<RefCell<Impl>>,
) {
    global.quick_assign(move |_, req, ddata| match req {
        xdg_activation_v1::Request::GetActivationToken { id } => {
            get_activation_token(id, state.clone(), implementation.clone());
        }
        xdg_activation_v1::Request::Activate { token, surface } => {
            activate(
                token.into(),
                surface,
                state.as_ref(),
                implementation.as_ref(),
                ddata,
            );
        }
        _ => {}
    });
}

/// New xdg activation token
fn get_activation_token(
    id: Main<xdg_activation_token_v1::XdgActivationTokenV1>,
    state: Arc<Mutex<XdgActivationState>>,
    implementation: Rc<RefCell<Impl>>,
) {
    id.quick_assign({
        let state = state.clone();

        let mut token_serial: Option<(Serial, WlSeat)> = None;
        let mut token_app_id: Option<String> = None;
        let mut token_surface: Option<WlSurface> = None;
        let mut token_constructed = false;

        move |id, req, _| {
            if !token_constructed {
                match req {
                    xdg_activation_token_v1::Request::SetSerial { serial, seat } => {
                        token_serial = Some((serial.into(), seat));
                    }
                    xdg_activation_token_v1::Request::SetAppId { app_id } => {
                        token_app_id = Some(app_id);
                    }
                    xdg_activation_token_v1::Request::SetSurface { surface } => {
                        token_surface = Some(surface);
                    }
                    xdg_activation_token_v1::Request::Commit => {
                        let (token, token_data) = XdgActivationTokenData::new(
                            token_serial.take(),
                            token_app_id.take(),
                            token_surface.take(),
                        );

                        state
                            .lock()
                            .unwrap()
                            .pending_tokens
                            .insert(token.clone(), token_data);
                        id.as_ref().user_data().set_threadsafe(|| token.clone());

                        id.done(token.to_string());

                        token_constructed = true;
                    }
                    _ => {}
                };
            } else {
                id.as_ref().post_error(
                    xdg_activation_token_v1::Error::AlreadyUsed as u32,
                    "The activation token has already been constructed".into(),
                )
            }
        }
    });

    id.assign_destructor(Filter::new(
        move |token: xdg_activation_token_v1::XdgActivationTokenV1, _, ddata| {
            if let Some(token) = token.as_ref().user_data().get::<XdgActivationToken>() {
                state.lock().unwrap().pending_tokens.remove(token);

                if let Some((token_data, surface)) = state.lock().unwrap().activation_requests.remove(token) {
                    let mut cb = implementation.borrow_mut();
                    cb(
                        &state,
                        XdgActivationEvent::DestroyActivationRequest {
                            token: token.clone(),
                            token_data,
                            surface,
                        },
                        ddata,
                    );
                }
            }
        },
    ));
}

/// Xdg activation request
fn activate(
    token: XdgActivationToken,
    surface: WlSurface,
    state: &Mutex<XdgActivationState>,
    implementation: &RefCell<Impl>,
    ddata: DispatchData<'_>,
) {
    let mut guard = state.lock().unwrap();
    if let Some(token_data) = guard.pending_tokens.remove(&token) {
        guard
            .activation_requests
            .insert(token.clone(), (token_data.clone(), surface.clone()));

        // The user may want to use state, so we need to unlock it
        drop(guard);

        let mut cb = implementation.borrow_mut();
        cb(
            state,
            XdgActivationEvent::RequestActivation {
                token: token.clone(),
                token_data,
                surface,
            },
            ddata,
        );
    }
}
