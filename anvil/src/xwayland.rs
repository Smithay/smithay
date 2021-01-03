use std::os::unix::net::UnixStream;

use smithay:: {
    reexports::wayland_server::Client,
    xwayland::XWindowManager,
};

use x11rb::{
    connection::Connection as _,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            ChangeWindowAttributesAux, ConnectionExt as _, EventMask, WindowClass,
        },
    },
    rust_connection::{DefaultStream, RustConnection},
};

/// Implementation of [`smithay::xwayland::XWindowManager`] that is used for starting XWayland.
/// After XWayland was started, the actual state is kept in `X11State`.
pub struct XWm;

impl XWm {
    pub fn new() -> Self {
        Self
    }
}

impl XWindowManager for XWm {
    fn xwayland_ready(&mut self, connection: UnixStream, _client: Client) {
        let _wm = X11State::start_wm(connection);
    }

    fn xwayland_exited(&mut self) {}
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_S0,
    }
}

/// The actual runtime state of the XWayland integration.
struct X11State {
}

impl X11State {
    fn start_wm(connection: UnixStream) -> Result<Self, Box<dyn std::error::Error>> {
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

        Ok(X11State {})
    }
}
