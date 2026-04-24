use wayland_server::{Client, DataInit, DisplayHandle, New, Resource, backend::ClientId};

/// A simplified version of [`wayland_server::Dispatch`]
///
/// A future version of `wayland-server` will replace `Dispatch` with this.
pub trait Dispatch2<I: Resource, State> {
    /// Called when a request from a client is processed.
    fn request(
        &self,
        state: &mut State,
        client: &Client,
        resource: &I,
        request: I::Request,
        dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    );

    /// Called when the object this user data is associated with has been destroyed.
    fn destroyed(&self, _state: &mut State, _client: ClientId, _resource: &I) {}
}

/// A simplified version of [`wayland_server::GlobalDispatch`]
///
/// A future version of `wayland-server` will replace `GlobalDispatch` with this.
pub trait GlobalDispatch2<I: Resource, State> {
    /// Called when a client has bound this global.
    fn bind(
        &self,
        state: &mut State,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<I>,
        data_init: &mut DataInit<'_, State>,
    );

    /// Checks if the global should be advertised to some client.
    fn can_view(&self, _client: &Client) -> bool {
        true
    }
}

/// Implement `Dispatch` and `GlobalDispatch` for every implementation of [`Dispatch2`] and
/// [`GlobalDispatch2`].
#[macro_export]
macro_rules! delegate_dispatch2 {
    ($(@< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)? $ty:ty) => {
        impl<$( $( $lt $( : $clt $(+ $dlt )* )? ),+, )? I, UserData> $crate::reexports::wayland_server::Dispatch<I, UserData> for $ty
            where
                I: $crate::reexports::wayland_server::Resource,
                UserData: $crate::wayland::Dispatch2<I, $ty> {
            fn request(
                state: &mut Self,
                client: &$crate::reexports::wayland_server::Client,
                resource: &I,
                request: <I as $crate::reexports::wayland_server::Resource>::Request,
                data: &UserData,
                dhandle: &$crate::reexports::wayland_server::DisplayHandle,
                data_init: &mut $crate::reexports::wayland_server::DataInit<'_, Self>,
            ) {
                data.request(state, client, resource, request, dhandle, data_init);
            }

            fn destroyed(state: &mut Self, client: $crate::reexports::wayland_server::backend::ClientId, resource: &I, data: &UserData) {
                data.destroyed(state, client, resource);
            }
        }

        impl<$( $( $lt $( : $clt $(+ $dlt )* )? ),+, )? I, UserData> $crate::reexports::wayland_server::GlobalDispatch<I, UserData> for $ty
            where
                I: $crate::reexports::wayland_server::Resource,
                UserData: $crate::wayland::GlobalDispatch2<I, $ty> {
            fn bind(
                state: &mut Self,
                dhandle: &$crate::reexports::wayland_server::DisplayHandle,
                client: &$crate::reexports::wayland_server::Client,
                resource: $crate::reexports::wayland_server::New<I>,
                data: &UserData,
                data_init: &mut $crate::reexports::wayland_server::DataInit<'_, Self>,
            ) {
                data.bind(state, dhandle, client, resource, data_init);
            }

            fn can_view(
                client: $crate::reexports::wayland_server::Client,
                data: &UserData
            ) -> bool {
                data.can_view(&client)
            }
        }
    };
}
