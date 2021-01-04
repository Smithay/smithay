use std::{convert::TryFrom, os::unix::net::UnixStream, rc::Rc};

use smithay::{
    reexports::{
        calloop::LoopHandle,
        wayland_server::Client,
    },
    xwayland::XWindowManager,
};

use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            ChangeWindowAttributesAux, ConfigWindow, ConfigureWindowAux, ConnectionExt as _, EventMask,
            WindowClass,
        },
        Event,
    },
    rust_connection::{DefaultStream, RustConnection},
};

use crate::AnvilState;

use x11rb_event_source::X11Source;

mod x11rb_event_source;

/// Implementation of [`smithay::xwayland::XWindowManager`] that is used for starting XWayland.
/// After XWayland was started, the actual state is kept in `X11State`.
pub struct XWm {
    handle: LoopHandle<AnvilState>,
    log: slog::Logger,
}

impl XWm {
    pub fn new(handle: LoopHandle<AnvilState>, log: slog::Logger) -> Self {
        Self { handle, log }
    }
}

impl XWindowManager for XWm {
    fn xwayland_ready(&mut self, connection: UnixStream, _client: Client) {
        let (mut wm, source) = X11State::start_wm(connection, self.log.clone()).unwrap();
        self.handle
            .insert_source(source, move |events, _, _| {
                for event in events.into_iter() {
                    wm.handle_event(event)?;
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
}

impl X11State {
    fn start_wm(connection: UnixStream, log: slog::Logger) -> Result<(Self, X11Source), Box<dyn std::error::Error>> {
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
            log,
        };

        Ok((wm, X11Source::new(conn)))
    }

    fn handle_event(&mut self, event: Event) -> Result<(), ReplyOrIdError> {
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
                    let id = msg.data.as_data32()[0];
                    info!(
                        self.log,
                        "X11 surface {:x?} corresponds to WlSurface {:x}", msg.window, id,
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }
}
