use compositor::{CompositorToken, Handler as CompositorHandler, Rectangle};
use compositor::roles::Role;

use wayland_protocols::unstable::xdg_shell::server::{zxdg_popup_v6, zxdg_positioner_v6 as xdg_positioner,
                                                     zxdg_shell_v6, zxdg_surface_v6, zxdg_toplevel_v6};

use wayland_server::{EventLoopHandle, EventResult, Init, Liveness, Resource};
use wayland_server::protocol::{wl_output, wl_seat, wl_shell, wl_shell_surface, wl_surface};

mod global;
mod wl_handlers;
mod xdg_handlers;

pub struct ShellSurfaceRole {
    pub pending_state: ShellSurfacePendingState,
    pub window_geometry: Option<Rectangle>,
    pub pending_configures: Vec<u32>,
    pub configured: bool,
}

#[derive(Copy, Clone, Debug)]
pub struct PositionerState {
    pub rect_size: (i32, i32),
    pub anchor_rect: Rectangle,
    pub anchor_edges: xdg_positioner::Anchor,
    pub gravity: xdg_positioner::Gravity,
    pub constraint_adjustment: xdg_positioner::ConstraintAdjustment,
    pub offset: (i32, i32),
}

pub enum ShellSurfacePendingState {
    /// This a regular, toplevel surface
    ///
    /// This corresponds to either the `xdg_toplevel` role from the
    /// `xdg_shell` protocol, or the result of `set_toplevel` using the
    /// `wl_shell` protocol.
    Toplevel(ToplevelState),
    /// This is a popup surface
    ///
    /// This corresponds to either the `xdg_popup` role from the
    /// `xdg_shell` protocol, or the result of `set_popup` using the
    /// `wl_shell` protocole
    Popup(PopupState),
    /// This surface was not yet assigned a kind
    None,
}

pub struct ToplevelState {
    pub parent: Option<wl_surface::WlSurface>,
    pub title: String,
    pub app_id: String,
    pub min_size: (i32, i32),
    pub max_size: (i32, i32),
}

impl ToplevelState {
    pub fn clone(&self) -> ToplevelState {
        ToplevelState {
            parent: self.parent.as_ref().and_then(|p| p.clone()),
            title: self.title.clone(),
            app_id: self.app_id.clone(),
            min_size: self.min_size,
            max_size: self.max_size,
        }
    }
}

pub struct PopupState {
    pub parent: wl_surface::WlSurface,
    pub positioner: PositionerState,
}

impl PopupState {
    pub fn clone(&self) -> Option<PopupState> {
        if let Some(p) = self.parent.clone() {
            Some(PopupState {
                parent: p,
                positioner: self.positioner.clone(),
            })
        } else {
            // the parent surface does no exist any longer,
            // this popup does not make any sense now
            None
        }
    }
}

impl Default for ShellSurfacePendingState {
    fn default() -> ShellSurfacePendingState {
        ShellSurfacePendingState::None
    }
}

pub struct ShellHandler<U, R, H, SH, SD> {
    my_id: usize,
    log: ::slog::Logger,
    token: CompositorToken<U, R, H>,
    handler: SH,
    known_toplevels: Vec<ToplevelSurface<U, R, H, SD>>,
    known_popups: Vec<PopupSurface<U, R, H, SD>>,
    _shell_data: ::std::marker::PhantomData<SD>,
}

impl<U, R, H, SH, SD> Init for ShellHandler<U, R, H, SH, SD> {
    fn init(&mut self, _evqh: &mut EventLoopHandle, index: usize) {
        self.my_id = index;
        debug!(self.log, "Init finished")
    }
}

impl<U, R, H, SH, SD> ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
{
    /// Create a new CompositorHandler
    pub fn new<F, L>(handler: SH, token: CompositorToken<U, R, H>, logger: L) -> ShellHandler<U, R, H, SH, SD>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger);
        ShellHandler {
            my_id: ::std::usize::MAX,
            log: log.new(o!("smithay_module" => "shell_handler")),
            token: token,
            handler: handler,
            known_toplevels: Vec::new(),
            known_popups: Vec::new(),
            _shell_data: ::std::marker::PhantomData,
        }
    }

    /// Access the inner handler of this CompositorHandler
    pub fn get_handler(&mut self) -> &mut SH {
        &mut self.handler
    }

    /// Cleans the internal surface storage by removing all dead surfaces
    pub fn cleanup_surfaces(&mut self) {
        self.known_toplevels.retain(|s| s.alive());
        self.known_popups.retain(|s| s.alive());
    }

    /// Access all the shell surfaces known by this handler
    pub fn toplevel_surfaces(&self) -> &[ToplevelSurface<U, R, H, SD>] {
        &self.known_toplevels[..]
    }

    /// Access all the popup surfaces known by this handler
    pub fn popup_surfaces(&self) -> &[PopupSurface<U, R, H, SD>] {
        &self.known_popups[..]
    }
}

/*
 * User interaction
 */

enum ShellClientKind {
    Wl(wl_shell::WlShell),
    Xdg(zxdg_shell_v6::ZxdgShellV6),
}

struct ShellClientData<SD> {
    pending_ping: u32,
    data: SD,
}

pub struct ShellClient<SD> {
    kind: ShellClientKind,
    _data: ::std::marker::PhantomData<*mut SD>,
}

impl<SD> ShellClient<SD> {
    pub fn alive(&self) -> bool {
        match self.kind {
            ShellClientKind::Wl(ref s) => s.status() == Liveness::Alive,
            ShellClientKind::Xdg(ref s) => s.status() == Liveness::Alive,
        }
    }

    pub fn equals(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (&ShellClientKind::Wl(ref s1), &ShellClientKind::Wl(ref s2)) => s1.equals(s2),
            (&ShellClientKind::Xdg(ref s1), &ShellClientKind::Xdg(ref s2)) => s1.equals(s2),
            _ => false,
        }
    }

    /// Send a ping request to this shell client
    ///
    /// You'll receive the reply in the `Handler::cient_pong()` method.
    ///
    /// A typical use is to start a timer at the same time you send this ping
    /// request, and cancel it when you receive the pong. If the timer runs
    /// down to 0 before a pong is received, mark the client as unresponsive.
    ///
    /// Fails if this shell client already has a pending ping or is already dead.
    pub fn send_ping(&self, serial: u32) -> Result<(), ()> {
        if !self.alive() {
            return Err(());
        }
        match self.kind {
            ShellClientKind::Wl(ref shell) => {
                let mutex = unsafe { &*(shell.get_user_data() as *mut self::wl_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.0.pending_ping == 0 {
                    return Err(());
                }
                guard.0.pending_ping = serial;
                if let Some(surface) = guard.1.first() {
                    // there is at least one surface, send the ping
                    // if there is no surface, the ping will remain pending
                    // and will be sent when the client creates a surface
                    surface.ping(serial);
                }
            }
            ShellClientKind::Xdg(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::xdg_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.pending_ping == 0 {
                    return Err(());
                }
                guard.pending_ping = serial;
                shell.ping(serial);
            }
        }
        Ok(())
    }

    pub fn with_data<F, T>(&self, f: F) -> Result<T, ()>
    where
        F: FnOnce(&mut SD) -> T,
    {
        if !self.alive() {
            return Err(());
        }
        match self.kind {
            ShellClientKind::Wl(ref shell) => {
                let mutex = unsafe { &*(shell.get_user_data() as *mut self::wl_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                Ok(f(&mut guard.0.data))
            }
            ShellClientKind::Xdg(ref shell) => {
                let mutex =
                    unsafe { &*(shell.get_user_data() as *mut self::xdg_handlers::ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                Ok(f(&mut guard.data))
            }
        }
    }
}

enum SurfaceKind {
    Wl(wl_shell_surface::WlShellSurface),
    XdgToplevel(zxdg_toplevel_v6::ZxdgToplevelV6),
    XdgPopup(zxdg_popup_v6::ZxdgPopupV6),
}

pub struct ToplevelSurface<U, R, H, SD> {
    wl_surface: wl_surface::WlSurface,
    shell_surface: SurfaceKind,
    token: CompositorToken<U, R, H>,
    _shell_data: ::std::marker::PhantomData<SD>,
}

impl<U, R, H, SD> ToplevelSurface<U, R, H, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
{
    pub fn alive(&self) -> bool {
        let shell_surface_alive = match self.shell_surface {
            SurfaceKind::Wl(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgToplevel(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgPopup(_) => unreachable!(),
        };
        shell_surface_alive && self.wl_surface.status() == Liveness::Alive
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface.equals(&other.wl_surface)
    }

    pub fn client(&self) -> ShellClient<SD> {
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => {
                let &(_, ref shell) =
                    unsafe { &*(s.get_user_data() as *mut self::wl_handlers::ShellSurfaceUserData) };
                ShellClient {
                    kind: ShellClientKind::Wl(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                }
            }
            SurfaceKind::XdgToplevel(ref s) => {
                let &(_, ref shell, _) =
                    unsafe { &*(s.get_user_data() as *mut self::xdg_handlers::ShellSurfaceUserData) };
                ShellClient {
                    kind: ShellClientKind::Xdg(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                }
            }
            SurfaceKind::XdgPopup(_) => unreachable!(),
        }
    }

    pub fn send_configure(&self, cfg: ToplevelConfigure) -> EventResult<()> {
        if !self.alive() {
            return EventResult::Destroyed;
        }
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => self::wl_handlers::send_toplevel_configure(s, cfg),
            SurfaceKind::XdgToplevel(ref s) => {
                self::xdg_handlers::send_toplevel_configure(self.token, s, cfg)
            }
            SurfaceKind::XdgPopup(_) => unreachable!(),
        }
        EventResult::Sent(())
    }

    /// Make sure this surface was configured
    ///
    /// Returns `true` if it was, if not, returns `false` and raise
    /// a protocol error to the associated client. Also returns `false`
    /// if the surface is already destroyed.
    ///
    /// xdg_shell mandates that a client acks a configure before commiting
    /// anything.
    pub fn ensure_configured(&self) -> bool {
        if !self.alive() {
            return false;
        }
        let configured = self.token
            .with_role_data::<ShellSurfaceRole, _, _>(&self.wl_surface, |data| data.configured)
            .expect(
                "A shell surface object exists but the surface does not have the shell_surface role ?!",
            );
        if !configured {
            if let SurfaceKind::XdgToplevel(ref s) = self.shell_surface {
                let ptr = s.get_user_data();
                let &(_, _, ref xdg_surface) =
                    unsafe { &*(ptr as *mut self::xdg_handlers::ShellSurfaceUserData) };
                xdg_surface.post_error(
                    zxdg_surface_v6::Error::NotConstructed as u32,
                    "Surface has not been confgured yet.".into(),
                );
            } else {
                unreachable!();
            }
        }
        configured
    }

    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        if self.alive() {
            Some(&self.wl_surface)
        } else {
            None
        }
    }

    pub fn get_pending_state(&self) -> Option<ToplevelState> {
        if !self.alive() {
            return None;
        }
        self.token
            .with_role_data::<ShellSurfaceRole, _, _>(&self.wl_surface, |data| match data.pending_state {
                ShellSurfacePendingState::Toplevel(ref state) => Some(state.clone()),
                _ => None,
            })
            .ok()
            .and_then(|x| x)
    }
}

pub struct PopupSurface<U, R, H, SD> {
    wl_surface: wl_surface::WlSurface,
    shell_surface: SurfaceKind,
    token: CompositorToken<U, R, H>,
    _shell_data: ::std::marker::PhantomData<SD>,
}

impl<U, R, H, SD> PopupSurface<U, R, H, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
{
    pub fn alive(&self) -> bool {
        let shell_surface_alive = match self.shell_surface {
            SurfaceKind::Wl(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgPopup(ref s) => s.status() == Liveness::Alive,
            SurfaceKind::XdgToplevel(_) => unreachable!(),
        };
        shell_surface_alive && self.wl_surface.status() == Liveness::Alive
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.wl_surface.equals(&other.wl_surface)
    }

    pub fn client(&self) -> ShellClient<SD> {
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => {
                let &(_, ref shell) =
                    unsafe { &*(s.get_user_data() as *mut self::wl_handlers::ShellSurfaceUserData) };
                ShellClient {
                    kind: ShellClientKind::Wl(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                }
            }
            SurfaceKind::XdgPopup(ref s) => {
                let &(_, ref shell, _) =
                    unsafe { &*(s.get_user_data() as *mut self::xdg_handlers::ShellSurfaceUserData) };
                ShellClient {
                    kind: ShellClientKind::Xdg(unsafe { shell.clone_unchecked() }),
                    _data: ::std::marker::PhantomData,
                }
            }
            SurfaceKind::XdgToplevel(_) => unreachable!(),
        }
    }

    pub fn send_configure(&self, cfg: PopupConfigure) -> EventResult<()> {
        if !self.alive() {
            return EventResult::Destroyed;
        }
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => self::wl_handlers::send_popup_configure(s, cfg),
            SurfaceKind::XdgPopup(ref s) => self::xdg_handlers::send_popup_configure(self.token, s, cfg),
            SurfaceKind::XdgToplevel(_) => unreachable!(),
        }
        EventResult::Sent(())
    }

    pub fn send_popup_done(&self) -> EventResult<()> {
        if !self.alive() {
            return EventResult::Destroyed;
        }
        match self.shell_surface {
            SurfaceKind::Wl(ref s) => s.popup_done(),
            SurfaceKind::XdgPopup(ref s) => s.popup_done(),
            SurfaceKind::XdgToplevel(_) => unreachable!(),
        }
    }

    pub fn get_surface(&self) -> Option<&wl_surface::WlSurface> {
        if self.alive() {
            Some(&self.wl_surface)
        } else {
            None
        }
    }

    pub fn get_pending_state(&self) -> Option<PopupState> {
        if !self.alive() {
            return None;
        }
        self.token
            .with_role_data::<ShellSurfaceRole, _, _>(&self.wl_surface, |data| match data.pending_state {
                ShellSurfacePendingState::Popup(ref state) => state.clone(),
                _ => None,
            })
            .ok()
            .and_then(|x| x)
    }
}

pub struct ToplevelConfigure {
    pub size: Option<(i32, i32)>,
    pub states: Vec<zxdg_toplevel_v6::State>,
    pub serial: u32,
}

pub struct PopupConfigure {
    pub position: (i32, i32),
    pub size: (i32, i32),
    pub serial: u32,
}

pub trait Handler<U, R, H, SD> {
    fn new_client(&mut self, evlh: &mut EventLoopHandle, client: ShellClient<SD>);
    fn client_pong(&mut self, evlh: &mut EventLoopHandle, client: ShellClient<SD>);
    fn new_toplevel(&mut self, evlh: &mut EventLoopHandle, surface: ToplevelSurface<U, R, H, SD>)
                    -> ToplevelConfigure;
    fn new_popup(&mut self, evlh: &mut EventLoopHandle, surface: PopupSurface<U, R, H, SD>)
                 -> PopupConfigure;
    fn move_(&mut self, evlh: &mut EventLoopHandle, surface: ToplevelSurface<U, R, H, SD>,
             seat: &wl_seat::WlSeat, serial: u32);
    fn resize(&mut self, evlh: &mut EventLoopHandle, surface: ToplevelSurface<U, R, H, SD>,
              seat: &wl_seat::WlSeat, serial: u32, edges: zxdg_toplevel_v6::ResizeEdge);
    fn grab(&mut self, evlh: &mut EventLoopHandle, surface: PopupSurface<U, R, H, SD>,
            seat: &wl_seat::WlSeat, serial: u32);
    fn change_display_state(&mut self, evlh: &mut EventLoopHandle, surface: ToplevelSurface<U, R, H, SD>,
                            maximized: Option<bool>, minimized: Option<bool>, fullscreen: Option<bool>,
                            output: Option<&wl_output::WlOutput>)
                            -> ToplevelConfigure;
    fn show_window_menu(&mut self, evlh: &mut EventLoopHandle, surface: ToplevelSurface<U, R, H, SD>,
                        seat: &wl_seat::WlSeat, serial: u32, x: i32, y: i32);
}
