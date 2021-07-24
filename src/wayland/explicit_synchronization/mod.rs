//! Explicit buffer synchronization per wayland surface
//!
//! This interface allow clients to switch from a per-buffer signaling of buffer release (via the
//! `wl_buffer.release` event) to a per-surface signaling using `dma_fence`s. This is notably used for
//! efficient synchronization of OpenGL/Vulkan clients.
//!
//! At surface commit time, in addition to a buffer the client can have attached two more properties:
//!
//! - an acquire `dma_fence` file descriptor, that the compositor is required to wait on before it will
//!   try to access the contents of the associated buffer
//! - an `ExplicitBufferRelease` object, that the compositor is expect to use to signal the client when it has
//!   finished using the buffer for this surface (if the same buffer is attached to multiple surfaces, the
//!   release only applies for the surface associated with this release object, not the whole buffer).
//!
//! The use of these `dma_fence`s in conjunction with the graphics stack allows for efficient synchronization
//! between the clients and the compositor.
//!
//! ## Usage
//!
//! First, you need to initialize the global:
//!
//! ```
//! # extern crate wayland_server;
//! use smithay::wayland::explicit_synchronization::*;
//! # let mut display = wayland_server::Display::new();
//! init_explicit_synchronization_global(
//!     &mut display,
//!     None /* You can insert a logger here */
//! );
//! ```
//!
//! Then when handling a surface commit, you can retrieve the synchronization information for the surface states:
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! #
//! # use wayland_server::protocol::wl_surface::WlSurface;
//! # use smithay::wayland::explicit_synchronization::*;
//! #
//! # fn dummy_function<R: 'static>(surface: &WlSurface) {
//! use smithay::wayland::compositor::with_states;
//! with_states(&surface, |states| {
//!     let explicit_sync_state = states.cached_state.current::<ExplicitSyncState>();
//!     /* process the explicit_sync_state */
//! });
//! # }
//! ```

use std::{cell::RefCell, ops::Deref as _, os::unix::io::RawFd};

use wayland_protocols::unstable::linux_explicit_synchronization::v1::server::{
    zwp_linux_buffer_release_v1::ZwpLinuxBufferReleaseV1,
    zwp_linux_explicit_synchronization_v1::{self, ZwpLinuxExplicitSynchronizationV1},
    zwp_linux_surface_synchronization_v1::{self, ZwpLinuxSurfaceSynchronizationV1},
};
use wayland_server::{protocol::wl_surface::WlSurface, Display, Filter, Global, Main};

use super::compositor::{with_states, Cacheable, SurfaceData};

/// An object to signal end of use of a buffer
#[derive(Debug)]
pub struct ExplicitBufferRelease {
    release: ZwpLinuxBufferReleaseV1,
}

impl ExplicitBufferRelease {
    /// Immediately release the buffer
    ///
    /// The client can reuse it as soon as this event is sent.
    pub fn immediate_release(self) {
        self.release.immediate_release();
    }

    /// Send a release fence to the client
    ///
    /// The client will be allowed to reuse the buffer once you signal this `dma_fence`.
    pub fn send_release_fence(self, fence: RawFd) {
        self.release.fenced_release(fence);
    }
}

/// An explicit synchronization state
///
/// The client is not required to fill both. `acquire` being `None` means that you don't need to wait
/// before accessing the buffer, `release` being `None` means that the client does not require additional
/// signaling that you are finished (you still need to send `wl_buffer.release`).
///
/// When processing the current state, [`Option::take`] the values from it. Otherwise they'll be
/// treated as unused and released when overwritten by the next client commit.
#[derive(Debug)]
pub struct ExplicitSyncState {
    /// An acquire `dma_fence` object, that you should wait on before accessing the contents of the
    /// buffer associated with the surface.
    pub acquire: Option<RawFd>,
    /// A buffer release object, that you should use to signal the client when you are done using the
    /// buffer associated with the surface.
    pub release: Option<ExplicitBufferRelease>,
}

impl Default for ExplicitSyncState {
    fn default() -> Self {
        ExplicitSyncState {
            acquire: None,
            release: None,
        }
    }
}

impl Cacheable for ExplicitSyncState {
    fn commit(&mut self) -> Self {
        std::mem::take(self)
    }
    fn merge_into(mut self, into: &mut Self) {
        if self.acquire.is_some() {
            if let Some(fd) = std::mem::replace(&mut into.acquire, self.acquire.take()) {
                // close the unused fd
                let _ = nix::unistd::close(fd);
            }
        }
        if self.release.is_some() {
            if let Some(release) = std::mem::replace(&mut into.release, self.release.take()) {
                // release the overriden state
                release.immediate_release();
            }
        }
    }
}

struct ESUserData {
    state: RefCell<Option<ZwpLinuxSurfaceSynchronizationV1>>,
}

/// Possible errors you can send to an ill-behaving clients
#[derive(Debug)]
pub enum ExplicitSyncError {
    /// An invalid file descriptor was sent by the client for an acquire fence
    InvalidFence,
    /// The client requested synchronization for a buffer type that does not support it
    UnsupportedBuffer,
    /// The client requested synchronization while not having attached any buffer
    NoBuffer,
}

/// This surface is not explicitly synchronized
#[derive(Debug)]
pub struct NoExplicitSync;

impl std::fmt::Display for NoExplicitSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The surface is not explicitly synchronized.")
    }
}

impl std::error::Error for NoExplicitSync {}

/// Send a synchronization error to a client
///
/// See the enum definition for possible errors. These errors are protocol errors, meaning that
/// the client associated with this `SurfaceAttributes` will be killed as a result of calling this
/// function.
pub fn send_explicit_synchronization_error(attrs: &SurfaceData, error: ExplicitSyncError) {
    if let Some(ref data) = attrs.data_map.get::<ESUserData>() {
        if let Some(sync_resource) = data.state.borrow().deref() {
            match error {
                ExplicitSyncError::InvalidFence => sync_resource.as_ref().post_error(
                    zwp_linux_surface_synchronization_v1::Error::InvalidFence as u32,
                    "The fence specified by the client could not be imported.".into(),
                ),
                ExplicitSyncError::UnsupportedBuffer => sync_resource.as_ref().post_error(
                    zwp_linux_surface_synchronization_v1::Error::UnsupportedBuffer as u32,
                    "The buffer does not support explicit synchronization.".into(),
                ),
                ExplicitSyncError::NoBuffer => sync_resource.as_ref().post_error(
                    zwp_linux_surface_synchronization_v1::Error::NoBuffer as u32,
                    "No buffer was attached.".into(),
                ),
            }
        }
    }
}

/// Initialize the explicit synchronization global
///
/// See module-level documentation for its use.
pub fn init_explicit_synchronization_global<L>(
    display: &mut Display,
    logger: L,
) -> Global<ZwpLinuxExplicitSynchronizationV1>
where
    L: Into<Option<::slog::Logger>>,
{
    let _log =
        crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "wayland_explicit_synchronization"));

    display.create_global::<ZwpLinuxExplicitSynchronizationV1, _>(
        2,
        Filter::new(
            move |(sync, _version): (Main<ZwpLinuxExplicitSynchronizationV1>, _), _, _| {
                sync.quick_assign(move |explicit_sync, req, _| {
                    if let zwp_linux_explicit_synchronization_v1::Request::GetSynchronization {
                        id,
                        surface,
                    } = req
                    {
                        let exists = with_states(&surface, |states| {
                            states.data_map.insert_if_missing(|| ESUserData {
                                state: RefCell::new(None),
                            });
                            states
                                .data_map
                                .get::<ESUserData>()
                                .map(|ud| ud.state.borrow().is_some())
                                .unwrap()
                        })
                        .unwrap_or(false);
                        if exists {
                            explicit_sync.as_ref().post_error(
                                zwp_linux_explicit_synchronization_v1::Error::SynchronizationExists as u32,
                                "The surface already has a synchronization object associated.".into(),
                            );
                            return;
                        }
                        let surface_sync = implement_surface_sync(id, surface.clone());
                        with_states(&surface, |states| {
                            let data = states.data_map.get::<ESUserData>().unwrap();
                            *data.state.borrow_mut() = Some(surface_sync);
                        })
                        .unwrap();
                    }
                });
            },
        ),
    )
}

fn implement_surface_sync(
    id: Main<ZwpLinuxSurfaceSynchronizationV1>,
    surface: WlSurface,
) -> ZwpLinuxSurfaceSynchronizationV1 {
    id.quick_assign(move |surface_sync, req, _| match req {
        zwp_linux_surface_synchronization_v1::Request::SetAcquireFence { fd } => {
            if !surface.as_ref().is_alive() {
                surface_sync.as_ref().post_error(
                    zwp_linux_surface_synchronization_v1::Error::NoSurface as u32,
                    "The associated wl_surface was destroyed.".into(),
                )
            }
            with_states(&surface, |states| {
                let mut pending = states.cached_state.pending::<ExplicitSyncState>();
                if pending.acquire.is_some() {
                    surface_sync.as_ref().post_error(
                        zwp_linux_surface_synchronization_v1::Error::DuplicateFence as u32,
                        "Multiple fences added for a single surface commit.".into(),
                    )
                } else {
                    pending.acquire = Some(fd);
                }
            })
            .unwrap();
        }
        zwp_linux_surface_synchronization_v1::Request::GetRelease { release } => {
            if !surface.as_ref().is_alive() {
                surface_sync.as_ref().post_error(
                    zwp_linux_surface_synchronization_v1::Error::NoSurface as u32,
                    "The associated wl_surface was destroyed.".into(),
                )
            }
            with_states(&surface, |states| {
                let mut pending = states.cached_state.pending::<ExplicitSyncState>();
                if pending.release.is_some() {
                    surface_sync.as_ref().post_error(
                        zwp_linux_surface_synchronization_v1::Error::DuplicateRelease as u32,
                        "Multiple releases added for a single surface commit.".into(),
                    )
                } else {
                    release.quick_assign(|_, _, _| {});
                    pending.release = Some(ExplicitBufferRelease {
                        release: release.deref().clone(),
                    });
                }
            })
            .unwrap();
        }
        zwp_linux_surface_synchronization_v1::Request::Destroy => {
            // disable the ESUserData
            with_states(&surface, |states| {
                if let Some(ref mut data) = states.data_map.get::<ESUserData>() {
                    *data.state.borrow_mut() = None;
                }
            })
            .unwrap();
        }
        _ => (),
    });
    id.deref().clone()
}
