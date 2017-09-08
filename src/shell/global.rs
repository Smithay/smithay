use super::{Handler as UserHandler, ShellClientData, ShellHandler, ShellSurfaceRole};
use super::wl_handlers::WlShellDestructor;
use super::xdg_handlers::XdgShellDestructor;

use compositor::Handler as CompositorHandler;
use compositor::roles::*;

use std::sync::Mutex;

use wayland_protocols::unstable::xdg_shell::server::zxdg_shell_v6;
use wayland_server::{Client, EventLoopHandle, GlobalHandler, Resource};
use wayland_server::protocol::{wl_shell, wl_shell_surface};

fn shell_client_data<SD: Default>() -> ShellClientData<SD> {
    ShellClientData {
        pending_ping: 0,
        data: Default::default(),
    }
}

impl<U, R, H, SH, SD> GlobalHandler<wl_shell::WlShell> for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Default + Send + 'static,
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: wl_shell::WlShell) {
        debug!(self.log, "New wl_shell global binded.");
        global.set_user_data(Box::into_raw(Box::new(Mutex::new((
            shell_client_data::<SD>(),
            Vec::<wl_shell_surface::WlShellSurface>::new(),
        )))) as *mut _);
        evlh.register_with_destructor::<_, Self, WlShellDestructor<SD>>(&global, self.my_id);
    }
}

impl<U, R, H, SH, SD> GlobalHandler<zxdg_shell_v6::ZxdgShellV6> for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Default + Send + 'static,
{
    fn bind(&mut self, evlh: &mut EventLoopHandle, _: &Client, global: zxdg_shell_v6::ZxdgShellV6) {
        debug!(self.log, "New xdg_shell global binded.");
        global.set_user_data(
            Box::into_raw(Box::new(Mutex::new(shell_client_data::<SD>()))) as *mut _,
        );
        evlh.register_with_destructor::<_, Self, XdgShellDestructor<SD>>(&global, self.my_id);
    }
}
