use std::{
    collections::HashMap,
    fmt,
    os::fd::{BorrowedFd, OwnedFd},
    sync::{Arc, Mutex},
};

use calloop::{LoopHandle, RegistrationToken};
use tracing::{debug, trace, warn};
use x11rb::{
    connection::Connection as _,
    errors::ReplyOrIdError,
    protocol::{
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GetPropertyReply, PropMode,
            Screen, SelectionNotifyEvent, SelectionRequestEvent, Window as X11Window, WindowClass,
            SELECTION_NOTIFY_EVENT,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::{
    wayland::selection::SelectionTarget,
    xwayland::xwm::{Atoms, OwnedX11Window},
};

// copied from wlroots - docs say "maximum size can vary widely depending on the implementation"
// and there is no way to query the maximum size, you just get a non-descriptive `Length` error...
pub const INCR_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug)]
pub struct XWmSelection {
    pub atom: Atom,

    pub conn: Arc<RustConnection>,
    pub atoms: Atoms,
    pub window: OwnedX11Window,
    pub owner: X11Window,
    pub mime_types: Vec<String>,
    pub timestamp: u32,

    pub pending_transfers: Arc<Mutex<HashMap<X11Window, (OwnedX11Window, OwnedFd)>>>,
    pub incoming: HashMap<X11Window, IncomingTransfer>,
    pub outgoing: HashMap<X11Window, OutgoingTransfer>,
}

pub struct IncomingTransfer {
    pub token: Option<RegistrationToken>,
    pub window: OwnedX11Window,

    pub incr: bool,
    pub source_data: Vec<u8>,
    pub incr_done: bool,
}

impl fmt::Debug for IncomingTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncomingTransfer")
            .field("token", &self.token)
            .field("window", &self.window)
            .field("incr", &self.incr)
            .field("source_data", &self.source_data)
            .field("incr_done", &self.incr_done)
            .finish()
    }
}

impl IncomingTransfer {
    pub fn read_selection_prop(&mut self, reply: GetPropertyReply) {
        self.source_data.extend(&reply.value)
    }

    pub fn write_selection(&mut self, fd: BorrowedFd<'_>) -> std::io::Result<bool> {
        if self.source_data.is_empty() {
            return Ok(true);
        }

        let len = rustix::io::write(fd, &self.source_data)?;
        self.source_data = self.source_data.split_off(len);

        Ok(self.source_data.is_empty())
    }

    pub fn destroy<D>(mut self, handle: &LoopHandle<'_, D>) {
        if let Some(token) = self.token.take() {
            handle.remove(token);
        }
    }
}

impl Drop for IncomingTransfer {
    fn drop(&mut self) {
        if self.token.is_some() {
            tracing::warn!(
                ?self,
                "IncomingTransfer freed before being removed from EventLoop"
            );
        }
    }
}

pub struct OutgoingTransfer {
    pub conn: Arc<RustConnection>,
    pub token: Option<RegistrationToken>,

    pub incr: bool,
    pub source_data: Vec<u8>,
    pub request: SelectionRequestEvent,

    pub property_set: bool,
    pub flush_property_on_delete: bool,
    /// The final 0-byte data chunk has been sent, denoting the completion of this transfer
    pub sent_finished: bool,
}

impl fmt::Debug for OutgoingTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingTransfer")
            .field("conn", &"...")
            .field("token", &self.token)
            .field("incr", &self.incr)
            .field("source_data", &self.source_data)
            .field("request", &self.request)
            .field("property_set", &self.property_set)
            .field("flush_property_on_delete", &self.flush_property_on_delete)
            .finish()
    }
}

impl OutgoingTransfer {
    pub fn flush_data(&mut self) -> Result<usize, ReplyOrIdError> {
        let len = std::cmp::min(self.source_data.len(), INCR_CHUNK_SIZE);

        if len == 0 {
            // This flush will complete the transfer
            self.sent_finished = true;
        }

        let mut data = self.source_data.split_off(len);
        std::mem::swap(&mut data, &mut self.source_data);

        self.conn.change_property8(
            PropMode::REPLACE,
            self.request.requestor,
            self.request.property,
            self.request.target,
            &data,
        )?;
        self.conn.flush()?;

        let remaining = self.source_data.len();
        self.property_set = true;
        Ok(remaining)
    }

    pub fn destroy<D>(mut self, handle: &LoopHandle<'_, D>) {
        if let Some(token) = self.token.take() {
            handle.remove(token);
        }
    }
}

impl Drop for OutgoingTransfer {
    fn drop(&mut self) {
        if self.token.is_some() {
            tracing::warn!(
                ?self,
                "OutgoingTransfer freed before being removed from EventLoop"
            );
        }
    }
}

impl XWmSelection {
    pub fn new(
        conn: &Arc<RustConnection>,
        screen: &Screen,
        atoms: &Atoms,
        atom: Atom,
    ) -> Result<Self, ReplyOrIdError> {
        let window = conn.generate_id()?;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            10,
            10,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;

        if atom == atoms.CLIPBOARD {
            conn.set_selection_owner(window, atoms.CLIPBOARD_MANAGER, x11rb::CURRENT_TIME)?;
        }
        conn.xfixes_select_selection_input(
            window,
            atom,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        conn.flush()?;

        debug!(
            selection_window = ?window,
            ?atom,
            "Selection init",
        );

        Ok(XWmSelection {
            atom,
            conn: conn.clone(),
            atoms: *atoms,
            window: OwnedX11Window::new(window, conn),
            owner: x11rb::NONE,
            mime_types: Vec::new(),
            timestamp: x11rb::CURRENT_TIME,
            pending_transfers: Arc::new(Mutex::new(HashMap::new())),
            incoming: HashMap::new(),
            outgoing: HashMap::new(),
        })
    }

    pub fn window_destroyed<D>(&mut self, window: &X11Window, loop_handle: &LoopHandle<'_, D>) -> bool {
        (if let Some(transfer) = self.incoming.remove(window) {
            transfer.destroy(loop_handle);
            true
        } else {
            false
        }) || (if let Some(transfer) = self.outgoing.remove(window) {
            transfer.destroy(loop_handle);
            true
        } else {
            false
        }) || self.pending_transfers.lock().unwrap().remove(window).is_some()
    }

    pub fn has_window(&self, window: &X11Window) -> bool {
        self.window == *window
            || self.incoming.contains_key(window)
            || self.pending_transfers.lock().unwrap().contains_key(window)
    }

    pub fn type_(&self) -> Option<SelectionTarget> {
        match self.atom {
            x if x == self.atoms.CLIPBOARD => Some(SelectionTarget::Clipboard),
            x if x == self.atoms.PRIMARY => Some(SelectionTarget::Primary),
            _ => None,
        }
    }
}

pub enum OutgoingAction {
    Done,
    DoneReading,
    WaitForReadable,
}

pub fn read_selection_callback(
    conn: &RustConnection,
    atoms: &Atoms,
    fd: BorrowedFd<'_>,
    transfer: &mut OutgoingTransfer,
) -> Result<OutgoingAction, ReplyOrIdError> {
    let mut buf = [0; INCR_CHUNK_SIZE];
    let Ok(len) = rustix::io::read(fd, &mut buf) else {
        debug!(
            requestor = transfer.request.requestor,
            "File descriptor closed, aborting transfer."
        );
        send_selection_notify_resp(conn, &transfer.request, false)?;
        return Ok(OutgoingAction::Done);
    };
    trace!(
        requestor = transfer.request.requestor,
        "Transfer became readable, read {} bytes",
        len
    );

    transfer.source_data.extend_from_slice(&buf[..len]);
    if transfer.source_data.len() >= INCR_CHUNK_SIZE {
        if !transfer.incr {
            // start incr transfer
            trace!(
                requestor = transfer.request.requestor,
                "Transfer became incremental",
            );
            conn.change_property32(
                PropMode::REPLACE,
                transfer.request.requestor,
                transfer.request.property,
                atoms.INCR,
                &[INCR_CHUNK_SIZE as u32],
            )?;
            conn.flush()?;
            transfer.incr = true;
            transfer.property_set = true;
            transfer.flush_property_on_delete = true;
            send_selection_notify_resp(conn, &transfer.request, true)?;
        } else if transfer.property_set {
            // got more bytes, waiting for property delete
            transfer.flush_property_on_delete = true;
        } else {
            // got more bytes, property deleted
            let len = transfer.flush_data()?;
            trace!(
                requestor = transfer.request.requestor,
                "Send data chunk: {} bytes",
                len
            );
        }
    }

    if len == 0 {
        if transfer.incr {
            debug!("Incr transfer completed");
            if !transfer.property_set {
                let len = transfer.flush_data()?;
                trace!(
                    requestor = transfer.request.requestor,
                    "Send data chunk: {} bytes",
                    len
                );
            }
            transfer.flush_property_on_delete = true;
            Ok(OutgoingAction::DoneReading)
        } else {
            let len = transfer.flush_data()?;
            debug!("Non-Incr transfer completed with {} bytes", len);
            send_selection_notify_resp(conn, &transfer.request, true)?;
            Ok(OutgoingAction::Done)
        }
    } else {
        Ok(OutgoingAction::WaitForReadable)
    } // nothing to be done, buffered the bytes
}

pub enum IncomingAction {
    Done,
    WaitForProperty,
    WaitForWritable,
}

pub fn write_selection_callback(
    fd: BorrowedFd<'_>,
    conn: &RustConnection,
    atoms: &Atoms,
    transfer: &mut IncomingTransfer,
) -> Result<IncomingAction, ReplyOrIdError> {
    match transfer.write_selection(fd) {
        Ok(true) => {
            if transfer.incr {
                conn.delete_property(*transfer.window, atoms._WL_SELECTION)?;
                Ok(IncomingAction::WaitForProperty)
            } else {
                debug!(?transfer, "Non-Incr Transfer complete!");
                Ok(IncomingAction::Done)
            }
        }
        Ok(false) => Ok(IncomingAction::WaitForWritable),
        Err(err) => {
            warn!(?err, "Transfer errored");
            if transfer.incr {
                // even if it failed, we still need to drain the incr transfer
                conn.delete_property(*transfer.window, atoms._WL_SELECTION)?;
                Ok(IncomingAction::WaitForProperty)
            } else {
                Ok(IncomingAction::Done)
            }
        }
    }
}

pub fn send_selection_notify_resp(
    conn: &RustConnection,
    req: &SelectionRequestEvent,
    success: bool,
) -> Result<(), ReplyOrIdError> {
    conn.send_event(
        false,
        req.requestor,
        EventMask::NO_EVENT,
        SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property: if success {
                req.property
            } else {
                AtomEnum::NONE.into()
            },
        },
    )?;
    conn.flush()?;
    Ok(())
}
