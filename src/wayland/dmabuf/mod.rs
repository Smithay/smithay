//! Linux DMABUF protocol
//!
//! This module provides helper to handle the linux-dmabuf protocol, which allows clients to submit their
//! contents as dmabuf file descriptors. These handlers automate the aggregation of the metadata associated
//! with a dma buffer, and do some basic checking of the sanity of what the client sends.
//!
//! ## How to use
//!
//! To setup the dmabuf global, you will need to provide 2 things:
//!
//! - the default [`DmabufFeedback`] containing the main device and the formats you wish to support when creating the `Global` through [`DmabufState::create_global_with_default_feedback`]
//! - an implementation of [`DmabufHandler`] to test if a dmabuf buffer can be imported by your renderer and optionally override the initial surface feedback
//!
//! The list of supported formats is a `Vec<Format>`, where you will enter all the (code, modifier) pairs you
//! support. You can typically receive a list of supported formats for one renderer by calling
//! [`ImportDma::dmabuf_formats`](crate::backend::renderer::ImportDma::dmabuf_formats).
//!
//! ```no_run
//! use smithay::{
//!     delegate_dmabuf,
//!     backend::allocator::dmabuf::{Dmabuf},
//!     reexports::{
//!         wayland_server::protocol::{
//!             wl_buffer::WlBuffer,
//!             wl_surface::WlSurface,
//!         }
//!     },
//!     wayland::{
//!         buffer::BufferHandler,
//!         dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier}
//!     },
//! };
//!
//! pub struct State {
//!     dmabuf_state: DmabufState,
//!     dmabuf_global: DmabufGlobal,
//! }
//!
//! // Smithay's "DmabufHandler" also requires the buffer management utilities, you need to implement
//! // "BufferHandler".
//! impl BufferHandler for State {
//!     fn buffer_destroyed(&mut self, buffer: &wayland_server::protocol::wl_buffer::WlBuffer) {
//!         // All renderers can handle buffer destruction at this point. Some parts of window management may
//!         // also use this function.
//!         //
//!         // If you need to mark a dmabuf elsewhere in your state as destroyed, you use the "get_dmabuf"
//!         // function defined in this module to access the dmabuf associated the "Buffer".
//!     }
//! }
//!
//! impl DmabufHandler for State {
//!     fn dmabuf_state(&mut self) -> &mut DmabufState {
//!         &mut self.dmabuf_state
//!     }
//!
//!     fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
//!         // Here you should import the dmabuf into your renderer.
//!         //
//!         // The notifier is used to communicate whether import was successful. In this example we
//!         // call successful to notify the client import was successful.
//!         notifier.successful::<State>();
//!     }
//!
//!     fn new_surface_feedback(
//!         &mut self,
//!         surface: &WlSurface,
//!         global: &DmabufGlobal,
//!     ) -> Option<DmabufFeedback> {
//!         // Here you can override the initial feedback sent to a client requesting feedback for a specific
//!         // surface. Returning `None` instructs the global to return the default feedback to the client which
//!         // is also the default implementation for this function when not overridden
//!         None
//!     }
//! }
//!
//! // Delegate dmabuf handling for State to DmabufState.
//! delegate_dmabuf!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! // First a DmabufState must be created. This type is used to create some "DmabufGlobal"s
//! let mut dmabuf_state = DmabufState::new();
//!
//! // ...identify primary render node and load dmabuf formats supported for rendering...
//! # let main_device = { todo!() };
//! # let formats: Vec<_> = { todo!() };
//!
//! // Build the default feedback from the device node of the primary render node and
//! // the supported dmabuf formats
//! let default_feedback = DmabufFeedbackBuilder::new(main_device, formats).build().unwrap();
//!
//! // And create the dmabuf global.
//! let dmabuf_global = dmabuf_state.create_global_with_default_feedback::<State>(
//!     &display_handle,
//!     &default_feedback,
//! );
//!
//! let state = State {
//!     dmabuf_state,
//!     dmabuf_global,
//! };
//!
//! // Rest of the compositor goes here...
//! ```
//!
//! Accessing a [`Dmabuf`] associated with a [`WlBuffer`]
//! may be achieved using [`get_dmabuf`].
//!
//! #### Notes on supporting per surface feedback
//!
//! If a client requests [`DmabufFeedback`] for a specific [`WlSurface`] it can be used to inform the client about
//! sub-optimal buffer allocations. This is especially important to support direct scan-out over drm planes
//! that typically only support a subset of supported formats from the rendering formats.
//!
//! The [`DmabufFeedback`] of a specific [`WlSurface`] can be updated by retrieving the [`SurfaceDmabufFeedbackState`]
//! with [`SurfaceDmabufFeedbackState::from_states`] and setting the feedback with [`SurfaceDmabufFeedbackState::set_feedback`].
//!
//! [`DmabufFeedback`] uses preference tranches to inform the client about formats that could result on more optimal buffer placement.
//! Preference tranches can be added to the feedback during initialization with [`DmabufFeedbackBuilder::add_preference_tranche`].
//! Note that the order of formats within a tranche (`target_device` + `flags`) is undefined, if you want to communicate preference
//! of a specific format you have to split the formats into multiple tranches. A tranche can additionally define [`TrancheFlags`](zwp_linux_dmabuf_feedback_v1::TrancheFlags)
//! which can give clients additional context what the tranche represents. As an example formats gathered from drm planes
//! should define [`TrancheFlags::Scanout`](`zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout) to communicate that buffers should be allocated so that
//! they support scan-out by the device specified as the `target device`.
//!
//! Note: Surface feedback only represents an optimization and the fallback path using compositing should always be supported, so
//! typically you do not want to announce formats in a preference tranche that are not supported by the main device for rendering.
//!
//! #### Notes on clients binding version 3 or lower
//!
//! During instantiation the global will automatically build a format list from the provided [`DmabufFeedback`] consisting of all formats that are part of a tranche
//! having the `target device` equal the `main device` and defining no special [`TrancheFlags`](zwp_linux_dmabuf_feedback_v1::TrancheFlags).
//!
//! ### Without feedback (v3)
//!
//! It is also possible to initialize the `Global` without support for [`DmabufFeedback`] by using [`DmabufState::create_global`] which
//! will then only advertise version `3` to clients. This is mostly meant to guarantee an easy update path for compositors already
//! supporting dmabuf version `3` without breakage.
//!
//! ```no_run
//! # extern crate wayland_server;
//! # use smithay::{
//! #     delegate_dmabuf,
//! #     backend::allocator::dmabuf::Dmabuf,
//! #     reexports::{wayland_server::protocol::wl_buffer::WlBuffer},
//! #     wayland::{
//! #         buffer::BufferHandler,
//! #         dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier}
//! #     },
//! # };
//! # pub struct State {
//! #     dmabuf_state: DmabufState,
//! #     dmabuf_global: DmabufGlobal,
//! # }
//! # impl BufferHandler for State {
//! #     fn buffer_destroyed(&mut self, buffer: &wayland_server::protocol::wl_buffer::WlBuffer) { }
//! # }
//! # impl DmabufHandler for State {
//! #     fn dmabuf_state(&mut self) -> &mut DmabufState {
//! #         &mut self.dmabuf_state
//! #     }
//! #     fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {}
//! # }
//! # delegate_dmabuf!(State);
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! # let mut dmabuf_state = DmabufState::new();
//! #
//! // define your supported formats
//! let formats = vec![
//!     /* ... */
//! ];
//!
//! // And create the dmabuf global.
//! let dmabuf_global = dmabuf_state.create_global::<State>(
//!     &display_handle,
//!     formats,
//! );
//!
//! let state = State {
//!     dmabuf_state,
//!     dmabuf_global,
//! };
//!
//! // Rest of the compositor goes here...
//! ```

mod dispatch;

use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    ops::Sub,
    os::unix::io::AsFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use indexmap::IndexSet;
use rustix::fs::{seek, SeekFrom};
use wayland_protocols::wp::linux_dmabuf::zv1::server::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_server::{
    backend::{GlobalId, InvalidId},
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_surface::WlSurface,
    },
    Client, Dispatch, DisplayHandle, GlobalDispatch, Resource, WEnum,
};

#[cfg(feature = "backend_drm")]
use crate::backend::drm::DrmNode;
use crate::{
    backend::allocator::{
        dmabuf::{Dmabuf, DmabufFlags, Plane},
        Format, Fourcc, Modifier,
    },
    utils::{ids::id_gen, sealed_file::SealedFile, UnmanagedResource},
};

use super::{buffer::BufferHandler, compositor};

#[derive(Debug, Clone, PartialEq)]
struct DmabufFeedbackTranche {
    target_device: libc::dev_t,
    flags: zwp_linux_dmabuf_feedback_v1::TrancheFlags,
    indices: IndexSet<usize>,
}

#[derive(Debug)]
struct DmabufFeedbackFormatTable {
    formats: IndexSet<Format>,
    file: SealedFile,
}

#[derive(Debug)]
struct DmabufFeedbackInner {
    main_device: libc::dev_t,
    format_table: DmabufFeedbackFormatTable,
    tranches: Vec<DmabufFeedbackTranche>,
}

impl PartialEq for DmabufFeedbackInner {
    fn eq(&self, other: &Self) -> bool {
        self.main_device == other.main_device
            && self.format_table.formats == other.format_table.formats
            && self.tranches == other.tranches
    }
}

/// Builder for [`DmabufFeedback`]
#[derive(Debug, Clone)]
pub struct DmabufFeedbackBuilder {
    main_device: libc::dev_t,
    main_tranche: DmabufFeedbackTranche,
    formats: IndexSet<Format>,
    preferred_tranches: Vec<DmabufFeedbackTranche>,
}

#[derive(Copy, Clone)]
struct DmabufFeedbackFormat {
    format: u32,
    _reserved: u32,
    modifier: u64,
}

impl DmabufFeedbackFormat {
    fn to_ne_bytes(self) -> [u8; 16] {
        let format: [u8; 4] = self.format.to_ne_bytes();
        let reserved: [u8; 4] = self._reserved.to_ne_bytes();
        let modifier: [u8; 8] = self.modifier.to_ne_bytes();

        [
            format[0],
            format[1],
            format[2],
            format[3],
            reserved[0],
            reserved[1],
            reserved[2],
            reserved[3],
            modifier[0],
            modifier[1],
            modifier[2],
            modifier[3],
            modifier[4],
            modifier[5],
            modifier[6],
            modifier[7],
        ]
    }
}

impl From<Format> for DmabufFeedbackFormat {
    fn from(format: Format) -> Self {
        DmabufFeedbackFormat {
            format: format.code as u32,
            _reserved: 0,
            modifier: format.modifier.into(),
        }
    }
}

impl DmabufFeedbackBuilder {
    /// Create a new feedback builder with the specified device and formats as the main device
    ///
    /// Preference tranches can be added with [`DmabufFeedbackBuilder::add_preference_tranche`]
    /// and the main tranche will be put after all preference tranches
    pub fn new(main_device: libc::dev_t, formats: impl IntoIterator<Item = Format>) -> Self {
        let feedback_formats: IndexSet<Format> = formats.into_iter().collect();
        let format_indices: IndexSet<usize> = (0..feedback_formats.len()).collect();
        let main_tranche = DmabufFeedbackTranche {
            flags: zwp_linux_dmabuf_feedback_v1::TrancheFlags::empty(),
            indices: format_indices,
            target_device: main_device,
        };

        Self {
            main_device,
            formats: feedback_formats,
            main_tranche,
            preferred_tranches: Vec::new(),
        }
    }

    /// Adds a preference tranche to the builder
    ///
    /// The tranches will be reported in the order they have been added with
    /// this function.
    ///
    /// Note: Formats already present in a previously added preference tranche with the
    /// same target device and flags will be skipped.
    /// If all formats are already included in a previously added tranche
    /// with the same target device and flags this tranche will be skipped.
    pub fn add_preference_tranche(
        mut self,
        target_device: libc::dev_t,
        flags: Option<zwp_linux_dmabuf_feedback_v1::TrancheFlags>,
        formats: impl IntoIterator<Item = Format>,
    ) -> Self {
        let flags = flags.unwrap_or(zwp_linux_dmabuf_feedback_v1::TrancheFlags::empty());

        let mut tranche = DmabufFeedbackTranche {
            target_device,
            flags,
            indices: Default::default(),
        };

        for format in formats {
            let (format_index, added) = self.formats.insert_full(format);

            // Compositors must not send duplicate format + modifier pairs within
            // the same tranche or across two different tranches with the same target device and flags.
            //
            // if the format has just been added there is no need to test if a previous tranche
            // with the same target device and flags already contains the format
            let duplicate_format = !added
                && self.preferred_tranches.iter().any(|tranche| {
                    tranche.target_device == target_device
                        && tranche.flags == flags
                        && tranche.indices.contains(&format_index)
                });

            if duplicate_format {
                continue;
            }

            // ...Compositors must not send duplicate format + modifier pairs within the same tranche...
            // this is handled by using a IndexSet which won't hold duplicates
            tranche.indices.insert(format_index);
        }

        if !tranche.indices.is_empty() {
            self.preferred_tranches.push(tranche);
        }

        self
    }

    /// Build the [`DmabufFeedback`]
    ///
    /// Returns an error if the format table shared memory file could
    /// not be created.
    pub fn build(mut self) -> Result<DmabufFeedback, std::io::Error> {
        let formats = self
            .formats
            .iter()
            .copied()
            .map(DmabufFeedbackFormat::from)
            .flat_map(DmabufFeedbackFormat::to_ne_bytes)
            .collect::<Vec<_>>();

        let name = CString::new("smithay-dmabuffeedback-format-table").unwrap();
        let format_table_file = SealedFile::with_data(name, &formats)?;

        // remove all formats from the main tranche that are already covered
        // by a preference tranche
        for duplicate_main_tranche in self.preferred_tranches.iter().filter(|tranche| {
            tranche.target_device == self.main_tranche.target_device
                && tranche.flags == self.main_tranche.flags
        }) {
            self.main_tranche.indices = self.main_tranche.indices.sub(&duplicate_main_tranche.indices);
        }

        if !self.main_tranche.indices.is_empty() {
            self.preferred_tranches.push(self.main_tranche);
        }

        Ok(DmabufFeedback(Arc::new(DmabufFeedbackInner {
            main_device: self.main_device,
            format_table: DmabufFeedbackFormatTable {
                file: format_table_file,
                formats: self.formats,
            },
            tranches: self.preferred_tranches,
        })))
    }
}

/// Feedback for dmabuf allocation
///
/// Use the [`DmabufFeedbackBuilder`] to create a new instance.
#[derive(Debug, Clone)]
pub struct DmabufFeedback(Arc<DmabufFeedbackInner>);

impl PartialEq for DmabufFeedback {
    fn eq(&self, other: &Self) -> bool {
        // Note: The dmabuf feedback can not change, it is initialized
        // with the DmabufFeedbackBuilder, so if the arc ptr equal we
        // can short-circuit the expensive equality test saving us
        // a few cpu cycles
        if Arc::ptr_eq(&self.0, &other.0) {
            return true;
        }

        self.0 == other.0
    }
}

impl DmabufFeedback {
    /// Send this feedback to the provided [`ZwpLinuxDmabufFeedbackV1`](zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1)
    pub fn send(&self, feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1) {
        feedback.main_device(self.0.main_device.to_ne_bytes().to_vec());
        feedback.format_table(
            self.0.format_table.file.as_fd(),
            self.0.format_table.file.size() as u32,
        );

        for tranche in self.0.tranches.iter() {
            feedback.tranche_target_device(tranche.target_device.to_ne_bytes().to_vec());
            feedback.tranche_flags(tranche.flags);
            feedback.tranche_formats(
                tranche
                    .indices
                    .iter()
                    .flat_map(|i| (*i as u16).to_ne_bytes())
                    .collect::<Vec<_>>(),
            );
            feedback.tranche_done();
        }

        feedback.done();
    }

    fn main_formats(&self) -> Vec<Format> {
        self.0
            .tranches
            .iter()
            .filter(|tranche| tranche.target_device == self.0.main_device && tranche.flags.is_empty())
            .map(|tranche| tranche.indices.clone())
            .reduce(|mut acc, item| {
                acc.extend(item);
                acc
            })
            .unwrap_or_default()
            .into_iter()
            .map(|index| self.0.format_table.formats[index])
            .collect()
    }
}

#[derive(Debug)]
struct SurfaceDmabufFeedbackStateInner {
    feedback: DmabufFeedback,
    known_instances: Vec<wayland_server::Weak<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>>,
}

/// Feedback state for a surface
#[derive(Debug, Clone, Default)]
pub struct SurfaceDmabufFeedbackState {
    inner: Arc<Mutex<Option<SurfaceDmabufFeedbackStateInner>>>,
}

impl SurfaceDmabufFeedbackState {
    /// Get the surface dmabuf feedback stored in the surface states
    ///
    /// Returns `None` if no feedback has been requested
    pub fn from_states(states: &compositor::SurfaceData) -> Option<&Self> {
        states.data_map.get::<SurfaceDmabufFeedbackState>()
    }

    /// Set the feedback for this surface
    ///
    /// Note: If the surface did not request feedback or the feedback equals
    /// the current feedback this function does nothing
    pub fn set_feedback(&self, feedback: &DmabufFeedback) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(inner) = guard.as_mut() {
            if &inner.feedback == feedback {
                return;
            }

            for instance in inner.known_instances.iter().filter_map(|i| i.upgrade().ok()) {
                feedback.send(&instance);
            }

            inner.feedback = feedback.clone();
        }
    }

    fn add_instance<F>(
        &self,
        instance: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        feedback_factory: F,
    ) -> DmabufFeedback
    where
        F: FnOnce() -> DmabufFeedback,
    {
        let mut guard = self.inner.lock().unwrap();
        if let Some(inner) = guard.as_mut() {
            inner.known_instances.push(instance.downgrade());
            inner.feedback.clone()
        } else {
            let feedback = feedback_factory();
            let inner = SurfaceDmabufFeedbackStateInner {
                feedback: feedback.clone(),
                known_instances: vec![instance.downgrade()],
            };
            *guard = Some(inner);
            feedback
        }
    }

    fn remove_instance(&self, instance: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1) {
        let mut guard = self.inner.lock().unwrap();

        // check if this was the last instance, in that case we can drop the feedback
        let reset = if let Some(inner) = guard.as_mut() {
            inner.known_instances.retain(|i| i != instance);
            inner.known_instances.is_empty()
        } else {
            false
        };
        if reset {
            *guard = None;
        }
    }
}

#[derive(Debug)]
struct DmabufGlobalState {
    id: GlobalId,

    default_feedback: Option<Arc<Mutex<DmabufFeedback>>>,
    known_default_feedbacks:
        Arc<Mutex<Vec<wayland_server::Weak<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>>>>,
}

/// Delegate type for all dmabuf globals.
///
/// Dmabuf globals are created using this type and events will be forwarded to an instance of the dmabuf global.
#[derive(Debug)]
pub struct DmabufState {
    /// Globals managed by the dmabuf handler.
    globals: HashMap<usize, DmabufGlobalState>,
}

impl DmabufState {
    /// Creates a new [`DmabufState`] delegate type.
    #[allow(clippy::new_without_default)]
    pub fn new() -> DmabufState {
        DmabufState {
            globals: HashMap::new(),
        }
    }

    /// Creates a dmabuf global with the specified supported formats.
    ///
    /// Note: This function will create a version 3 dmabuf global and thus not call [`DmabufHandler::new_surface_feedback`],
    /// if you want to create a version 4 global you need to call [`DmabufState::create_global_with_default_feedback`].
    pub fn create_global<D>(&mut self, display: &DisplayHandle, formats: Vec<Format>) -> DmabufGlobal
    where
        D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
            + BufferHandler
            + DmabufHandler
            + 'static,
    {
        self.create_global_with_filter::<D, _>(display, formats, |_| true)
    }

    /// Creates a dmabuf global with the specified supported formats.
    ///
    /// This function unlike [`DmabufState::create_global`] also allows you to specify a filter function to
    /// determine which clients may see this global. This functionality may be used on multi-gpu systems in
    /// order to make a client choose the correct gpu.
    ///
    /// Note: This function will create a version 3 dmabuf global and thus not call [`DmabufHandler::new_surface_feedback`],
    /// if you want to create a version 4 global you need to call [`DmabufState::create_global_with_filter_and_default_feedback`]
    pub fn create_global_with_filter<D, F>(
        &mut self,
        display: &DisplayHandle,
        formats: Vec<Format>,
        filter: F,
    ) -> DmabufGlobal
    where
        D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
            + BufferHandler
            + DmabufHandler
            + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        self.create_global_with_filter_and_optional_default_feedback::<D, _>(
            display,
            Some(formats),
            None,
            filter,
        )
    }

    /// Creates a dmabuf global with the specified default feedback.
    ///
    /// Clients binding to version 3 or lower will receive the formats from the main tranche.
    pub fn create_global_with_default_feedback<D>(
        &mut self,
        display: &DisplayHandle,
        default_feedback: &DmabufFeedback,
    ) -> DmabufGlobal
    where
        D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
            + BufferHandler
            + DmabufHandler
            + 'static,
    {
        self.create_global_with_filter_and_default_feedback::<D, _>(display, default_feedback, |_| true)
    }

    /// Creates a dmabuf global with the specified supported formats and default feedback
    ///
    /// This function unlike [`DmabufState::create_global_with_default_feedback`] also allows you to specify a filter function to
    /// determine which clients may see this global. This functionality may be used on multi-gpu systems in
    /// order to make a client choose the correct gpu.
    ///
    /// Clients binding to version 3 or lower will receive the formats from the main tranche.
    pub fn create_global_with_filter_and_default_feedback<D, F>(
        &mut self,
        display: &DisplayHandle,
        default_feedback: &DmabufFeedback,
        filter: F,
    ) -> DmabufGlobal
    where
        D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
            + BufferHandler
            + DmabufHandler
            + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        self.create_global_with_filter_and_optional_default_feedback::<D, _>(
            display,
            None,
            Some(default_feedback),
            filter,
        )
    }

    fn create_global_with_filter_and_optional_default_feedback<D, F>(
        &mut self,
        display: &DisplayHandle,
        formats: Option<Vec<Format>>,
        default_feedback: Option<&DmabufFeedback>,
        filter: F,
    ) -> DmabufGlobal
    where
        D: GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, DmabufGlobalData>
            + BufferHandler
            + DmabufHandler
            + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let id = next_global_id();

        let formats = formats
            .or_else(|| default_feedback.map(|f| f.main_formats()))
            .unwrap()
            .into_iter()
            .fold(
                HashMap::<Fourcc, HashSet<Modifier>>::new(),
                |mut formats, format| {
                    if let Some(modifiers) = formats.get_mut(&format.code) {
                        modifiers.insert(format.modifier);
                    } else {
                        formats.insert(format.code, HashSet::from_iter(std::iter::once(format.modifier)));
                    }
                    formats
                },
            );

        let formats = Arc::new(formats);
        let version = if default_feedback.is_some() { 5 } else { 3 };

        let known_default_feedbacks = Arc::new(Mutex::new(Vec::new()));
        let default_feedback = default_feedback.map(|f| Arc::new(Mutex::new(f.clone())));

        let data = DmabufGlobalData {
            filter: Box::new(filter),
            formats,
            default_feedback: default_feedback.clone(),
            known_default_feedbacks: known_default_feedbacks.clone(),
            id,
        };

        let global = display.create_global::<D, zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _>(version, data);
        self.globals.insert(
            id,
            DmabufGlobalState {
                id: global,
                default_feedback,
                known_default_feedbacks,
            },
        );

        DmabufGlobal { id }
    }

    /// Set the default [`DmabufFeedback`] for the specified global and send it to
    /// all known default feedbacks.
    ///
    /// Note: This will do nothing if the global is not found, the global has been
    /// initialized without feedback or the feedback equals the current default feedback.
    pub fn set_default_feedback(&self, global: &DmabufGlobal, default_feedback: &DmabufFeedback) {
        let Some(global) = self.globals.get(&global.id) else {
            return;
        };

        let Some(mut current_feedback) = global.default_feedback.as_ref().map(|f| f.lock().unwrap()) else {
            return;
        };

        // No need to update if the feedback did not change
        if &*current_feedback == default_feedback {
            return;
        }

        let known_default_feedbacks = global.known_default_feedbacks.lock().unwrap();
        for feedback in known_default_feedbacks.iter().filter_map(|f| f.upgrade().ok()) {
            default_feedback.send(&feedback);
        }

        *current_feedback = default_feedback.clone();
    }

    /// Disables a dmabuf global.
    ///
    /// This operation is permanent and there is no way to re-enable a global.
    pub fn disable_global<D: 'static>(&mut self, display: &DisplayHandle, global: &DmabufGlobal) {
        if let Some(global_state) = self.globals.get(&global.id) {
            display.disable_global::<D>(global_state.id.clone());
        }
    }

    /// Destroys a dmabuf global.
    ///
    /// It is highly recommended you disable the global before destroying it and ensure all child objects have
    /// been destroyed.
    pub fn destroy_global<D: 'static>(&mut self, display: &DisplayHandle, global: DmabufGlobal) {
        if DMABUF_GLOBAL_IDS.lock().unwrap().remove(&global.id) {
            if let Some(global_state) = self.globals.remove(&global.id) {
                display.remove_global::<D>(global_state.id);
            }
        }
    }
}

/// Data associated with a dmabuf global.
#[allow(missing_debug_implementations)]
pub struct DmabufGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
    formats: Arc<HashMap<Fourcc, HashSet<Modifier>>>,
    default_feedback: Option<Arc<Mutex<DmabufFeedback>>>,
    known_default_feedbacks:
        Arc<Mutex<Vec<wayland_server::Weak<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>>>>,
    id: usize,
}

/// Data associated with a dmabuf global protocol object.
#[derive(Debug)]
pub struct DmabufData {
    formats: Arc<HashMap<Fourcc, HashSet<Modifier>>>,
    id: usize,

    default_feedback: Option<Arc<Mutex<DmabufFeedback>>>,
    known_default_feedbacks:
        Arc<Mutex<Vec<wayland_server::Weak<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>>>>,
}

/// Data associated with a dmabuf global protocol object.
#[derive(Debug)]
pub struct DmabufFeedbackData {
    known_default_feedbacks:
        Arc<Mutex<Vec<wayland_server::Weak<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>>>>,
    surface: Option<wayland_server::Weak<wayland_server::protocol::wl_surface::WlSurface>>,
}

/// Data associated with a pending [`Dmabuf`] import.
#[derive(Debug)]
pub struct DmabufParamsData {
    /// Id of the dmabuf global these params were created from.
    id: usize,

    /// Whether the params protocol object has been used before to create a wl_buffer.
    used: AtomicBool,

    formats: Arc<HashMap<Fourcc, HashSet<Modifier>>>,

    /// Pending planes for the params.
    modifier: Mutex<Option<Modifier>>,
    planes: Mutex<Vec<Plane>>,
}

/// A handle to a registered dmabuf global.
///
/// This type may be used in equitability checks to determine which global a dmabuf is being imported to.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct DmabufGlobal {
    id: usize,
}

/// An object to allow asynchronous creation of a [`Dmabuf`] backed [`WlBuffer`].
///
/// This object is [`Send`] to allow import of a [`Dmabuf`] to take place on another thread if desired.
#[must_use = "This object must be used to notify the client whether dmabuf import succeeded"]
#[derive(Debug)]
pub struct ImportNotifier {
    inner: ZwpLinuxBufferParamsV1,
    display: DisplayHandle,
    dmabuf: Dmabuf,
    import: Import,
    drop_ignore: bool,
}

/// Type of dmabuf import.
#[derive(Debug)]
enum Import {
    /// The import can fail or create a WlBuffer.
    Falliable,

    /// A WlBuffer object has already been created. Failure causes client death.
    Infallible(WlBuffer),
}

impl ImportNotifier {
    /// Returns the client trying to import this dmabuf, if not dead.
    pub fn client(&self) -> Option<Client> {
        self.inner.client()
    }

    /// Dmabuf import was successful.
    ///
    /// This can return [`InvalidId`] if the client the buffer was imported from has died.
    pub fn successful<D>(mut self) -> Result<WlBuffer, InvalidId>
    where
        D: Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
            + Dispatch<wl_buffer::WlBuffer, Dmabuf>
            + BufferHandler
            + DmabufHandler
            + 'static,
    {
        let client = self.inner.client();

        let result = match self.import {
            Import::Falliable => {
                if let Some(client) = client {
                    match client.create_resource::<wl_buffer::WlBuffer, Dmabuf, D>(
                        &self.display,
                        1,
                        self.dmabuf.clone(),
                    ) {
                        Ok(buffer) => {
                            self.inner.created(&buffer);
                            Ok(buffer)
                        }

                        Err(err) => {
                            tracing::error!("failed to create protocol object for \"create\" request");
                            Err(err)
                        }
                    }
                } else {
                    tracing::error!("client was dead while creating wl_buffer resource");
                    self.inner.post_error(
                        zwp_linux_buffer_params_v1::Error::InvalidWlBuffer,
                        "create_immed failed and produced an invalid wl_buffer",
                    );
                    Err(InvalidId)
                }
            }
            Import::Infallible(ref buffer) => Ok(buffer.clone()),
        };

        self.drop_ignore = true;
        result
    }

    /// The buffer being imported is incomplete.
    ///
    /// This may be the result of too few or too many planes being used when creating a buffer.
    pub fn incomplete(mut self) {
        self.inner.post_error(
            zwp_linux_buffer_params_v1::Error::Incomplete,
            "missing or too many planes to create a buffer",
        );
        self.drop_ignore = true;
    }

    /// The buffer being imported has an invalid width or height.
    pub fn invalid_dimensions(mut self) {
        self.inner.post_error(
            zwp_linux_buffer_params_v1::Error::InvalidDimensions,
            "width or height of dmabuf is invalid",
        );
        self.drop_ignore = true;
    }

    /// Import failed due to an invalid format and plane combination.
    ///
    /// This is always a client error and will result in the client being killed.
    pub fn invalid_format(mut self) {
        self.inner.post_error(
            zwp_linux_buffer_params_v1::Error::InvalidFormat,
            "format and plane combination are not valid",
        );
        self.drop_ignore = true;
    }

    /// Import failed for an implementation dependent reason.
    pub fn failed(mut self) {
        if matches!(self.import, Import::Falliable) {
            self.inner.failed();
        } else {
            self.inner.post_error(
                zwp_linux_buffer_params_v1::Error::InvalidWlBuffer,
                "create_immed failed and produced an invalid wl_buffer",
            );
        }
        self.drop_ignore = true;
    }

    fn new(params: ZwpLinuxBufferParamsV1, display: DisplayHandle, dmabuf: Dmabuf, import: Import) -> Self {
        Self {
            inner: params,
            display,
            dmabuf,
            import,
            drop_ignore: false,
        }
    }
}

impl Drop for ImportNotifier {
    fn drop(&mut self) {
        if !self.drop_ignore {
            tracing::warn!(
                "Compositor bug: Server ignored ImportNotifier for {:?}",
                self.inner
            );
        }
    }
}

/// Handler trait for [`Dmabuf`] import from the compositor.
pub trait DmabufHandler: BufferHandler {
    /// Returns a mutable reference to the [`DmabufState`] delegate type.
    fn dmabuf_state(&mut self) -> &mut DmabufState;

    /// This function is called when a client has imported a [`Dmabuf`].
    ///
    /// The `global` indicates which [`DmabufGlobal`] the buffer was imported to. You should import the dmabuf
    /// into your renderer to ensure the dmabuf may be used later when rendering.
    ///
    /// Whether dmabuf import succeded is notified through the [`ImportNotifier`] object provided in this function.
    fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier);

    /// This function allows to override the default [`DmabufFeedback`] for a surface
    ///
    /// Note: This will only be called if there is no alive surface feedback for the surface.
    /// Normally this will be the first time a surface requests feedback, but can also occur
    /// if all instances have been destroyed and a new surface request is sent by the client.
    ///
    /// Returning `None` will use the default [`DmabufFeedback`] from the global
    fn new_surface_feedback(
        &mut self,
        _surface: &WlSurface,
        _global: &DmabufGlobal,
    ) -> Option<DmabufFeedback> {
        None
    }
}

/// Gets the contents of a [`Dmabuf`] backed [`WlBuffer`].
///
/// If the buffer is managed by the dmabuf handler, the [`Dmabuf`] is returned.
///
/// If the buffer is not managed by the dmabuf handler (whether the buffer is a different kind of buffer,
/// such as an shm buffer or is not managed by smithay), this function will return an [`UnmanagedResource`]
/// error.
///
/// [`WlBuffer`]: wl_buffer::WlBuffer
pub fn get_dmabuf(buffer: &wl_buffer::WlBuffer) -> Result<Dmabuf, UnmanagedResource> {
    buffer.data::<Dmabuf>().cloned().ok_or(UnmanagedResource)
}

/// Macro to delegate implementation of the linux dmabuf to [`DmabufState`].
///
/// You must also implement [`DmabufHandler`] to use this.
#[macro_export]
macro_rules! delegate_dmabuf {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __ZwpLinuxDmabufV1 =
            $crate::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
        type __ZwpLinuxBufferParamsV1 =
            $crate::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1;
        type __ZwpLinuxDmabufFeedbackv1 =
            $crate::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwpLinuxDmabufV1: $crate::wayland::dmabuf::DmabufGlobalData
        ] => $crate::wayland::dmabuf::DmabufState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwpLinuxDmabufV1: $crate::wayland::dmabuf::DmabufData
        ] => $crate::wayland::dmabuf::DmabufState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwpLinuxBufferParamsV1: $crate::wayland::dmabuf::DmabufParamsData
        ] => $crate::wayland::dmabuf::DmabufState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_buffer::WlBuffer: $crate::backend::allocator::dmabuf::Dmabuf
        ] => $crate::wayland::dmabuf::DmabufState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwpLinuxDmabufFeedbackv1: $crate::wayland::dmabuf::DmabufFeedbackData
        ] => $crate::wayland::dmabuf::DmabufState);

    };
}

impl DmabufParamsData {
    /// Emits a protocol error if the params have already been used to create a dmabuf.
    ///
    /// This returns true if the protocol object has not been used.
    fn ensure_unused(&self, params: &ZwpLinuxBufferParamsV1) -> bool {
        if !self.used.load(Ordering::Relaxed) {
            return true;
        }

        params.post_error(
            zwp_linux_buffer_params_v1::Error::AlreadyUsed,
            "This buffer_params has already been used to create a buffer.",
        );

        false
    }

    /// Attempt to create a Dmabuf from the parameters.
    ///
    /// This function will perform the necessary validation of all the parameters, emitting protocol errors as
    /// needed.
    ///
    /// A return value of [`None`] indicates buffer import has failed and the client has been killed.
    fn create_dmabuf(
        &self,
        params: &ZwpLinuxBufferParamsV1,
        width: i32,
        height: i32,
        format: u32,
        flags: WEnum<zwp_linux_buffer_params_v1::Flags>,
        _node: Option<libc::dev_t>,
    ) -> Option<Dmabuf> {
        // We cannot create a dmabuf if the parameters have already been used.
        if !self.ensure_unused(params) {
            return None;
        }

        self.used.store(true, Ordering::Relaxed);

        let format = match Fourcc::try_from(format) {
            Ok(format) => format,
            Err(_) => {
                params.post_error(
                    zwp_linux_buffer_params_v1::Error::InvalidFormat,
                    format!("Format {:x} is not supported", format),
                );

                return None;
            }
        };

        // Validate buffer parameters:
        // 1. Must have known format
        if !self.formats.contains_key(&format) {
            params.post_error(
                zwp_linux_buffer_params_v1::Error::InvalidFormat,
                format!("Format {:?}/{:x} is not supported.", format, format as u32),
            );
            return None;
        }

        // 2. Width and height must be positive
        if width < 1 {
            params.post_error(
                zwp_linux_buffer_params_v1::Error::InvalidDimensions,
                "invalid width",
            );
        }

        if height < 1 {
            params.post_error(
                zwp_linux_buffer_params_v1::Error::InvalidDimensions,
                "invalid height",
            );
        }

        // 3. Validate all the planes
        let mut planes = self.planes.lock().unwrap();

        for plane in &*planes {
            // Must not overflow
            let end = match plane
                .stride
                .checked_mul(height as u32)
                .and_then(|o| o.checked_add(plane.offset))
            {
                Some(e) => e,

                None => {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!("Size overflow for plane {}.", plane.plane_idx),
                    );

                    return None;
                }
            };

            if let Ok(size) = seek(&plane.fd, SeekFrom::End(0)) {
                // Reset seek point
                let _ = seek(&plane.fd, SeekFrom::Start(0));

                if plane.offset as u64 > size {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!("Invalid offset {} for plane {}.", plane.offset, plane.plane_idx),
                    );

                    return None;
                }

                if (plane.offset + plane.stride) as u64 > size {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!("Invalid stride {} for plane {}.", plane.stride, plane.plane_idx),
                    );

                    return None;
                }

                // Planes > 0 can be subsampled, in which case 'size' will be smaller than expected.
                if plane.plane_idx == 0 && end as u64 > size {
                    params.post_error(
                        zwp_linux_buffer_params_v1::Error::OutOfBounds,
                        format!(
                            "Invalid stride ({}) or height ({}) for plane {}.",
                            plane.stride, height, plane.plane_idx
                        ),
                    );

                    return None;
                }
            }
        }

        let modifier = self.modifier.lock().unwrap().unwrap_or(Modifier::Invalid);
        let mut buf = Dmabuf::builder(
            (width, height),
            format,
            modifier,
            DmabufFlags::from_bits_truncate(flags.into()),
        );

        for (i, plane) in planes.drain(..).enumerate() {
            let offset = plane.offset;
            let stride = plane.stride;
            buf.add_plane(plane.into(), i as u32, offset, stride);
        }

        #[cfg(feature = "backend_drm")]
        if let Some(node) = _node.and_then(|node| DrmNode::from_dev_id(node).ok()) {
            buf.set_node(node);
        }

        let dmabuf = match buf.build() {
            Some(buf) => buf,

            None => {
                params.post_error(
                    zwp_linux_buffer_params_v1::Error::Incomplete as u32,
                    "Provided buffer is incomplete, it has zero planes",
                );
                return None;
            }
        };

        Some(dmabuf)
    }
}

id_gen!(next_global_id, DMABUF_GLOBAL_ID, DMABUF_GLOBAL_IDS);
