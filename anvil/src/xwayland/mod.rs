use std::{cell::RefCell, collections::HashMap, convert::TryFrom, os::unix::net::UnixStream, rc::Rc};

use smithay::{
    reexports::{
        calloop::LoopHandle,
        wayland_server::{protocol::wl_surface::WlSurface, Client},
    },
    wayland::compositor::CompositorToken,
    xwayland::XWindowManager,
};

use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            ChangeWindowAttributesAux, ConfigWindow, ConfigureWindowAux, ConnectionExt as _, EventMask,
            Window, WindowClass,
        },
        Event,
    },
    rust_connection::{DefaultStream, RustConnection},
};

use crate::{
    shell::{MyWindowMap, Roles},
    window_map::Kind,
    AnvilState,
};

use x11rb_event_source::X11Source;

mod x11rb_event_source;

/// Implementation of [`smithay::xwayland::XWindowManager`] that is used for starting XWayland.
/// After XWayland was started, the actual state is kept in `X11State`.
pub struct XWm {
    handle: LoopHandle<AnvilState>,
    token: CompositorToken<Roles>,
    window_map: Rc<RefCell<MyWindowMap>>,
    log: slog::Logger,
}

impl XWm {
    pub fn new(
        handle: LoopHandle<AnvilState>,
        token: CompositorToken<Roles>,
        window_map: Rc<RefCell<MyWindowMap>>,
        log: slog::Logger,
    ) -> Self {
        Self {
            handle,
            token,
            window_map,
            log,
        }
    }
}

impl XWindowManager for XWm {
    fn xwayland_ready(&mut self, connection: UnixStream, client: Client) {
        let (wm, source) =
            X11State::start_wm(connection, self.token, self.window_map.clone(), self.log.clone()).unwrap();
        let wm = Rc::new(RefCell::new(wm));
        client.data_map().insert_if_missing(|| Rc::clone(&wm));
        self.handle
            .insert_source(source, move |events, _, _| {
                let mut wm = wm.borrow_mut();
                for event in events.into_iter() {
                    wm.handle_event(event, &client)?;
                }
                Ok(())
            })
            .unwrap();
    }

    fn xwayland_exited(&mut self) {}
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_S0,
        WL_SURFACE_ID,
    }
}

/// The actual runtime state of the XWayland integration.
struct X11State {
    conn: Rc<RustConnection>,
    atoms: Atoms,
    log: slog::Logger,
    unpaired_surfaces: HashMap<u32, Window>,
    token: CompositorToken<Roles>,
    window_map: Rc<RefCell<MyWindowMap>>,
}

impl X11State {
    fn start_wm(
        connection: UnixStream,
        token: CompositorToken<Roles>,
        window_map: Rc<RefCell<MyWindowMap>>,
        log: slog::Logger,
    ) -> Result<(Self, X11Source), Box<dyn std::error::Error>> {
        // Create an X11 connection. XWayland only uses screen 0.
        let screen = 0;
        let stream = DefaultStream::from_unix_stream(connection)?;
        let conn = RustConnection::connect_to_stream(stream, screen)?;
        let atoms = Atoms::new(&conn)?.reply()?;

        let screen = &conn.setup().roots[0];

        // Actually become the WM by redirecting some operations
        conn.change_window_attributes(
            screen.root,
            &ChangeWindowAttributesAux::default().event_mask(EventMask::SubstructureRedirect),
        )?;

        // Tell XWayland that we are the WM by acquiring the WM_S0 selection. No X11 clients are accepted before this.
        let win = conn.generate_id()?;
        conn.create_window(
            screen.root_depth,
            win,
            screen.root,
            // x, y, width, height, border width
            0,
            0,
            1,
            1,
            0,
            WindowClass::InputOutput,
            x11rb::COPY_FROM_PARENT,
            &Default::default(),
        )?;
        conn.set_selection_owner(win, atoms.WM_S0, x11rb::CURRENT_TIME)?;

        // XWayland wants us to do this to function properly...?
        conn.composite_redirect_subwindows(screen.root, Redirect::Manual)?;

        conn.flush()?;

        let conn = Rc::new(conn);
        let wm = Self {
            conn: Rc::clone(&conn),
            atoms,
            unpaired_surfaces: Default::default(),
            token,
            window_map,
            log,
        };

        Ok((wm, X11Source::new(conn)))
    }

    fn handle_event(&mut self, event: Event, client: &Client) -> Result<(), ReplyOrIdError> {
        debug!(self.log, "X11: Got event {:?}", event);
        match event {
            Event::ConfigureRequest(r) => {
                // Just grant the wish
                let mut aux = ConfigureWindowAux::default();
                if r.value_mask & u16::from(ConfigWindow::StackMode) != 0 {
                    aux = aux.stack_mode(r.stack_mode);
                }
                if r.value_mask & u16::from(ConfigWindow::Sibling) != 0 {
                    aux = aux.sibling(r.sibling);
                }
                if r.value_mask & u16::from(ConfigWindow::X) != 0 {
                    aux = aux.x(i32::try_from(r.x).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::Y) != 0 {
                    aux = aux.y(i32::try_from(r.y).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::Width) != 0 {
                    aux = aux.width(u32::try_from(r.width).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::Height) != 0 {
                    aux = aux.height(u32::try_from(r.height).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::BorderWidth) != 0 {
                    aux = aux.border_width(u32::try_from(r.border_width).unwrap());
                }
                self.conn.configure_window(r.window, &aux)?;
            }
            Event::MapRequest(r) => {
                // Just grant the wish
                self.conn.map_window(r.window)?;
            }
            Event::ClientMessage(msg) => {
                if msg.type_ == self.atoms.WL_SURFACE_ID {
                    // We get a WL_SURFACE_ID message when Xwayland creates a WlSurface for a
                    // window. Both the creation of the surface and this client message happen at
                    // roughly the same time and are sent over different sockets (X11 socket and
                    // wayland socket). Thus, we could receive these two in any order. Hence, it
                    // can happen that we get None below when X11 was faster than Wayland.

                    let id = msg.data.as_data32()[0];
                    let surface = client.get_resource::<WlSurface>(id);
                    info!(
                        self.log,
                        "X11 surface {:x?} corresponds to WlSurface {:x} = {:?}", msg.window, id, surface,
                    );
                    match surface {
                        None => {
                            self.unpaired_surfaces.insert(id, msg.window);
                        }
                        Some(surface) => self.new_window(msg.window, surface),
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn new_window(&mut self, window: Window, surface: WlSurface) {
        debug!(self.log, "Matched X11 surface {:x?} to {:x?}", window, surface);

        if self.token.give_role_with(&surface, X11SurfaceRole).is_err() {
            // It makes no sense to post a protocol error here since that would only kill Xwayland
            error!(self.log, "Surface {:x?} already has a role?!", surface);
            return;
        }

        let x11surface = X11Surface { surface };
        self.window_map
            .borrow_mut()
            .insert(Kind::X11(x11surface), (0, 0));
    }
}

// Called when a WlSurface commits.
pub fn commit_hook(surface: &WlSurface) {
    // Is this the Xwayland client?
    if let Some(client) = surface.as_ref().client() {
        if let Some(x11) = client.data_map().get::<Rc<RefCell<X11State>>>() {
            let mut inner = x11.borrow_mut();
            // Is the surface among the unpaired surfaces (see comment next to WL_SURFACE_ID
            // handling above)
            if let Some(window) = inner.unpaired_surfaces.remove(&surface.as_ref().id()) {
                inner.new_window(window, surface.clone());
            }
        }
    }
}

pub struct X11SurfaceRole;

#[derive(Clone)]
pub struct X11Surface {
    surface: WlSurface,
}

impl X11Surface {
    pub fn alive(&self) -> bool {
        self.surface.as_ref().is_alive()
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.alive() && other.alive() && self.surface.as_ref().equals(&other.surface.as_ref())
    }

    pub fn get_surface(&self) -> Option<&WlSurface> {
        if self.alive() {
            Some(&self.surface)
        } else {
            None
        }
    }
}
