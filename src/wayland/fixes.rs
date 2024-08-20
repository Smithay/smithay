use wayland_server::{
    backend::GlobalId, protocol::wl_fixes, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource,
};

#[derive(Debug, Clone)]
pub struct FixesState {
    global: GlobalId,
}

impl FixesState {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<wl_fixes::WlFixes, ()>,
        D: Dispatch<wl_fixes::WlFixes, ()>,
        D: 'static,
    {
        let global = display.create_global::<D, wl_fixes::WlFixes, _>(1, ());
        Self { global }
    }

    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<wl_fixes::WlFixes, (), D> for FixesState
where
    D: GlobalDispatch<wl_fixes::WlFixes, ()>,
    D: Dispatch<wl_fixes::WlFixes, ()>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<wl_fixes::WlFixes>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<wl_fixes::WlFixes, (), D> for FixesState
where
    D: Dispatch<wl_fixes::WlFixes, ()>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &wl_fixes::WlFixes,
        request: wl_fixes::Request,
        _data: &(),
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_fixes::Request::DestroyRegistry { registry } => {
                dh.backend_handle().destroy_object::<D>(&registry.id());
            }
            wl_fixes::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

#[macro_export]
macro_rules! delegate_fixes {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_fixes::WlFixes: ()
        ] => $crate::wayland::fixes::FixesState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_fixes::WlFixes: ()
        ] => $crate::wayland::fixes::FixesState);
    };
}
