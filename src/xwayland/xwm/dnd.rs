use std::{
    any::Any,
    fmt,
    os::fd::AsFd,
    sync::{atomic::Ordering, Arc, Mutex},
};

use atomic_float::AtomicF64;
use smallvec::SmallVec;
use tracing::warn;
#[cfg(feature = "wayland_frontend")]
use wayland_server::DisplayHandle;
use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        xfixes::SelectionNotifyEvent,
        xproto::{
            Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, EventMask, PropMode,
            Screen, Window as X11Window,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
    CURRENT_TIME,
};

use crate::{
    input::{
        dnd::{DndAction, DndFocus, OfferData, Source},
        Seat, SeatHandler,
    },
    utils::{Logical, Point, Serial},
    xwayland::{
        xwm::{selection::XWmSelection, Atoms},
        X11Surface, XwmHandler,
    },
};

const DND_VERSION: u32 = 5;
const MIN_DND_VERSION: u32 = 2;

#[derive(Debug)]
pub struct XWmDnd {
    pub selection: XWmSelection,
    pub active_offer: Option<XwmActiveOffer>,
}

impl XWmDnd {
    pub fn new(conn: &Arc<RustConnection>, screen: &Screen, atoms: &Atoms) -> Result<Self, ReplyOrIdError> {
        let selection = XWmSelection::new(conn, screen, atoms, atoms.XdndSelection)?;

        conn.change_property32(
            PropMode::REPLACE,
            selection.window,
            atoms.XdndAware,
            AtomEnum::ATOM,
            &[DND_VERSION],
        )?;
        conn.flush()?;

        Ok(XWmDnd {
            selection,
            active_offer: None,
        })
    }

    pub fn update_screen(&self, _screen: &Screen) -> Result<(), ReplyOrIdError> {
        // TODO: Update drop-in incoming windows
        /*
        self.conn.configure_window(
            self.selection.window,
            &ConfigureWindowAux::new()
                .width(screen.width_in_pixels as u32)
                .height(screen.height_in_pixels as u32),
        )?;
        */

        Ok(())
    }

    pub fn xfixes_selection_notify(
        &mut self,
        conn: &RustConnection,
        atoms: &Atoms,
        event: SelectionNotifyEvent,
    ) -> Result<(), ReplyOrIdError> {
        self.selection.owner = event.owner;
        if self.selection.owner == self.selection.window {
            self.selection.timestamp = event.timestamp;
            return Ok(());
        }

        if self.active_offer.is_some() {
            // rough X11 client tries to take over the selection, take it back.
            conn.set_selection_owner(self.selection.window, atoms.XdndSelection, CURRENT_TIME)?;
            return Ok(());
        }

        // TODO: start DndGrab for X -> WL Drag

        Ok(())
    }

    pub fn handle_status(
        &mut self,
        window: &X11Surface,
        status_msg: ClientMessageData,
        atoms: &Atoms,
        client_scale: Arc<AtomicF64>,
    ) {
        if let Some(offer) = self.active_offer.as_ref() {
            let data = status_msg.as_data32();
            let mut offer_state = offer.state.lock().unwrap();

            if offer_state.target == data[0] {
                let mut pos_update = false;
                offer_state.client_accepts = (data[1] & 1) != 0;

                if !offer_state.dropped {
                    let old_action = offer_state.preferred_action;
                    let new_action = DndAction::from_x(data[4], atoms);
                    if offer_state.supported_actions.contains(&new_action) {
                        offer_state.preferred_action = new_action;
                    } else if offer_state.supported_actions.contains(&DndAction::Copy) {
                        offer_state.preferred_action = DndAction::Copy;
                    } else {
                        offer_state.preferred_action = DndAction::None;
                    }

                    offer.source.choose_action(offer_state.preferred_action);
                    if old_action != offer_state.preferred_action {
                        pos_update = true;
                    }
                }

                // TODO: rectangle in data[2] and data[3] for optimizations

                offer_state.pos_pending = false;
                if let Some(pos) = offer_state
                    .pos_cached
                    .take()
                    .or(pos_update.then_some(offer_state.last_pos))
                {
                    let location = (window.geometry().loc + pos.to_i32_round())
                        .to_client_precise_round::<_, i32>(client_scale.load(Ordering::Acquire));

                    let data = [
                        self.selection.window,
                        0,
                        ((location.x << 16) | location.y) as u32,
                        CURRENT_TIME,
                        offer_state.preferred_action.to_x(atoms),
                    ];
                    let conn = window.conn.upgrade().unwrap();

                    if let Err(err) = conn.send_event(
                        false,
                        window.window_id(),
                        EventMask::NO_EVENT,
                        ClientMessageEvent::new(32, window.window_id(), atoms.XdndPosition, data),
                    ) {
                        warn!("Failed to send DND_POSITION event: {:?}", err);
                    }
                    let _ = conn.flush();
                    offer_state.pos_pending = true;
                } else if offer_state.dropped {
                    // finish drop

                    offer.source.drop_performed();

                    let data = [self.selection.window, 0, CURRENT_TIME, 0, 0];
                    let conn = window.conn.upgrade().unwrap();
                    if let Err(err) = conn.send_event(
                        false,
                        window.window_id(),
                        EventMask::NO_EVENT,
                        ClientMessageEvent::new(32, window.window_id(), self.selection.atoms.XdndDrop, data),
                    ) {
                        warn!("Failed to send DND_DROP event: {:?}", err);
                    }
                    let _ = conn.flush();
                }
            }
        }
    }

    pub fn handle_finished(&mut self, status_msg: ClientMessageData) {
        if let Some(offer) = self.active_offer.as_ref() {
            let offer_state = offer.state.lock().unwrap();
            let data = status_msg.as_data32();

            if data[0] != offer_state.target {
                return;
            }

            if !offer_state.dropped {
                return;
            }

            offer.source.finished();
            std::mem::drop(offer_state);
            self.active_offer.take();
        }
    }

    pub fn transfer(&mut self, mime_type: impl AsRef<str>, send_fd: impl AsFd) {
        if let Some(offer) = self.active_offer.as_ref() {
            offer.source.send(mime_type.as_ref(), send_fd.as_fd());
        }
    }
}

impl DndAction {
    fn from_x(atom: Atom, atoms: &Atoms) -> DndAction {
        match atom {
            x if x == atoms.XdndActionCopy => DndAction::Copy,
            x if x == atoms.XdndActionMove => DndAction::Move,
            x if x == atoms.XdndActionAsk => DndAction::Ask,
            _ => DndAction::None,
        }
    }

    fn to_x(&self, atoms: &Atoms) -> Atom {
        match self {
            DndAction::Copy => atoms.XdndActionCopy,
            DndAction::Move => atoms.XdndActionMove,
            DndAction::Ask => atoms.XdndActionAsk,
            DndAction::None => AtomEnum::NONE.into(),
        }
    }
}

#[derive(Debug)]
struct XwmOfferState {
    target: X11Window,

    preferred_action: DndAction,
    supported_actions: SmallVec<[DndAction; 3]>,

    pos_pending: bool,
    last_pos: Point<f64, Logical>,
    pos_cached: Option<Point<f64, Logical>>,

    client_accepts: bool,
    active: bool,
    dropped: bool,
}

pub struct XwmActiveOffer {
    state: Arc<Mutex<XwmOfferState>>,
    source: Arc<dyn Source>,
}

impl fmt::Debug for XwmActiveOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XwmActiveOffer")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Type implementing [`OfferData`](crate::input::dnd::OfferData) for XDND based DnD sources
#[derive(Debug)]
pub struct XwmOfferData<S: Source> {
    state: Arc<Mutex<XwmOfferState>>,
    source: Arc<S>,
}

impl<D: XwmHandler + SeatHandler> DndFocus<D> for X11Surface {
    type OfferData<S>
        = XwmOfferData<S>
    where
        S: Source;

    fn enter<S: Source>(
        &self,
        data: &mut D,
        _dh: &DisplayHandle,
        source: Arc<S>,
        seat: &Seat<D>,
        location: Point<f64, Logical>,
        _serial: &Serial,
    ) -> Option<Self::OfferData<S>> {
        let xwm = data.xwm_state(self.xwm_id()?);
        if xwm.dnd.selection.owner != xwm.dnd.selection.window {
            xwm.conn
                .set_selection_owner(xwm.dnd.selection.window, xwm.atoms.XdndSelection, CURRENT_TIME)
                .inspect_err(|err| warn!("Failed to take X11 DND_SELECTION: {:?}", err))
                .ok()?;
        }

        let prop = xwm
            .conn
            .get_property(false, self.window_id(), self.atoms.XdndAware, AtomEnum::ANY, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        if prop.type_ != AtomEnum::ATOM.into() {
            return None;
        }
        let client_ver = prop.value32()?.next()?;
        if client_ver < MIN_DND_VERSION {
            return None;
        }
        let version = client_ver.min(DND_VERSION);

        let metadata = source.metadata()?;
        let mime_types = metadata
            .mime_types
            .iter()
            .filter_map(|mime| {
                Some(match &**mime {
                    "text/plain" => xwm.atoms.TEXT,
                    "text/plain;charset=utf-8" => xwm.atoms.UTF8_STRING,
                    mime => {
                        xwm.conn
                            .intern_atom(false, mime.as_bytes())
                            .ok()?
                            .reply_unchecked()
                            .ok()??
                            .atom
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut enter_data = [
            xwm.dnd.selection.window,
            version << 24,
            AtomEnum::NONE.into(),
            AtomEnum::NONE.into(),
            AtomEnum::NONE.into(),
        ];
        for (i, mime_type) in mime_types.iter().take(3).enumerate() {
            enter_data[i + 2] = *mime_type;
        }

        if mime_types.len() > 3 {
            enter_data[1] |= 1;
            xwm.conn
                .change_property32(
                    PropMode::REPLACE,
                    xwm.dnd.selection.window,
                    self.atoms.XdndTypeList,
                    AtomEnum::ATOM,
                    &mime_types,
                )
                .inspect_err(|err| warn!("Failed to update DND_TYPE_LIST: {:?}", err))
                .ok()?;
        } else {
            xwm.conn
                .delete_property(xwm.dnd.selection.window, self.atoms.XdndTypeList)
                .inspect_err(|err| warn!("Failed to update DND_TYPE_LIST: {:?}", err))
                .ok()?;
        }
        xwm.dnd.selection.mime_types = metadata.mime_types;

        xwm.conn
            .send_event(
                false,
                self.window_id(),
                EventMask::NO_EVENT,
                ClientMessageEvent::new(32, self.window_id(), self.atoms.XdndEnter, enter_data),
            )
            .inspect_err(|err| warn!("Failed to send DND_ENTER event: {:?}", err))
            .ok()?;

        let mut offer = XwmOfferData {
            state: Arc::new(Mutex::new(XwmOfferState {
                target: self.window_id(),

                preferred_action: if metadata.dnd_actions.contains(&DndAction::Copy) {
                    DndAction::Copy
                } else {
                    DndAction::None
                },
                supported_actions: metadata.dnd_actions,

                last_pos: location,
                pos_pending: false,
                pos_cached: None,

                client_accepts: false,
                active: true,
                dropped: false,
            })),
            source,
        };
        xwm.dnd.active_offer = Some(XwmActiveOffer {
            state: offer.state.clone(),
            source: offer.source.clone(),
        });

        DndFocus::motion(self, data, Some(&mut offer), seat, location, 0);
        Some(offer)
    }

    fn motion<S: Source>(
        &self,
        data: &mut D,
        offer: Option<&mut XwmOfferData<S>>,
        _seat: &Seat<D>,
        location: Point<f64, Logical>,
        _time: u32,
    ) {
        let Some(offer) = offer else { return };
        if offer.state.lock().unwrap().dropped {
            return;
        }

        let preferred = {
            let mut state = offer.state.lock().unwrap();
            if state.pos_pending {
                state.pos_cached = Some(location);
                return;
            }
            state.pos_pending = true;
            state.preferred_action
        };

        let Some(xwm_id) = self.xwm_id() else { return };
        let xwm = data.xwm_state(xwm_id);
        let location = (self.geometry().loc + location.to_i32_round())
            .to_client_precise_round::<_, i32>(xwm.client_scale.load(Ordering::Acquire));

        let data = [
            xwm.dnd.selection.window,
            0,
            ((location.x << 16) | location.y) as u32,
            CURRENT_TIME,
            preferred.to_x(&xwm.atoms),
        ];

        if let Err(err) = xwm.conn.send_event(
            false,
            self.window_id(),
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, self.window_id(), self.atoms.XdndPosition, data),
        ) {
            warn!("Failed to send DND_POSITION event: {:?}", err);
        }
        let _ = xwm.conn.flush();
    }

    fn leave<S: Source>(&self, data: &mut D, offer: Option<&mut XwmOfferData<S>>, _seat: &Seat<D>) {
        let Some(xwm_id) = self.xwm_id() else { return };
        let Some(offer) = offer else { return };

        if !offer.state.lock().unwrap().dropped {
            let xwm = data.xwm_state(xwm_id);
            let data = [xwm.dnd.selection.window, 0, 0, 0, 0];

            if let Err(err) = xwm.conn.send_event(
                false,
                self.window_id(),
                EventMask::NO_EVENT,
                ClientMessageEvent::new(32, self.window_id(), self.atoms.XdndLeave, data),
            ) {
                warn!("Failed to send DND_LEAVE event: {:?}", err);
            }
            let _ = xwm.conn.flush();

            offer.source.cancel();
            xwm.dnd.active_offer = None;
        }
    }

    fn drop<S: Source>(&self, data: &mut D, offer: Option<&mut XwmOfferData<S>>, _seat: &Seat<D>) {
        let Some(xwm_id) = self.xwm_id() else { return };
        let Some(offer) = offer else { return };
        let xwm = data.xwm_state(xwm_id);

        let mut offer_state = offer.state.lock().unwrap();
        offer_state.dropped = true;
        if offer_state.pos_pending {
            return;
        }

        offer.source.drop_performed();

        let data = [xwm.dnd.selection.window, 0, CURRENT_TIME, 0, 0];
        if let Err(err) = xwm.conn.send_event(
            false,
            self.window_id(),
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, self.window_id(), self.atoms.XdndDrop, data),
        ) {
            warn!("Failed to send DND_DROP event: {:?}", err);
        }
        let _ = xwm.conn.flush();
    }
}

impl<S: Source> OfferData for XwmOfferData<S> {
    fn disable(&self) {
        self.state.lock().unwrap().active = false;
    }

    fn drop(&self) {
        self.state.lock().unwrap().dropped = true;
    }

    fn validated(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.active && state.preferred_action != DndAction::None && state.client_accepts
    }
}
