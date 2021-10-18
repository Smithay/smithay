/**
A note for future contributors and maintainers:

When editing this file, grab the nearest copy of the ICCCM. Following the ICCCM is paramount to
X11 clients behaving properly and preventing scenarios such as windows not being resized in tiling
window managers.

Pay particular attention to "Section 4: Client to Window Manager Communication"

A link to the ICCCM Section 4: https://tronche.com/gui/x/icccm/sec-4.html
*/
use crate::utils::{Logical, Size};

use super::{extension::Extensions, Atoms, Window, X11Error};
use drm_fourcc::DrmFourcc;
use std::sync::{
    atomic::{AtomicU32, AtomicU64},
    Arc, Mutex, Weak,
};
use x11rb::{
    connection::Connection,
    protocol::{
        present::{self, ConnectionExt as _},
        xfixes::ConnectionExt as _,
        xproto::{
            self as x11, AtomEnum, ConnectionExt, CreateWindowAux, Depth, EventMask, PropMode, Screen,
            UnmapNotifyEvent, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

impl From<Arc<WindowInner>> for Window {
    fn from(inner: Arc<WindowInner>) -> Self {
        Window(Arc::downgrade(&inner))
    }
}

#[derive(Debug)]
pub struct CursorState {
    pub inside_window: bool,
    pub visible: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        CursorState {
            inside_window: false,
            visible: true,
        }
    }
}

#[derive(Debug)]
pub(crate) struct WindowInner {
    pub connection: Weak<RustConnection>,
    pub id: x11::Window,
    root: x11::Window,
    present_event_id: u32,
    pub atoms: Atoms,
    pub cursor_state: Arc<Mutex<CursorState>>,
    pub size: Mutex<Size<u16, Logical>>,
    pub next_serial: AtomicU32,
    pub last_msc: Arc<AtomicU64>,
    pub format: DrmFourcc,
    pub depth: Depth,
    pub extensions: Extensions,
}

impl WindowInner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection: Weak<RustConnection>,
        screen: &Screen,
        size: Size<u16, Logical>,
        title: &str,
        format: DrmFourcc,
        atoms: Atoms,
        depth: Depth,
        visual_id: u32,
        colormap: u32,
        extensions: Extensions,
    ) -> Result<WindowInner, X11Error> {
        let weak = connection;
        let connection = weak.upgrade().unwrap();

        // Generate the xid for the window
        let window = connection.generate_id()?;

        // The event mask never include `EventMask::RESIZE_REDIRECT`.
        //
        // The reason is twofold:
        // - We are not a window manager
        // - Makes our window impossible to resize.
        //
        // On the resizing aspect, KWin and some other WMs would allow resizing, but those
        // compositors rely on putting this window in another window for drawing decorations,
        // so visibly in KWin it would look like using the RESIZE_REDIRECT event mask would work,
        // but a tiling window manager would be sad and the tiling window manager devs mad because
        // this window would refuse to listen to the tiling WM.
        //
        // For resizing we use ConfigureNotify events from the STRUCTURE_NOTIFY event mask.

        let window_aux = CreateWindowAux::new()
            .event_mask(
                EventMask::EXPOSURE // Be told when the window is exposed
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::KEY_PRESS // Key press and release
            | EventMask::KEY_RELEASE
            | EventMask::BUTTON_PRESS // Mouse button press and release
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION // Mouse movement
            | EventMask::ENTER_WINDOW // Track whether the cursor enters of leaves the window.
            | EventMask::LEAVE_WINDOW
            | EventMask::EXPOSURE
            | EventMask::NO_EVENT,
            )
            // Border pixel and color map need to be set if our depth may differ from the root depth.
            .border_pixel(screen.black_pixel)
            .colormap(colormap);

        let _ = connection.create_window(
            depth.depth,
            window,
            screen.root,
            0,
            0,
            size.w,
            size.h,
            0,
            WindowClass::INPUT_OUTPUT,
            visual_id,
            &window_aux,
        )?;

        // We only ever need one event id since we will only ever have one event context.
        let present_event_id = connection.generate_id()?;
        connection.present_select_input(
            present_event_id,
            window,
            present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY,
        )?;

        // Send requests to change window properties while we wait for the window creation request to complete.
        let window = WindowInner {
            connection: weak,
            id: window,
            root: screen.root,
            present_event_id,
            atoms,
            cursor_state: Arc::new(Mutex::new(CursorState::default())),
            size: Mutex::new(size),
            next_serial: AtomicU32::new(0),
            last_msc: Arc::new(AtomicU64::new(0)),
            format,
            depth,
            extensions,
        };

        // Enable WM_DELETE_WINDOW so our client is not disconnected upon our toplevel window being destroyed.
        connection.change_property32(
            PropMode::REPLACE,
            window.id,
            atoms.WM_PROTOCOLS,
            AtomEnum::ATOM,
            &[atoms.WM_DELETE_WINDOW],
        )?;

        // WM class cannot be safely changed later.
        let _ = connection.change_property8(
            PropMode::REPLACE,
            window.id,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"Smithay\0Wayland_Compositor\0",
        )?;

        window.set_title(title);
        window.map();

        // Flush requests to server so window is displayed.
        connection.flush()?;

        Ok(window)
    }

    pub fn map(&self) {
        if let Some(connection) = self.connection.upgrade() {
            let _ = connection.map_window(self.id);
        }
    }

    pub fn unmap(&self) {
        if let Some(connection) = self.connection.upgrade() {
            // ICCCM - Changing Window State
            //
            // Normal -> Withdrawn - The client should unmap the window and follow it with a synthetic
            // UnmapNotify event as described later in this section.
            let _ = connection.unmap_window(self.id);

            // Send a synthetic UnmapNotify event to make the ICCCM happy
            let _ = connection.send_event(
                false,
                self.id,
                EventMask::STRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_NOTIFY,
                UnmapNotifyEvent {
                    response_type: x11rb::protocol::xproto::UNMAP_NOTIFY_EVENT,
                    sequence: 0, // Ignored by X server
                    event: self.root,
                    window: self.id,
                    from_configure: false,
                },
            );
        }
    }

    pub fn size(&self) -> Size<u16, Logical> {
        *self.size.lock().unwrap()
    }

    pub fn set_title(&self, title: &str) {
        if let Some(connection) = self.connection.upgrade() {
            // _NET_WM_NAME should be preferred by window managers, but set both properties.
            let _ = connection.change_property8(
                PropMode::REPLACE,
                self.id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            );

            let _ = connection.change_property8(
                PropMode::REPLACE,
                self.id,
                self.atoms._NET_WM_NAME,
                self.atoms.UTF8_STRING,
                title.as_bytes(),
            );
        }
    }

    pub fn set_cursor_visible(&self, visible: bool) {
        if let Some(connection) = self.connection.upgrade() {
            let mut state = self.cursor_state.lock().unwrap();
            let changed = state.visible != visible;

            if changed && state.inside_window {
                state.visible = visible;
                self.update_cursor(&*connection, state.visible);
            }
        }
    }

    pub fn cursor_enter(&self) {
        if let Some(connection) = self.connection.upgrade() {
            let mut state = self.cursor_state.lock().unwrap();
            state.inside_window = true;
            self.update_cursor(&*connection, state.visible);
        }
    }

    pub fn cursor_leave(&self) {
        if let Some(connection) = self.connection.upgrade() {
            let mut state = self.cursor_state.lock().unwrap();
            state.inside_window = false;
            self.update_cursor(&*connection, true);
        }
    }

    fn update_cursor<C: ConnectionExt>(&self, connection: &C, visible: bool) {
        let _ = match visible {
            // This generates a Match error if we did not call Show/HideCursor before. Ignore that error.
            true => connection
                .xfixes_show_cursor(self.id)
                .map(|cookie| cookie.ignore_error()),
            false => connection
                .xfixes_hide_cursor(self.id)
                .map(|cookie| cookie.ignore_error()),
        };
    }
}

impl PartialEq for WindowInner {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.upgrade() {
            let _ = connection.destroy_window(self.id);
        }
    }
}
