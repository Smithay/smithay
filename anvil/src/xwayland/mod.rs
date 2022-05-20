use std::{collections::HashMap, convert::TryFrom, os::unix::net::UnixStream, sync::Arc};

use crate::AnvilState;
use smithay::{
    desktop::{Kind, Space, Window, X11Surface},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Client, Display, DisplayHandle, Resource},
    utils::{x11rb::X11Source, Logical, Point},
    wayland::compositor::give_role,
};
use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            ChangeWindowAttributesAux, ConfigWindow, ConfigureWindowAux, ConnectionExt as _, EventMask,
            Window as X11Window, WindowClass,
        },
        Event,
    },
    rust_connection::{DefaultStream, RustConnection},
};

impl<BackendData: 'static> AnvilState<BackendData> {
    pub fn start_xwayland(&mut self, display: &mut Display<AnvilState<BackendData>>) {
        if let Err(e) = self.xwayland.start(self.handle.clone(), display) {
            error!(self.log, "Failed to start XWayland: {}", e);
        }
    }

    pub fn xwayland_ready(&mut self, connection: UnixStream, client: Client) {
        let (wm, source) = X11State::start_wm(connection, client, self.log.clone()).unwrap();
        self.x11_state = Some(wm);
        let log = self.log.clone();
        self.handle
            .insert_source(source, move |event, _, data| {
                if let Some(x11) = data.state.x11_state.as_mut() {
                    match x11.handle_event(event, &mut data.display.handle(), &mut data.state.space) {
                        Ok(()) => {}
                        Err(err) => error!(log, "Error while handling X11 event: {}", err),
                    }
                }
            })
            .unwrap();
    }

    pub fn xwayland_exited(&mut self) {
        let _ = self.x11_state.take();
        error!(self.log, "Xwayland crashed");
    }
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_S0,
        WL_SURFACE_ID,
        _ANVIL_CLOSE_CONNECTION,
    }
}

/// The actual runtime state of the XWayland integration.
#[derive(Debug)]
pub struct X11State {
    conn: Arc<RustConnection>,
    atoms: Atoms,
    client: Client,
    log: slog::Logger,
    unpaired_surfaces: HashMap<u32, (X11Window, Point<i32, Logical>)>,
}

impl X11State {
    fn start_wm(
        connection: UnixStream,
        client: Client,
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
            &ChangeWindowAttributesAux::default().event_mask(EventMask::SUBSTRUCTURE_REDIRECT),
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
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &Default::default(),
        )?;
        conn.set_selection_owner(win, atoms.WM_S0, x11rb::CURRENT_TIME)?;

        // XWayland wants us to do this to function properly...?
        conn.composite_redirect_subwindows(screen.root, Redirect::MANUAL)?;

        conn.flush()?;

        let conn = Arc::new(conn);
        let wm = Self {
            conn: Arc::clone(&conn),
            atoms,
            client,
            unpaired_surfaces: Default::default(),
            log: log.clone(),
        };

        Ok((wm, X11Source::new(conn, win, atoms._ANVIL_CLOSE_CONNECTION, log)))
    }

    fn handle_event(
        &mut self,
        event: Event,
        dh: &mut DisplayHandle<'_>,
        space: &mut Space,
    ) -> Result<(), ReplyOrIdError> {
        debug!(self.log, "X11: Got event {:?}", event);
        match event {
            Event::ConfigureRequest(r) => {
                // Just grant the wish
                let mut aux = ConfigureWindowAux::default();
                if r.value_mask & u16::from(ConfigWindow::STACK_MODE) != 0 {
                    aux = aux.stack_mode(r.stack_mode);
                }
                if r.value_mask & u16::from(ConfigWindow::SIBLING) != 0 {
                    aux = aux.sibling(r.sibling);
                }
                if r.value_mask & u16::from(ConfigWindow::X) != 0 {
                    aux = aux.x(i32::try_from(r.x).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::Y) != 0 {
                    aux = aux.y(i32::try_from(r.y).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::WIDTH) != 0 {
                    aux = aux.width(u32::try_from(r.width).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::HEIGHT) != 0 {
                    aux = aux.height(u32::try_from(r.height).unwrap());
                }
                if r.value_mask & u16::from(ConfigWindow::BORDER_WIDTH) != 0 {
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

                    let location = {
                        match self.conn.get_geometry(msg.window)?.reply() {
                            Ok(geo) => (geo.x as i32, geo.y as i32).into(),
                            Err(err) => {
                                error!(
                                    self.log,
                                    "Failed to get geometry for {:x}, perhaps the window was already destroyed?",
                                    msg.window;
                                    "err" => format!("{:?}", err),
                                );
                                (0, 0).into()
                            }
                        }
                    };

                    let id = msg.data.as_data32()[0];
                    let surface = self.client.object_from_protocol_id(dh, id);

                    match surface {
                        Err(_) => {
                            self.unpaired_surfaces.insert(id, (msg.window, location));
                        }
                        Ok(surface) => {
                            info!(
                                self.log,
                                "X11 surface {:x?} corresponds to WlSurface {:x} = {:?}",
                                msg.window,
                                id,
                                surface,
                            );
                            self.new_window(msg.window, surface, location, space);
                        }
                    }
                }
            }
            _ => {}
        }
        self.conn.flush()?;
        Ok(())
    }

    fn new_window(
        &mut self,
        window: X11Window,
        surface: WlSurface,
        location: Point<i32, Logical>,
        space: &mut Space,
    ) {
        debug!(self.log, "Matched X11 surface {:x?} to {:x?}", window, surface);

        if give_role(&surface, "x11_surface").is_err() {
            // It makes no sense to post a protocol error here since that would only kill Xwayland
            error!(self.log, "Surface {:x?} already has a role?!", surface);
            return;
        }

        let x11surface = X11Surface { surface };
        space.map_window(&Window::new(Kind::X11(x11surface)), location, true);
    }
}

// Called when a WlSurface commits.
pub fn commit_hook(surface: &WlSurface, dh: &mut DisplayHandle<'_>, state: &mut X11State, space: &mut Space) {
    if let Ok(client) = dh.get_client(surface.id()) {
        // Is this the Xwayland client?
        if client == state.client {
            // Is the surface among the unpaired surfaces (see comment next to WL_SURFACE_ID
            // handling above)
            if let Some((window, location)) = state.unpaired_surfaces.remove(&surface.id().protocol_id()) {
                state.new_window(window, surface.clone(), location, space);
            }
        }
    }
}
