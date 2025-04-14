//! Utilities for handling the `wp_presentation` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation, create [`PresentationState`], store it in your `State` struct and
//! implement the required traits, as shown in this example:
//!
//! ```
//! use smithay::wayland::presentation::PresentationState;
//! use smithay::delegate_presentation;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the presentation state:
//! let presentation_state = PresentationState::new::<State>(
//!     &display.handle(), // the display
//!     1 // the id of the clock
//! );
//!
//! // implement Dispatch for the Presentation types
//! delegate_presentation!(State);
//!
//! // You're now ready to go!
//! ```
//!
//! ### Use the presentation state
//!
//! Before sending the frame callbacks you should drain all committed presentation feedback callbacks.
//! After the associated frame has been presented the callbacks can be marked presented as shown in
//! the example.
//!
//! The [`presentation state`](PresentationFeedbackCachedState) is double-buffered and
//! can be accessed by using the [`with_states`] function
//!
//! ```no_run
//! # use smithay::{output::{Output, PhysicalProperties, Subpixel}, wayland::compositor::with_states};
//! # use wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, Resource};
//! # use std::time::Duration;
//! use smithay::wayland::presentation::{PresentationFeedbackCachedState, Refresh};
//! use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let dh = display.handle();
//! # let surface = WlSurface::from_id(&dh, ObjectId::null()).unwrap();
//! # let output = Output::new(
//! #     "output-0".into(), // the name of this output,
//! #     PhysicalProperties {
//! #         size: (200, 150).into(),        // dimensions (width, height) in mm
//! #         subpixel: Subpixel::HorizontalRgb,  // subpixel information
//! #         make: "Screens Inc".into(),     // make of the monitor
//! #         model: "Monitor Ultra".into(),  // model of the monitor
//! #     },
//! # );
//! // ... render frame ...
//!
//! let presentation_feedbacks = with_states(&surface, |states| {
//!     std::mem::take(&mut states.cached_state.get::<PresentationFeedbackCachedState>().current().callbacks)
//! });
//!
//! // ... send frame callbacks and present frame
//!
//! # let time = Duration::ZERO;
//! # let refresh = Refresh::fixed(Duration::from_secs_f64(1_000f64 / 60_000f64));
//! # let seq = 0;
//! for feedback in presentation_feedbacks {
//!     feedback.presented(&output, time, refresh, seq, wp_presentation_feedback::Kind::Vsync);
//! }
//! ```

use std::time::Duration;

use wayland_protocols::wp::presentation_time::server::{wp_presentation, wp_presentation_feedback};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface, Dispatch, DisplayHandle, GlobalDispatch, Resource, Weak,
};

use crate::output::Output;

use super::compositor::{with_states, Cacheable};

const EVT_PRESENTED_VARIABLE_SINCE: u32 = 2;

/// State of the wp_presentation global
#[derive(Debug)]
pub struct PresentationState {
    global: GlobalId,
}

impl PresentationState {
    /// Create new [`WpPresentation`](wp_presentation::WpPresentation) global.
    ///
    /// It returns the presentation state, which you can drop to remove these global from
    /// the event loop in the future.
    pub fn new<D>(display: &DisplayHandle, clk_id: u32) -> Self
    where
        D: GlobalDispatch<wp_presentation::WpPresentation, u32>
            + Dispatch<wp_presentation::WpPresentation, u32>
            + Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()>
            + 'static,
    {
        PresentationState {
            global: display.create_global::<D, wp_presentation::WpPresentation, u32>(2, clk_id),
        }
    }

    /// Returns the presentation global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<wp_presentation::WpPresentation, u32, D> for PresentationState
where
    D: GlobalDispatch<wp_presentation::WpPresentation, u32>,
    D: Dispatch<wp_presentation::WpPresentation, u32>,
    D: Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()>,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<wp_presentation::WpPresentation>,
        global_data: &u32,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let interface = data_init.init(resource, *global_data);
        interface.clock_id(*global_data);
    }
}

impl<D> Dispatch<wp_presentation::WpPresentation, u32, D> for PresentationState
where
    D: GlobalDispatch<wp_presentation::WpPresentation, u32>,
    D: Dispatch<wp_presentation::WpPresentation, u32>,
    D: Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()>,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &wp_presentation::WpPresentation,
        request: <wp_presentation::WpPresentation as Resource>::Request,
        data: &u32,
        _dhandle: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_presentation::Request::Feedback { surface, callback } => {
                let callback = data_init.init(callback, ());

                // TODO: Is there a better way to store the surface?
                with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<PresentationFeedbackCachedState>()
                        .pending()
                        .add_callback(&surface, *data, callback);
                });
            }
            wp_presentation::Request::Destroy => {
                // All is already handled by our destructor
            }
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<wp_presentation_feedback::WpPresentationFeedback, (), D> for PresentationFeedbackState
where
    D: GlobalDispatch<wp_presentation::WpPresentation, u32>,
    D: Dispatch<wp_presentation::WpPresentation, u32>,
    D: Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()>,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &wp_presentation_feedback::WpPresentationFeedback,
        _request: <wp_presentation_feedback::WpPresentationFeedback as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        // wp_presentation_feedback currently has no requests
    }
}

/// State for a single presentation feedback callback
#[derive(Debug)]
pub struct PresentationFeedbackState;

/// Holds a single presentation feedback
#[derive(Debug)]
pub struct PresentationFeedbackCallback {
    surface: Weak<wl_surface::WlSurface>,
    clk_id: u32,
    callback: wp_presentation_feedback::WpPresentationFeedback,
}

/// Refresh of the output on which the surface was presented
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Refresh {
    /// The output does not have a known refresh rate
    Unknown,
    /// The output has a variable refresh rate
    ///
    /// The passed refresh rate has to correspond to the minimum (fastest) rate
    Variable(Duration),
    /// The output has a fixed rate
    Fixed(Duration),
}

impl Refresh {
    /// Construct a [`Refresh::Variable`] from a [`Duration`]
    pub fn variable(min: impl Into<Duration>) -> Self {
        Self::Variable(min.into())
    }

    /// Construct a [`Refresh::Fixed`] from a [`Duration`]
    pub fn fixed(rate: impl Into<Duration>) -> Self {
        Self::Fixed(rate.into())
    }
}

impl PresentationFeedbackCallback {
    /// Get the id of the clock that was used at bind
    pub fn clk_id(&self) -> u32 {
        self.clk_id
    }

    /// Mark this callback as presented
    pub fn presented(
        self,
        output: &Output,
        time: impl Into<Duration>,
        refresh: Refresh,
        seq: u64,
        flags: wp_presentation_feedback::Kind,
    ) {
        // If the surface has been destroyed but the callback
        // has already been taken it won't be automatically
        // discarded, do that now
        let surface = match self.surface.upgrade() {
            Ok(surface) => surface,
            Err(_) => {
                self.discarded();
                return;
            }
        };

        // If the client disconnected there is no need to
        // send any feedback
        let client = match surface.client() {
            Some(client) => client,
            None => return,
        };

        for output in output.client_outputs(&client) {
            self.callback.sync_output(&output);
        }

        // Since version 2 wp_presentation supports variable refresh rates,
        // in which case the minimum (fastest rate) refresh is sent.
        // Clients binding an earlier version shall receive a zero refresh in this case.
        let refresh = match refresh {
            Refresh::Fixed(duration) => duration,
            Refresh::Variable(duration) if self.callback.version() >= EVT_PRESENTED_VARIABLE_SINCE => {
                duration
            }
            _ => Duration::ZERO,
        };

        let time = time.into();
        let tv_sec_hi = (time.as_secs() >> 32) as u32;
        let tv_sec_lo = (time.as_secs() & 0xFFFFFFFF) as u32;
        let tv_nsec = time.subsec_nanos();
        let refresh = refresh.as_nanos() as u32;
        let seq_hi = (seq >> 32) as u32;
        let seq_lo = (seq & 0xFFFFFFFF) as u32;

        self.callback
            .presented(tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, flags);
    }

    /// Mark this callback as discarded
    pub fn discarded(self) {
        self.callback.discarded()
    }
}

/// State of a single presentation feedback requested
/// for a surface
#[derive(Debug, Default)]
pub struct PresentationFeedbackCachedState {
    /// Holds the registered presentation feedbacks
    pub callbacks: Vec<PresentationFeedbackCallback>,
}

impl PresentationFeedbackCachedState {
    fn add_callback(
        &mut self,
        surface: &wl_surface::WlSurface,
        clk_id: u32,
        callback: wp_presentation_feedback::WpPresentationFeedback,
    ) {
        self.callbacks.push(PresentationFeedbackCallback {
            surface: surface.downgrade(),
            clk_id,
            callback,
        });
    }
}

impl Cacheable for PresentationFeedbackCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        PresentationFeedbackCachedState {
            callbacks: std::mem::take(&mut self.callbacks),
        }
    }

    fn merge_into(mut self, into: &mut Self, _dh: &DisplayHandle) {
        // In case our cached state does not contain any callbacks we can
        // exit early. This is important for commits on a subsurface tree
        // where we might end up merging the state multiple times which
        // would override the callbacks without this check.
        if self.callbacks.is_empty() {
            return;
        }

        // discard unprocessed callbacks as defined by the spec
        // ...the user did not see the content update because it was superseded...
        for callback in std::mem::replace(&mut into.callbacks, std::mem::take(&mut self.callbacks)) {
            callback.discarded();
        }
    }
}

impl Drop for PresentationFeedbackCachedState {
    fn drop(&mut self) {
        // discard unprocessed callbacks as defined by the spec
        // ...the user did not see the content update because it was superseded or its surface destroyed...
        for callback in self.callbacks.drain(..) {
            callback.discarded();
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_presentation {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation::WpPresentation: u32
        ] => $crate::wayland::presentation::PresentationState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation::WpPresentation: u32
        ] => $crate::wayland::presentation::PresentationState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback: ()
        ] => $crate::wayland::presentation::PresentationFeedbackState);
    };
}
