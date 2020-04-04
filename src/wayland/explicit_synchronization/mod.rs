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
//! # #[macro_use] extern crate smithay;
//! #
//! # use smithay::wayland::compositor::roles::*;
//! # use smithay::wayland::compositor::CompositorToken;
//! use smithay::wayland::explicit_synchronization::*;
//! # define_roles!(MyRoles);
//! #
//! # let mut event_loop = calloop::EventLoop::<()>::new().unwrap();
//! # let mut display = wayland_server::Display::new(event_loop.handle());
//! # let (compositor_token, _, _) = smithay::wayland::compositor::compositor_init::<MyRoles, _, _>(
//! #     &mut display,
//! #     |_, _, _| {},
//! #     None
//! # );
//! init_explicit_synchronization_global(
//!     &mut display,
//!     compositor_token,
//!     None /* You can insert a logger here */
//! );
//! ```
//!
//! Then when handling a surface commit, you can retrieve the synchronization information for the surface
//! data:
//! ```no_run
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! #
//! # use wayland_server::protocol::wl_surface::WlSurface;
//! # use smithay::wayland::compositor::CompositorToken;
//! # use smithay::wayland::explicit_synchronization::*;
//! #
//! # fn dummy_function<R: 'static>(surface: &WlSurface, compositor_token: CompositorToken<R>) {
//! compositor_token.with_surface_data(&surface, |surface_attributes| {
//!     // While you retrieve the surface data from the commit ...
//!     // Check the explicit synchronization data:
//!     match get_explicit_synchronization_state(surface_attributes) {
//!         Ok(sync_state) => {
//!             /* This surface is explicitly synchronized, you need to handle
//!                the contents of sync_state
//!             */
//!         },
//!         Err(()) => {
//!             /* This surface is not explicitly synchronized, nothing more to do
//!             */
//!         }
//!     }
//! });
//! # }
//! ```

use std::{cell::RefCell, ops::{Deref as _, DerefMut as _}, os::unix::io::RawFd};

use wayland_protocols::unstable::linux_explicit_synchronization::v1::server::*;
use wayland_server::{protocol::wl_surface::WlSurface, Display, Filter, Global, Main};

use crate::wayland::compositor::{CompositorToken, SurfaceAttributes};

/// An object to signal end of use of a buffer
pub struct ExplicitBufferRelease {
    release: zwp_linux_buffer_release_v1::ZwpLinuxBufferReleaseV1,
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
/// before acessing the buffer, `release` being `None` means that the client does not require additionnal
/// signaling that you are finished (you still need to send `wl_buffer.release`).
pub struct ExplicitSyncState {
    /// An acquire `dma_fence` object, that you should wait on before accessing the contents of the
    /// buffer associated with the surface.
    pub acquire: Option<RawFd>,
    /// A buffer release object, that you should use to signal the client when you are done using the
    /// buffer associated with the surface.
    pub release: Option<ExplicitBufferRelease>,
}

struct InternalState {
    sync_state: ExplicitSyncState,
    sync_resource: zwp_linux_surface_synchronization_v1::ZwpLinuxSurfaceSynchronizationV1,
}

struct ESUserData {
    state: RefCell<Option<InternalState>>,
}

impl ESUserData {
    fn take_state(&self) -> Option<ExplicitSyncState> {
        if let Some(state) = self.state.borrow_mut().deref_mut() {
            Some(ExplicitSyncState {
                acquire: state.sync_state.acquire.take(),
                release: state.sync_state.release.take(),
            })
        } else {
            None
        }
    }
}

/// Possible errors you can send to an ill-behaving clients
pub enum ExplicitSyncError {
    /// An invalid file descriptor was sent by the client for an acquire fence
    InvalidFence,
    /// The client requested synchronization for a buffer type that does not support it
    UnsupportedBuffer,
    /// The client requested synchronization while not having attached any buffer
    NoBuffer,
}

/// Retrieve the explicit synchronization state commited by the client
///
/// This state can contain an acquire fence and a release object, for synchronization (see module-level docs).
///
/// This function will clear the pending state, preparing the surface for the next commit, as a result you
/// should always call it on surface commit to avoid getting out-of-sync with the client.
///
/// This function returns an error if the client has not setup explicit synchronization for this surface.
pub fn get_explicit_synchronization_state(attrs: &mut SurfaceAttributes) -> Result<ExplicitSyncState, ()> {
    attrs
        .user_data
        .get::<ESUserData>()
        .and_then(|s| s.take_state())
        .ok_or(())
}

/// Send a synchronization error to a client
///
/// See the enum definition for possible errors. These errors are protocol errors, meaning that
/// the client associated with this `SurfaceAttributes` will be killed as a result of calling this
/// function.
pub fn send_explicit_synchronization_error(attrs: &SurfaceAttributes, error: ExplicitSyncError) {
    if let Some(ref data) = attrs.user_data.get::<ESUserData>() {
        if let Some(state) = data.state.borrow().deref() {
            match error {
                ExplicitSyncError::InvalidFence => state.sync_resource.as_ref().post_error(
                    zwp_linux_surface_synchronization_v1::Error::InvalidFence as u32,
                    "The fence specified by the client could not be imported.".into(),
                ),
                ExplicitSyncError::UnsupportedBuffer => state.sync_resource.as_ref().post_error(
                    zwp_linux_surface_synchronization_v1::Error::UnsupportedBuffer as u32,
                    "The buffer does not support explicit synchronization.".into(),
                ),
                ExplicitSyncError::NoBuffer => state.sync_resource.as_ref().post_error(
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
pub fn init_explicit_synchronization_global<R, L>(
    display: &mut Display,
    compositor: CompositorToken<R>,
    logger: L,
) -> Global<zwp_linux_explicit_synchronization_v1::ZwpLinuxExplicitSynchronizationV1>
where
    L: Into<Option<::slog::Logger>>,
    R: 'static,
{
    let _log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "wayland_explicit_synchronization"));

    display.create_global::<zwp_linux_explicit_synchronization_v1::ZwpLinuxExplicitSynchronizationV1, _>(
        2,
        Filter::new(move |(sync, _version): (Main<zwp_linux_explicit_synchronization_v1::ZwpLinuxExplicitSynchronizationV1>, _), _, _| {
            sync.quick_assign(
                move |explicit_sync, req, _| {
                    if let zwp_linux_explicit_synchronization_v1::Request::GetSynchronization {
                        id,
                        surface,
                    } = req
                    {
                        let exists = compositor.with_surface_data(&surface, |attrs| {
                            attrs.user_data.insert_if_missing(|| ESUserData { state: RefCell::new(None) });
                            attrs
                                .user_data
                                .get::<ESUserData>()
                                .map(|ud| ud.state.borrow().is_some())
                                .unwrap()
                        });
                        if exists {
                            explicit_sync.as_ref().post_error(
                                zwp_linux_explicit_synchronization_v1::Error::SynchronizationExists as u32,
                                "The surface already has a synchronization object associated.".into(),
                            );
                            return;
                        }
                        let surface_sync = implement_surface_sync(id, surface.clone(), compositor);
                        compositor.with_surface_data(&surface, |attrs| {
                            let data = attrs.user_data.get::<ESUserData>().unwrap();
                            *data.state.borrow_mut() = Some(InternalState {
                                sync_state: ExplicitSyncState {
                                    acquire: None,
                                    release: None,
                                },
                                sync_resource: surface_sync,
                            });
                        });
                    }
                }
            );
        })
    )
}

fn implement_surface_sync<R>(
    id: Main<zwp_linux_surface_synchronization_v1::ZwpLinuxSurfaceSynchronizationV1>,
    surface: WlSurface,
    compositor: CompositorToken<R>,
) -> zwp_linux_surface_synchronization_v1::ZwpLinuxSurfaceSynchronizationV1
where
    R: 'static,
{
    id.quick_assign(
        move |surface_sync, req, _| match req {
            zwp_linux_surface_synchronization_v1::Request::SetAcquireFence { fd } => {
                if !surface.as_ref().is_alive() {
                    surface_sync.as_ref().post_error(
                        zwp_linux_surface_synchronization_v1::Error::NoSurface as u32,
                        "The associated wl_surface was destroyed.".into(),
                    )
                }
                compositor.with_surface_data(&surface, |attrs| {
                    let data = attrs.user_data.get::<ESUserData>().unwrap();
                    if let Some(state) = data.state.borrow_mut().deref_mut() {
                        if state.sync_state.acquire.is_some() {
                            surface_sync.as_ref().post_error(
                                zwp_linux_surface_synchronization_v1::Error::DuplicateFence as u32,
                                "Multiple fences added for a single surface commit.".into(),
                            )
                        } else {
                            state.sync_state.acquire = Some(fd);
                        }
                    }
                });
            }
            zwp_linux_surface_synchronization_v1::Request::GetRelease { release } => {
                if !surface.as_ref().is_alive() {
                    surface_sync.as_ref().post_error(
                        zwp_linux_surface_synchronization_v1::Error::NoSurface as u32,
                        "The associated wl_surface was destroyed.".into(),
                    )
                }
                compositor.with_surface_data(&surface, |attrs| {
                    let data = attrs.user_data.get::<ESUserData>().unwrap();
                    if let Some(state) = data.state.borrow_mut().deref_mut() {
                        if state.sync_state.acquire.is_some() {
                            surface_sync.as_ref().post_error(
                                zwp_linux_surface_synchronization_v1::Error::DuplicateRelease as u32,
                                "Multiple releases added for a single surface commit.".into(),
                            )
                        } else {
                            release.quick_assign(|_, _, _| {});
                            state.sync_state.release = Some(ExplicitBufferRelease {
                                release: release.deref().clone(),
                            });
                        }
                    }
                });
            }
            zwp_linux_surface_synchronization_v1::Request::Destroy => {
                // disable the ESUserData
                compositor.with_surface_data(&surface, |attrs| {
                    if let Some(ref mut data) = attrs.user_data.get::<ESUserData>() {
                        *data.state.borrow_mut() = None;
                    }
                });
            }
            _ => (),
        },
    );
    id.deref().clone()
}
