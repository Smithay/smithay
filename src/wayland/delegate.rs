use wayland_server::{DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

///  Dispatch delegation helper
pub trait DelegateDispatchBase<I: Resource> {
    ///  Dispatch delegation helper
    type UserData: Send + Sync + 'static;
}

///  Dispatch delegation helper
pub trait DelegateDispatch<
    I: Resource,
    D: Dispatch<I, UserData = <Self as DelegateDispatchBase<I>>::UserData>,
>: Sized + DelegateDispatchBase<I>
{
    ///  Dispatch delegation helper
    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &I,
        request: I::Request,
        data: &Self::UserData,
        dhandle: &mut DisplayHandle<'_, D>,
        init: &mut DataInit<'_, D>,
    );
}

///  Dispatch delegation helper
pub trait DelegateGlobalDispatchBase<I: Resource> {
    ///  Dispatch delegation helper
    type GlobalData: Send + Sync + 'static;
}

///  Dispatch delegation helper
pub trait DelegateGlobalDispatch<
    I: Resource,
    D: GlobalDispatch<I, GlobalData = <Self as DelegateGlobalDispatchBase<I>>::GlobalData>
        + Dispatch<I, UserData = <Self as DelegateDispatchBase<I>>::UserData>,
>: Sized + DelegateGlobalDispatchBase<I> + DelegateDispatch<I, D>
{
    ///  Dispatch delegation helper
    fn bind(
        &mut self,
        handle: &mut DisplayHandle<'_, D>,
        client: &wayland_server::Client,
        resource: New<I>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    );

    ///  Dispatch delegation helper
    fn can_view(_client: wayland_server::Client, _global_data: &Self::GlobalData) -> bool {
        true
    }
}
