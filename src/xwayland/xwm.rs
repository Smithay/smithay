#![allow(missing_docs)]

use crate::{
    backend::renderer::{utils::draw_surface_tree, ImportAll, Renderer},
    utils::{x11rb::X11Source, Logical, Point, Rectangle, Size},
    wayland::compositor::{get_role, give_role},
};
use calloop::channel::SyncSender;
use std::{
    cell::RefCell,
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    convert::TryFrom,
    os::unix::net::UnixStream,
    rc::Rc,
    sync::{Arc, Weak},
};
use wayland_server::{protocol::wl_surface::WlSurface, Client};

use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        composite::{ConnectionExt as _, Redirect},
        xproto::{
            Atom, ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent, ConfigWindow,
            ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, Screen, StackMode,
            Window as X11Window, WindowClass,
        },
        Event,
    },
    rust_connection::{ConnectionError, DefaultStream, RustConnection},
    COPY_DEPTH_FROM_PARENT,
};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_S0,
        WL_SURFACE_ID,
        _LATE_SURFACE_ID,
        _SMITHAY_CLOSE_CONNECTION,
    }
}

#[derive(Debug, Clone)]
pub struct X11Surface {
    window: X11Window,
    wl_surface: Option<WlSurface>,
    conn: Weak<RustConnection>,
    state: Rc<RefCell<SharedSurfaceState>>,
}

#[derive(Debug)]
struct SharedSurfaceState {
    mapped: bool,
    mapped_onto: X11Window,
    location: Point<i32, Logical>,
    size: Size<i32, Logical>,
}

impl PartialEq for X11Surface {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window && self.wl_surface == other.wl_surface
    }
}

impl X11Surface {
    pub fn set_mapped(&self, mapped: bool) -> Result<(), ConnectionError> {
        if let Some(conn) = self.conn.upgrade() {
            let mut state = self.state.borrow_mut();
            let frame = state.mapped_onto;
            state.mapped = mapped;
            if mapped {
                conn.map_window(frame)?;
            } else {
                conn.unmap_window(frame)?;
            }
        }
        Ok(())
    }

    pub fn configure(&mut self, rect: Rectangle<i32, Logical>) -> Result<(), ConnectionError> {
        if let Some(conn) = self.conn.upgrade() {
            let aux = ConfigureWindowAux::default()
                .x(rect.loc.x)
                .y(rect.loc.y)
                .width(rect.size.w as u32)
                .height(rect.size.h as u32);
            conn.configure_window(self.window, &aux)?;
            let mut state = self.state.borrow_mut();
            state.location = rect.loc;
            state.size = rect.size;
        }
        Ok(())
    }

    pub fn window_id(&self) -> X11Window {
        self.window
    }

    pub fn wl_surface(&self) -> Option<&WlSurface> {
        self.wl_surface.as_ref()
    }

    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let state = self.state.borrow();
        Rectangle::from_loc_and_size(state.location, state.size)
    }
}

#[derive(Debug, Clone)]
pub enum X11Request {
    NewWindow {
        window: X11Surface,
        location: Point<i32, Logical>,
    },
    DestroyedWindow {
        window: X11Surface,
    },
    Configure {
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        border_width: Option<u32>,
        stacking: Option<(StackMode, X11Window)>,
    },
}

/// The runtime state of the XWayland window manager.
#[derive(Debug)]
pub struct X11WM {
    conn: Arc<RustConnection>,
    screen: Screen,
    atoms: Atoms,
    wl_client: Client,
    unpaired_surfaces: HashMap<u32, X11Window>,
    sequences_to_ignore: BinaryHeap<Reverse<u16>>,
    windows: Vec<X11Surface>,
    log: slog::Logger,
}

struct X11Injector {
    atom: Atom,
    sender: SyncSender<Event>,
}
impl X11Injector {
    pub fn late_window(&self, surface: &WlSurface) {
        let _ = self.sender.send(Event::ClientMessage(ClientMessageEvent {
            response_type: 0,
            format: 0,
            sequence: 0,
            window: 0,
            type_: self.atom,
            data: ClientMessageData::from([surface.as_ref().id(), 0, 0, 0, 0]),
        }));
    }
}

impl X11WM {
    pub fn start_wm<L>(
        connection: UnixStream,
        client: Client,
        log: L,
    ) -> Result<(Self, X11Source), Box<dyn std::error::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        // Create an X11 connection. XWayland only uses screen 0.
        let screen = 0;
        let stream = DefaultStream::from_unix_stream(connection)?;
        let conn = RustConnection::connect_to_stream(stream, screen)?;
        let atoms = Atoms::new(&conn)?.reply()?;

        let screen = conn.setup().roots[0].clone();

        // Actually become the WM by redirecting some operations
        conn.change_window_attributes(
            screen.root,
            &ChangeWindowAttributesAux::default()
                .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY),
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

        conn.flush()?;

        let log = crate::slog_or_fallback(log);
        let conn = Arc::new(conn);
        let source = X11Source::new(
            Arc::clone(&conn),
            win,
            atoms._SMITHAY_CLOSE_CONNECTION,
            log.clone(),
        );
        let injector = X11Injector {
            atom: atoms._LATE_SURFACE_ID,
            sender: source.sender.clone(),
        };
        client.data_map().insert_if_missing(move || injector);

        let wm = Self {
            conn,
            screen,
            atoms,
            wl_client: client,
            unpaired_surfaces: Default::default(),
            sequences_to_ignore: Default::default(),
            windows: Vec::new(),
            log,
        };
        Ok((wm, source))
    }

    pub fn handle_event<Impl>(&mut self, event: Event, callback: Impl) -> Result<(), ReplyOrIdError>
    where
        Impl: FnOnce(X11Request),
    {
        let mut should_ignore = false;
        if let Some(seqno) = event.wire_sequence_number() {
            // Check sequences_to_ignore and remove entries with old (=smaller) numbers.
            while let Some(&Reverse(to_ignore)) = self.sequences_to_ignore.peek() {
                // Sequence numbers can wrap around, so we cannot simply check for
                // "to_ignore <= seqno". This is equivalent to "to_ignore - seqno <= 0", which is what we
                // check instead. Since sequence numbers are unsigned, we need a trick: We decide
                // that values from [MAX/2, MAX] count as "<= 0" and the rest doesn't.
                if to_ignore.wrapping_sub(seqno) <= u16::max_value() / 2 {
                    // If the two sequence numbers are equal, this event should be ignored.
                    should_ignore = to_ignore == seqno;
                    break;
                }
                self.sequences_to_ignore.pop();
            }
        }

        slog::debug!(
            self.log,
            "X11: Got event {:?}{}",
            event,
            if should_ignore { " [ignored]" } else { "" }
        );
        if should_ignore {
            return Ok(());
        }

        match event {
            Event::CreateNotify(n) => {
                self.conn.composite_redirect_window(n.window, Redirect::MANUAL)?;
            }
            Event::ConfigureRequest(r) => {
                if let Some(surface) = self.windows.iter().find(|x| x.window == r.window) {
                    // Pass the request to downstream to decide
                    callback(X11Request::Configure {
                        window: surface.clone(),
                        x: if r.value_mask & u16::from(ConfigWindow::X) != 0 {
                            Some(i32::try_from(r.x).unwrap())
                        } else {
                            None
                        },
                        y: if r.value_mask & u16::from(ConfigWindow::Y) != 0 {
                            Some(i32::try_from(r.y).unwrap())
                        } else {
                            None
                        },
                        width: if r.value_mask & u16::from(ConfigWindow::WIDTH) != 0 {
                            Some(u32::try_from(r.width).unwrap())
                        } else {
                            None
                        },
                        height: if r.value_mask & u16::from(ConfigWindow::HEIGHT) != 0 {
                            Some(u32::try_from(r.height).unwrap())
                        } else {
                            None
                        },
                        border_width: if r.value_mask & u16::from(ConfigWindow::BORDER_WIDTH) != 0 {
                            Some(u32::try_from(r.border_width).unwrap())
                        } else {
                            None
                        },
                        stacking: if r.value_mask & u16::from(ConfigWindow::STACK_MODE) != 0
                            && r.value_mask & u16::from(ConfigWindow::SIBLING) != 0
                        {
                            Some((r.stack_mode, r.sibling))
                        } else {
                            None
                        },
                    });
                } else {
                    let aux = ConfigureWindowAux::from_configure_request(&r);
                    self.conn.configure_window(r.window, &aux)?;
                }
            }
            Event::MapRequest(r) => {
                // we reparent windows, because a lot of stuff expects, that we do
                let geo = self.conn.get_geometry(r.window)?.reply()?;
                let win = r.window;
                let frame_win = self.conn.generate_id()?;
                let win_aux = CreateWindowAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY);
                self.conn.create_window(
                    COPY_DEPTH_FROM_PARENT,
                    frame_win,
                    self.screen.root,
                    geo.x,
                    geo.y,
                    geo.width,
                    geo.height,
                    1,
                    WindowClass::INPUT_OUTPUT,
                    0,
                    &win_aux,
                )?;

                self.conn.grab_server()?;
                let cookie = self.conn.reparent_window(win, frame_win, 0, 0)?;
                self.conn.map_window(win)?;
                self.conn.map_window(frame_win)?;
                self.conn.ungrab_server()?;

                // Ignore all events caused by reparent_window(). All those events have the sequence number
                // of the reparent_window() request, thus remember its sequence number. The
                // grab_server()/ungrab_server() is done so that the server does not handle other clients
                // in-between, which could cause other events to get the same sequence number.
                self.sequences_to_ignore
                    .push(Reverse(cookie.sequence_number() as u16));

                let location = (geo.x as i32, geo.y as i32).into();
                let surface = X11Surface {
                    window: win,
                    wl_surface: None,
                    conn: Arc::downgrade(&self.conn),
                    state: Rc::new(RefCell::new(SharedSurfaceState {
                        mapped: true,
                        mapped_onto: frame_win,
                        location,
                        size: (geo.width as i32, geo.height as i32).into(),
                    })),
                };
                self.windows.push(surface);
            }
            Event::UnmapNotify(n) => {
                if let Some(pos) = self.windows.iter().position(|x| x.window == n.window) {
                    let surface = self.windows.remove(pos);
                    {
                        let state = surface.state.borrow();
                        self.conn.reparent_window(
                            n.window,
                            self.screen.root,
                            state.location.x as i16,
                            state.location.y as i16,
                        )?;
                        self.conn.destroy_window(state.mapped_onto)?;
                    }
                    callback(X11Request::DestroyedWindow {
                        window: surface.clone(),
                    });
                }
            }
            Event::ClientMessage(msg) => {
                if msg.type_ == self.atoms.WL_SURFACE_ID {
                    if let Some(surface) = self.windows.iter_mut().find(|x| x.window == msg.window) {
                        // We get a WL_SURFACE_ID message when Xwayland creates a WlSurface for a
                        // window. Both the creation of the surface and this client message happen at
                        // roughly the same time and are sent over different sockets (X11 socket and
                        // wayland socket). Thus, we could receive these two in any order. Hence, it
                        // can happen that we get None below when X11 was faster than Wayland.

                        let id = msg.data.as_data32()[0];
                        let wl_surface = self.wl_client.get_resource::<WlSurface>(id);
                        slog::info!(
                            self.log,
                            "X11 surface {:x?} corresponds to WlSurface {:?} = {:?}",
                            msg.window,
                            id,
                            surface,
                        );
                        match wl_surface {
                            None => {
                                self.unpaired_surfaces.insert(id, msg.window);
                            }
                            Some(wl_surface) => {
                                Self::new_window(surface, wl_surface, callback, self.log.clone());
                            }
                        }
                    }
                } else if msg.type_ == self.atoms._LATE_SURFACE_ID {
                    let id = msg.data.as_data32()[0];
                    if let Some(window) = self.unpaired_surfaces.remove(&id) {
                        if let Some(surface) = self.windows.iter_mut().find(|x| x.window == window) {
                            let wl_surface = self.wl_client.get_resource::<WlSurface>(id).unwrap();
                            Self::new_window(surface, wl_surface, callback, self.log.clone());
                        }
                    }
                }
            }
            _ => {}
        }
        self.conn.flush()?;
        Ok(())
    }

    fn new_window<Impl>(surface: &mut X11Surface, wl_surface: WlSurface, callback: Impl, log: ::slog::Logger)
    where
        Impl: FnOnce(X11Request),
    {
        slog::debug!(
            log,
            "Matched X11 surface {:x?} to {:x?}",
            surface.window,
            wl_surface
        );
        if give_role(&wl_surface, "x11_surface").is_err() {
            // It makes no sense to post a protocol error here since that would only kill Xwayland
            slog::error!(log, "Surface {:x?} already has a role?!", wl_surface);
            return;
        }

        let geometry = {
            let state = surface.state.borrow();
            Rectangle::from_loc_and_size(state.location, state.size)
        };

        surface.wl_surface = Some(wl_surface);
        callback(X11Request::NewWindow {
            window: surface.clone(),
            location: geometry.loc,
        });
    }
}

pub fn commit_hook(surface: &WlSurface) {
    // Is this the Xwayland client?
    if let Some(client) = surface.as_ref().client() {
        if let Some(x11) = client.data_map().get::<X11Injector>() {
            if get_role(surface).is_none() {
                x11.late_window(surface);
            }
        }
    }
}

pub fn draw_xwayland_surface<R>(
    renderer: &mut R,
    frame: &mut <R as Renderer>::Frame,
    surface: &X11Surface,
    scale: f64,
    damage: &[Rectangle<i32, Logical>],
    log: &slog::Logger,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    let location = surface.state.borrow().location;
    if let Some(wl_surface) = surface.wl_surface.as_ref() {
        draw_surface_tree(renderer, frame, wl_surface, scale, location, damage, log)
    } else {
        Ok(())
    }
}
