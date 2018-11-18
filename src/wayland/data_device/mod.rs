use std::sync::Mutex;

use wayland_server::{
    protocol::{
        wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_pointer, wl_surface,
    },
    Client, Display, Global, NewResource, Resource,
};

use wayland::seat::{AxisFrame, PointerGrab, PointerInnerHandle, Seat};

mod data_source;

pub use self::data_source::{with_source_metadata, SourceMetadata};

enum Selection {
    Empty,
    Client(Resource<wl_data_source::WlDataSource>),
}

struct SeatData {
    known_devices: Vec<Resource<wl_data_device::WlDataDevice>>,
    selection: Selection,
    log: ::slog::Logger,
    current_focus: Option<Client>,
}

impl SeatData {
    fn set_selection(&mut self, new_selection: Selection) {
        self.selection = new_selection;
        self.send_selection();
    }

    fn set_focus(&mut self, new_focus: Option<Client>) {
        self.current_focus = new_focus;
        self.send_selection();
    }

    fn send_selection(&mut self) {
        let client = match self.current_focus.as_ref() {
            Some(c) => c,
            None => return,
        };
        // first sanitize the selection, reseting it to null if the client holding
        // it dropped it
        let cleanup = if let Selection::Client(ref data_source) = self.selection {
            !data_source.is_alive()
        } else {
            false
        };
        if cleanup {
            self.selection = Selection::Empty;
        }
        // then send it if appropriate
        match self.selection {
            Selection::Empty => {
                // send an empty selection
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dd.client().map(|c| !c.equals(client)).unwrap_or(true) {
                        continue;
                    }
                    dd.send(wl_data_device::Event::Selection { id: None });
                }
            }
            Selection::Client(ref data_source) => {
                for dd in &self.known_devices {
                    // skip data devices not belonging to our client
                    if dd.client().map(|c| !c.equals(client)).unwrap_or(true) {
                        continue;
                    }
                    let source = data_source.clone();
                    let log = self.log.clone();
                    // create a corresponding data offer
                    let offer = client
                        .create_resource::<wl_data_offer::WlDataOffer>(dd.version())
                        .unwrap()
                        .implement(
                            move |req, _offer| match req {
                                wl_data_offer::Request::Receive { fd, mime_type } => {
                                    // check if the source and associated mime type is still valid
                                    let valid = with_source_metadata(&source, |meta| {
                                        meta.mime_types.contains(&mime_type)
                                    }).unwrap_or(false)
                                        && source.is_alive();
                                    if !valid {
                                        // deny the receive
                                        debug!(log, "Denying a wl_data_offer.receive with invalid source.");
                                    } else {
                                        source.send(wl_data_source::Event::Send { mime_type, fd });
                                    }
                                    let _ = ::nix::unistd::close(fd);
                                }
                                _ => { /* seleciton data offers only care about the `receive` event */ }
                            },
                            None::<fn(_)>,
                            (),
                        );
                    // advertize the offer to the client
                    dd.send(wl_data_device::Event::DataOffer { id: offer.clone() });
                    with_source_metadata(data_source, |meta| {
                        for mime_type in meta.mime_types.iter().cloned() {
                            offer.send(wl_data_offer::Event::Offer { mime_type })
                        }
                    }).unwrap();
                    dd.send(wl_data_device::Event::Selection { id: Some(offer) });
                }
            }
        }
    }
}

impl SeatData {
    fn new(log: ::slog::Logger) -> SeatData {
        SeatData {
            known_devices: Vec::new(),
            selection: Selection::Empty,
            log,
            current_focus: None,
        }
    }
}

/// Initialize the data device global
pub fn init_data_device<L>(
    display: &mut Display,
    logger: L,
) -> Global<wl_data_device_manager::WlDataDeviceManager>
where
    L: Into<Option<::slog::Logger>>,
{
    let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "data_device_mgr"));

    let global = display.create_global(3, move |new_ddm, _version| {
        implement_ddm(new_ddm, log.clone());
    });

    global
}

/// Set the data device focus to a certain client for a given seat
pub fn set_data_device_focus(seat: &Seat, client: Option<Client>) {
    // ensure the seat user_data is ready
    // TODO: find a better way to retrieve a logger without requiring the user
    // to provide one ?
    // This should be a rare path anyway, it is unlikely that a client gets focus
    // before initializing its data device, which would already init the user_data.
    seat.user_data().insert_if_missing(|| {
        Mutex::new(SeatData::new(
            seat.arc.log.new(o!("smithay_module" => "data_device_mgr")),
        ))
    });
    let seat_data = seat.user_data().get::<Mutex<SeatData>>().unwrap();
    seat_data.lock().unwrap().set_focus(client);
}

fn implement_ddm(
    new_ddm: NewResource<wl_data_device_manager::WlDataDeviceManager>,
    log: ::slog::Logger,
) -> Resource<wl_data_device_manager::WlDataDeviceManager> {
    use self::wl_data_device_manager::Request;
    new_ddm.implement(
        move |req, _ddm| match req {
            Request::CreateDataSource { id } => {
                self::data_source::implement_data_source(id);
            }
            Request::GetDataDevice { id, seat } => match Seat::from_resource(&seat) {
                Some(seat) => {
                    // ensure the seat user_data is ready
                    seat.user_data()
                        .insert_if_missing(|| Mutex::new(SeatData::new(log.clone())));
                    let seat_data = seat.user_data().get::<Mutex<SeatData>>().unwrap();
                    let data_device = implement_data_device(id, seat.clone(), log.clone());
                    seat_data.lock().unwrap().known_devices.push(data_device);
                }
                None => {
                    error!(log, "Unmanaged seat given to a data device.");
                }
            },
        },
        None::<fn(_)>,
        (),
    )
}

fn implement_data_device(
    new_dd: NewResource<wl_data_device::WlDataDevice>,
    seat: Seat,
    log: ::slog::Logger,
) -> Resource<wl_data_device::WlDataDevice> {
    use self::wl_data_device::Request;
    new_dd.implement(
        move |req, dd| match req {
            Request::StartDrag {
                source,
                origin,
                icon: _,
                serial,
            } => {
                /* TODO: handle the icon */
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.has_grab(serial) {
                        // The StartDrag is in response to a pointer implicit grab, all is good
                        pointer.set_grab(
                            DnDGrab {
                                data_source: source,
                                origin,
                                seat: seat.clone(),
                            },
                            serial,
                        );
                        return;
                    }
                }
                debug!(log, "denying drag from client without implicit grab");
            }
            Request::SetSelection { source, serial: _ } => {
                if let Some(keyboard) = seat.get_keyboard() {
                    if dd
                        .client()
                        .as_ref()
                        .map(|c| keyboard.has_focus(c))
                        .unwrap_or(false)
                    {
                        let seat_data = seat.user_data().get::<Mutex<SeatData>>().unwrap();
                        // The client has kbd focus, it can set the selection
                        seat_data
                            .lock()
                            .unwrap()
                            .set_selection(source.map(Selection::Client).unwrap_or(Selection::Empty));
                        return;
                    }
                }
                debug!(log, "denying setting selection by a non-focused client");
            }
            Request::Release => {
                // Clean up the known devices
                seat.user_data()
                    .get::<Mutex<SeatData>>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .known_devices
                    .retain(|ndd| ndd.is_alive() && (!ndd.equals(&dd)))
            }
        },
        None::<fn(_)>,
        (),
    )
}

struct DnDGrab {
    data_source: Option<Resource<wl_data_source::WlDataSource>>,
    origin: Resource<wl_surface::WlSurface>,
    seat: Seat,
}

impl PointerGrab for DnDGrab {
    fn motion(
        &mut self,
        handle: &mut PointerInnerHandle,
        location: (f64, f64),
        focus: Option<(Resource<wl_surface::WlSurface>, (f64, f64))>,
        serial: u32,
        time: u32,
    ) {

    }

    fn button(
        &mut self,
        handle: &mut PointerInnerHandle,
        button: u32,
        state: wl_pointer::ButtonState,
        serial: u32,
        time: u32,
    ) {

    }

    fn axis(&mut self, handle: &mut PointerInnerHandle, details: AxisFrame) {}
}
