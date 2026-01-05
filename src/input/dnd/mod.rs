//! Drag'n'Drop related types for smithay's input-abstractions

mod grab;

use std::{any::Any, os::fd::OwnedFd, sync::Arc};

use smallvec::SmallVec;
#[cfg(feature = "wayland_frontend")]
use wayland_server::DisplayHandle;

#[cfg(feature = "xwayland")]
use crate::wayland::seat::WaylandFocus;
use crate::{
    input::{Seat, SeatHandler},
    utils::{IsAlive, Logical, Point, Serial},
};

pub use self::grab::*;

/// Enumeration of valid actions of a Drag'n'Drop operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DndAction {
    /// No action
    #[default]
    None,
    /// Data will be copied
    Copy,
    /// Data will be moved
    Move,
    /// User will be asked how to handle the data
    Ask,
}

/// The metadata describing a data source
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMetadata {
    /// The MIME types supported by this source
    pub mime_types: Vec<String>,
    /// The Drag'n'Drop actions supported by this source
    pub dnd_actions: SmallVec<[DndAction; 3]>,
}

impl Default for SourceMetadata {
    fn default() -> Self {
        Self {
            mime_types: Vec::new(),
            dnd_actions: SmallVec::new(),
        }
    }
}

/// A Drag'n'Drop data source
pub trait Source: IsAlive + Send + Sync + 'static {
    /// Method specifically for implementing drag'n'drop operations,
    /// which are only visible to a particular client with data
    /// being transferred out-of-band.
    fn is_client_local(&self, target: &dyn Any) -> bool {
        let _ = target;
        false
    }

    /// Access the metadata associated with this source.
    ///
    /// If this returns `None` the source is not managed by smithay (e.g. client_local)
    fn metadata(&self) -> Option<SourceMetadata>;
    /// An action was selected by the target
    fn choose_action(&self, action: DndAction);
    /// The target requests data to be transferred to the given file descriptor for the given mime-type
    fn send(&self, mime_type: &str, fd: OwnedFd);
    /// A drop was performed
    fn drop_performed(&self);
    /// The source is cancelled
    fn cancel(&self);
    /// The source is done
    fn finished(&self);
}

/// Data associated with an offer of a [`Source`] to a particular target.
pub trait OfferData: Send + 'static {
    /// The offer is now considered disabled and not valid anymore
    fn disable(&self);
    /// The offer is accepted and a drop is being performed
    fn drop(&self);
    /// Returns whether the offer is still considered valid
    fn validated(&self) -> bool;
}

// We sadly have to duplicate this whole trait for code in `DnDGrab`,
// because conditional trait bounds are not a thing (yet? rust-lang/rust#115590)
#[cfg(feature = "xwayland")]
/// A potential Drag'n'Drop target
pub trait DndFocus<D: SeatHandler>: WaylandFocus + IsAlive + PartialEq {
    /// OfferData implementation returned by this target
    type OfferData<S>: OfferData
    where
        S: Source;

    /// An active Drag'n'Drop operation has entered the client
    fn enter<S: Source>(
        &self,
        data: &mut D,
        #[cfg(feature = "wayland_frontend")] dh: &DisplayHandle,
        source: Arc<S>,
        seat: &Seat<D>,
        location: Point<f64, Logical>,
        serial: &Serial,
    ) -> Option<Self::OfferData<S>>;

    /// An active Drag'n'Drop operation, which has previously
    /// entered the client, has been moved
    fn motion<S: Source>(
        &self,
        data: &mut D,
        offer: Option<&mut Self::OfferData<S>>,
        seat: &Seat<D>,
        location: Point<f64, Logical>,
        time: u32,
    );

    /// An active Drag'n'Drop operation, which has previously
    /// entered the client, left again.
    fn leave<S: Source>(&self, data: &mut D, offer: Option<&mut Self::OfferData<S>>, seat: &Seat<D>);

    /// An active Drag'n'Drop operation, which has previously
    /// entered the client, has been dropped.
    fn drop<S: Source>(&self, data: &mut D, offer: Option<&mut Self::OfferData<S>>, seat: &Seat<D>);
}
#[cfg(not(feature = "xwayland"))]
/// A potential Drag'n'Drop target
pub trait DndFocus<D: SeatHandler>: IsAlive + PartialEq {
    /// OfferData implementation returned by this target
    type OfferData<S>: OfferData
    where
        S: Source;

    /// An active Drag'n'Drop operation has entered the client
    fn enter<S: Source>(
        &self,
        data: &mut D,
        #[cfg(feature = "wayland_frontend")] dh: &DisplayHandle,
        source: Arc<S>,
        seat: &Seat<D>,
        location: Point<f64, Logical>,
        serial: &Serial,
    ) -> Option<Self::OfferData<S>>;

    /// An active Drag'n'Drop operation, which has previously
    /// entered the client, has been moved
    fn motion<S: Source>(
        &self,
        data: &mut D,
        offer: Option<&mut Self::OfferData<S>>,
        seat: &Seat<D>,
        location: Point<f64, Logical>,
        time: u32,
    );

    /// An active Drag'n'Drop operation, which has previously
    /// entered the client, left again.
    fn leave<S: Source>(&self, data: &mut D, offer: Option<&mut Self::OfferData<S>>, seat: &Seat<D>);

    /// An active Drag'n'Drop operation, which has previously
    /// entered the client, has been dropped.
    fn drop<S: Source>(&self, data: &mut D, offer: Option<&mut Self::OfferData<S>>, seat: &Seat<D>);
}
