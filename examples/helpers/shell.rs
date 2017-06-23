

use smithay::compositor::{CompositorToken, Handler as CompositorHandler};
use wayland_server::{Client, EventLoopHandle, GlobalHandler, Init, Resource};
use wayland_server::protocol::{wl_shell, wl_shell_surface, wl_surface};

/// A very basic handler for wl_shell
///
/// All it does is track which wl_shell_surface exist and which do not,
/// as well as the roles associated to them.
///
/// That's it.
pub struct WlShellStubHandler<U, H> {
    my_id: Option<usize>,
    token: CompositorToken<U, H>,
    surfaces: Vec<(wl_shell_surface::WlShellSurface, wl_surface::WlSurface)>,
}

impl<U, H> WlShellStubHandler<U, H> {
    pub fn new(compositor_token: CompositorToken<U, H>) -> WlShellStubHandler<U, H> {
        WlShellStubHandler {
            my_id: None,
            token: compositor_token,
            surfaces: Vec::new(),
        }
    }

    pub fn surfaces(&self) -> &[(wl_shell_surface::WlShellSurface, wl_surface::WlSurface)] {
        &self.surfaces
    }
}

impl<U, H> Init for WlShellStubHandler<U, H> {
    fn init(&mut self, evqh: &mut EventLoopHandle, index: usize) {
        self.my_id = Some(index)
    }
}


impl<U, H> GlobalHandler<wl_shell::WlShell> for WlShellStubHandler<U, H>
    where U: Send + 'static,
          H: CompositorHandler<U> + Send + 'static
{
    fn bind(&mut self, evqh: &mut EventLoopHandle, client: &Client, global: wl_shell::WlShell) {
        evqh.register::<_, Self>(&global,
                                 self.my_id
                                     .expect("WlShellStubHandler was not properly initialized."));
    }
}

impl<U, H> wl_shell::Handler for WlShellStubHandler<U, H>
where
    U: Send + 'static,
    H: CompositorHandler<U> + Send + 'static,
{
    fn get_shell_surface(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                         resource: &wl_shell::WlShell, id: wl_shell_surface::WlShellSurface,
                         surface: &wl_surface::WlSurface) {
        let surface = surface.clone().expect(
            "WlShellStubHandler can only manage surfaces managed by Smithay's CompositorHandler.",
        );
        if self.token.give_role(&surface).is_err() {
            // This surface already has a role, and thus cannot be given one!
            resource.post_error(
                wl_shell::Error::Role as u32,
                "Surface already has a role.".into(),
            );
            return;
        }
        evqh.register::<_, Self>(&id, self.my_id.unwrap());
        self.surfaces.push((id, surface))
    }
}

server_declare_handler!(WlShellStubHandler<U: [Send], H: [CompositorHandler<U>, Send]>, wl_shell::Handler, wl_shell::WlShell);

impl<U, H> wl_shell_surface::Handler for WlShellStubHandler<U, H>
where
    U: Send + 'static,
    H: CompositorHandler<U> + Send + 'static,
{
}

server_declare_handler!(WlShellStubHandler<U: [Send], H: [CompositorHandler<U>, Send]>, wl_shell_surface::Handler, wl_shell_surface::WlShellSurface);
