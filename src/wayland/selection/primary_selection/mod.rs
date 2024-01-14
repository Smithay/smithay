//! Utilities for manipulating the primary selection
//!
//! The primary selection is an additional protocol modeled after the data device to represent
//! and additional selection (copy/paste), a concept taken from the X11 Server.
//! This primary selection is a shortcut to the common clipboard selection,
//! where text just needs to be selected in order to allow copying it elsewhere
//! The de facto way to perform this action is the middle mouse button, although it is not limited to this one.
//!
//! This module provides the freestanding [`set_primary_focus`] function:
//!   This function sets the data device focus for a given seat; you'd typically call it
//!   whenever the keyboard focus changes, to follow it (for example in the focus hook of your keyboards).
//!
//! The module also provides an additional mechanism allowing your compositor to see and interact with
//! the contents of the primary selection:
//!
//! - the freestanding function [`set_primary_selection`]
//!   allows you to set the contents of the selection for your clients
//! - the `PrimarySelectionHandle` gives you the option to inspect new selections
//!   by overriding [`SelectionHandler::new_selection`].
//!
//! ## Initialization
//!
//! To initialize this implementation, create the [`PrimarySelectionState`], store it inside your `State` struct
//! and implement the [`PrimarySelectionHandler`] and [`SelectionHandler`], as shown in this example:
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! use smithay::delegate_primary_selection;
//! use smithay::wayland::selection::SelectionHandler;
//! use smithay::wayland::selection::primary_selection::{PrimarySelectionState, PrimarySelectionHandler};
//! # use smithay::input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus};
//! # use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
//!
//! # struct State { primary_selection_state: PrimarySelectionState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the primary_selection state
//! let primary_selection_state = PrimarySelectionState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // insert the PrimarySelectionState into your state
//! // ..
//!
//! // implement the necessary traits
//! # impl SeatHandler for State {
//! #     type KeyboardFocus = WlSurface;
//! #     type PointerFocus = WlSurface;
//! #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
//! #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//! #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! # }
//! impl SelectionHandler for State {
//!     type SelectionUserData = ();
//! }
//! impl PrimarySelectionHandler for State {
//!     fn primary_selection_state(&self) -> &PrimarySelectionState { &self.primary_selection_state }
//!     // ... override default implementations here to customize handling ...
//! }
//! delegate_primary_selection!(State);
//!
//! // You're now ready to go!
//! ```

use std::{
    cell::{Ref, RefCell},
    os::unix::io::OwnedFd,
};

use tracing::instrument;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 as PrimaryDeviceManager;
use wayland_server::{backend::GlobalId, Client, DisplayHandle, GlobalDispatch};

use crate::{
    input::{Seat, SeatHandler},
    wayland::selection::SelectionTarget,
};

mod device;
mod source;

pub use device::PrimaryDeviceUserData;
pub use source::{PrimarySourceUserData, SourceMetadata};

use super::source::CompositorSelectionProvider;
use super::SelectionHandler;
use super::{offer::OfferReplySource, seat_data::SeatData};

/// Access the primary selection state.
pub trait PrimarySelectionHandler: Sized + SeatHandler + SelectionHandler {
    /// [PrimarySelectionState] getter.
    fn primary_selection_state(&self) -> &PrimarySelectionState;
}

/// State of the primary selection.
#[derive(Debug)]
pub struct PrimarySelectionState {
    manager_global: GlobalId,
}

impl PrimarySelectionState {
    /// Register new [`PrimaryDeviceManager`] global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<PrimaryDeviceManager, ()> + 'static,
        D: PrimarySelectionHandler,
    {
        let manager_global = display.create_global::<D, PrimaryDeviceManager, _>(1, ());

        Self { manager_global }
    }

    /// [`PrimaryDeviceManager`] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.manager_global.clone()
    }
}

/// Set the primary selection focus to a certain client for a given seat
#[instrument(name = "wayland_primary_selection", level = "debug", skip(dh, seat, client), fields(seat = seat.name(), client = ?client.as_ref().map(|c| c.id())))]
pub fn set_primary_focus<D>(dh: &DisplayHandle, seat: &Seat<D>, client: Option<Client>)
where
    D: SeatHandler + PrimarySelectionHandler + 'static,
{
    seat.user_data()
        .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));
    let seat_data = seat
        .user_data()
        .get::<RefCell<SeatData<D::SelectionUserData>>>()
        .unwrap();
    seat_data.borrow_mut().set_primary_focus::<D>(dh, client);
}

/// Set a compositor-provided primary selection for this seat
///
/// You need to provide the available mime types for this selection.
///
/// Whenever a client requests to read the selection, your callback will
/// receive a [`SelectionHandler::send_selection`] event.
#[instrument(name = "wayland_primary_selection", level = "debug", skip(dh, seat, user_data), fields(seat = seat.name()))]
pub fn set_primary_selection<D>(
    dh: &DisplayHandle,
    seat: &Seat<D>,
    mime_types: Vec<String>,
    user_data: D::SelectionUserData,
) where
    D: SeatHandler + PrimarySelectionHandler + 'static,
{
    seat.user_data()
        .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));
    let seat_data = seat
        .user_data()
        .get::<RefCell<SeatData<D::SelectionUserData>>>()
        .unwrap();

    let selection = OfferReplySource::Compositor(CompositorSelectionProvider {
        ty: SelectionTarget::Primary,
        mime_types,
        user_data,
    });

    seat_data
        .borrow_mut()
        .set_primary_selection::<D>(dh, Some(selection));
}

/// Errors happening when requesting selection contents
#[derive(Debug, thiserror::Error)]
pub enum SelectionRequestError {
    /// Requested mime type is not available
    #[error("Requested mime type is not available")]
    InvalidMimetype,
    /// Requesting server side selection contents is not supported
    #[error("Current selection is server-side")]
    ServerSideSelection,
    /// There is no active selection
    #[error("No active selection to query")]
    NoSelection,
}

/// Request the current primary selection of the given seat
/// to be written to the provided file descriptor with the given mime type.
pub fn request_primary_client_selection<D>(
    seat: &Seat<D>,
    mime_type: String,
    fd: OwnedFd,
) -> Result<(), SelectionRequestError>
where
    D: SeatHandler + PrimarySelectionHandler + 'static,
{
    seat.user_data()
        .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));
    let seat_data = seat
        .user_data()
        .get::<RefCell<SeatData<D::SelectionUserData>>>()
        .unwrap();
    match seat_data.borrow().get_primary_selection() {
        None => Err(SelectionRequestError::NoSelection),
        Some(OfferReplySource::Client(source)) => {
            if !source.contains_mime_type(&mime_type) {
                Err(SelectionRequestError::InvalidMimetype)
            } else {
                source.send(mime_type, fd);
                Ok(())
            }
        }
        Some(OfferReplySource::Compositor(selection)) => {
            if !selection.mime_types.contains(&mime_type) {
                Err(SelectionRequestError::InvalidMimetype)
            } else {
                Err(SelectionRequestError::ServerSideSelection)
            }
        }
    }
}

/// Gets the user_data for the currently active selection, if set by the compositor
#[instrument(name = "wayland_primary_selection", level = "debug", skip_all, fields(seat = seat.name()))]
pub fn current_primary_selection_userdata<D>(seat: &Seat<D>) -> Option<Ref<'_, D::SelectionUserData>>
where
    D: SeatHandler + PrimarySelectionHandler + 'static,
{
    seat.user_data()
        .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));
    let seat_data = seat
        .user_data()
        .get::<RefCell<SeatData<D::SelectionUserData>>>()
        .unwrap();
    Ref::filter_map(seat_data.borrow(), |data| match data.get_primary_selection() {
        Some(OfferReplySource::Compositor(CompositorSelectionProvider { ref user_data, .. })) => {
            Some(user_data)
        }
        _ => None,
    })
    .ok()
}

/// Clear the current selection for this seat
#[instrument(name = "wayland_primary_selection", level = "debug", skip_all, fields(seat = seat.name()))]
pub fn clear_primary_selection<D>(dh: &DisplayHandle, seat: &Seat<D>)
where
    D: SeatHandler + PrimarySelectionHandler + 'static,
{
    seat.user_data()
        .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));
    let seat_data = seat
        .user_data()
        .get::<RefCell<SeatData<D::SelectionUserData>>>()
        .unwrap();
    seat_data.borrow_mut().set_primary_selection::<D>(dh, None);
}

mod handlers {
    use std::cell::RefCell;

    use tracing::error;
    use wayland_protocols::wp::primary_selection::zv1::server::{
        zwp_primary_selection_device_manager_v1::{
            self as primary_device_manager, ZwpPrimarySelectionDeviceManagerV1 as PrimaryDeviceManager,
        },
        zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1 as PrimaryDevice,
        zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1 as PrimarySource,
    };
    use wayland_server::{Dispatch, DisplayHandle, GlobalDispatch};

    use crate::{
        input::{Seat, SeatHandler},
        wayland::selection::{device::SelectionDevice, seat_data::SeatData},
    };

    use super::{device::PrimaryDeviceUserData, source::PrimarySourceUserData};
    use super::{PrimarySelectionHandler, PrimarySelectionState};

    impl<D> GlobalDispatch<PrimaryDeviceManager, (), D> for PrimarySelectionState
    where
        D: GlobalDispatch<PrimaryDeviceManager, ()>,
        D: Dispatch<PrimaryDeviceManager, ()>,
        D: Dispatch<PrimarySource, PrimarySourceUserData>,
        D: Dispatch<PrimaryDevice, PrimaryDeviceUserData>,
        D: PrimarySelectionHandler,
        D: 'static,
    {
        fn bind(
            _state: &mut D,
            _handle: &DisplayHandle,
            _client: &wayland_server::Client,
            resource: wayland_server::New<PrimaryDeviceManager>,
            _global_data: &(),
            data_init: &mut wayland_server::DataInit<'_, D>,
        ) {
            data_init.init(resource, ());
        }
    }

    impl<D> Dispatch<PrimaryDeviceManager, (), D> for PrimarySelectionState
    where
        D: Dispatch<PrimaryDeviceManager, ()>,
        D: Dispatch<PrimarySource, PrimarySourceUserData>,
        D: Dispatch<PrimaryDevice, PrimaryDeviceUserData>,
        D: PrimarySelectionHandler,
        D: SeatHandler,
        D: 'static,
    {
        fn request(
            _state: &mut D,
            client: &wayland_server::Client,
            _resource: &PrimaryDeviceManager,
            request: primary_device_manager::Request,
            _data: &(),
            _dhandle: &DisplayHandle,
            data_init: &mut wayland_server::DataInit<'_, D>,
        ) {
            match request {
                primary_device_manager::Request::CreateSource { id } => {
                    data_init.init(id, PrimarySourceUserData::new());
                }
                primary_device_manager::Request::GetDevice { id, seat: wl_seat } => {
                    match Seat::<D>::from_resource(&wl_seat) {
                        Some(seat) => {
                            seat.user_data()
                                .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));

                            let device = SelectionDevice::Primary(
                                data_init.init(id, PrimaryDeviceUserData { wl_seat }),
                            );

                            let seat_data = seat
                                .user_data()
                                .get::<RefCell<SeatData<D::SelectionUserData>>>()
                                .unwrap();

                            seat_data.borrow_mut().add_device(device);
                        }
                        None => {
                            error!(
                                primary_selection_device = ?id,
                                client = ?client,
                                "Unmanaged seat given to a primary selection device."
                            );
                        }
                    }
                }
                primary_device_manager::Request::Destroy => {}
                _ => unreachable!(),
            }
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_primary_selection {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1: ()
        ] => $crate::wayland::selection::primary_selection::PrimarySelectionState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1: ()
        ] => $crate::wayland::selection::primary_selection::PrimarySelectionState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1: $crate::wayland::selection::primary_selection::PrimaryDeviceUserData
        ] => $crate::wayland::selection::primary_selection::PrimarySelectionState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1: $crate::wayland::selection::primary_selection::PrimarySourceUserData
        ] => $crate::wayland::selection::primary_selection::PrimarySelectionState);
    };
}
