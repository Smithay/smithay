//!
//!
//! This module provides Traits reprensentating open devices
//! and their surfaces to render contents.
//!
//! ---
//!
//! Initialization of devices happens through an open file descriptor
//! of a drm device.
//!
//! ---
//!
//! Initialization of surfaces happens through the types provided by
//! [`drm-rs`](https://docs.rs/drm/0.3.4/drm/).
//!
//! Four entities are relevant for the initialization procedure.
//!
//! [`crtc`](https://docs.rs/drm/0.3.4/drm/control/crtc/index.html)s represent scanout engines
//! of the device pointer to one framebuffer.
//! Their responsibility is to read the data of the framebuffer and export it into an "Encoder".
//! The number of crtc's represent the number of independant output devices the hardware may handle.
//!
//! An [`encoder`](https://docs.rs/drm/0.3.4/drm/control/encoder/index.html) encodes the data of
//! connected crtcs into a video signal for a fixed set of connectors.
//! E.g. you might have an analog encoder based on a DAG for VGA ports, but another one for digital ones.
//! Also not every encoder might be connected to every crtc.
//!
//! A [`connector`](https://docs.rs/drm/0.3.4/drm/control/connector/index.html) represents a port
//! on your computer, possibly with a connected monitor, TV, capture card, etc.
//!
//! On surface creation a matching encoder for your `encoder`-`connector` is automatically selected,
//! if it exists, which means you still need to check your configuration.
//!
//! At last a [`Mode`](https://docs.rs/drm/0.3.4/drm/control/struct.Mode.html) needs to be selected,
//! supported by the `crtc` in question.
//!

pub use drm::{
    Device as BasicDevice,
    buffer::Buffer,
    control::{connector, crtc, framebuffer, Mode, ResourceHandles, ResourceInfo, Device as ControlDevice},
};
pub use nix::libc::dev_t;

use std::error::Error;
use std::iter::IntoIterator;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use wayland_server::calloop::generic::{EventedFd, Generic};
use wayland_server::calloop::mio::Ready;
pub use wayland_server::calloop::InsertError;
use wayland_server::calloop::{LoopHandle, Source};

use super::graphics::SwapBuffersError;

#[cfg(feature = "backend_drm_egl")]
pub mod egl;
#[cfg(feature = "backend_drm_gbm")]
pub mod gbm;
#[cfg(feature = "backend_drm_legacy")]
pub mod legacy;

/// Trait to receive events of a bound [`Device`](trait.Device.html)
///
/// See [`device_bind`](fn.device_bind.html)
pub trait DeviceHandler {
    /// The [`Device`](trait.Device.html) type this handler can handle
    type Device: Device + ?Sized;

    /// A vblank blank event on the provided crtc has happend
    fn vblank(&mut self, crtc: crtc::Handle);
    /// An error happend while processing events
    fn error(&mut self, error: <<<Self as DeviceHandler>::Device as Device>::Surface as Surface>::Error);
}

/// An open drm device
pub trait Device: AsRawFd + DevPath {
    /// Associated [`Surface`](trait.Surface.html) of this `Device` type
    type Surface: Surface;

    /// Returns the `id` of this device node.
    fn device_id(&self) -> dev_t;

    /// Assigns a `DeviceHandler` called during event processing.
    ///
    /// See [`device_bind`](fn.device_bind.html) and [`DeviceHandler`](trait.DeviceHandler.html)
    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static);
    /// Clear a set [`DeviceHandler`](trait.DeviceHandler.html), if any
    fn clear_handler(&mut self);

    /// Creates a new rendering surface.
    ///
    /// Initialization of surfaces happens through the types provided by
    /// [`drm-rs`](https://docs.rs/drm/0.3.4/drm/).
    ///
    /// [`crtc`](https://docs.rs/drm/0.3.4/drm/control/crtc/index.html)s represent scanout engines
    /// of the device pointer to one framebuffer.
    /// Their responsibility is to read the data of the framebuffer and export it into an "Encoder".
    /// The number of crtc's represent the number of independant output devices the hardware may handle.
    fn create_surface(
        &mut self,
        ctrc: crtc::Handle,
    ) -> Result<Self::Surface, <Self::Surface as Surface>::Error>;

    /// Processes any open events of the underlying file descriptor.
    ///
    /// You should not call this function manually, but rather use
    /// [`device_bind`](fn.device_bind.html) to register the device
    /// to an [`EventLoop`](https://docs.rs/calloop/0.4.2/calloop/struct.EventLoop.html)
    /// to synchronize your rendering to the vblank events of the open crtc's
    fn process_events(&mut self);

    /// Load the resource from a `Device` given its
    /// [`ResourceHandle`](https://docs.rs/drm/0.3.4/drm/control/trait.ResourceHandle.html)
    fn resource_info<T: ResourceInfo>(
        &self,
        handle: T::Handle,
    ) -> Result<T, <Self::Surface as Surface>::Error>;

    /// Attempts to acquire a copy of the `Device`'s
    /// [`ResourceHandles`](https://docs.rs/drm/0.3.4/drm/control/struct.ResourceHandles.html)
    fn resource_handles(&self) -> Result<ResourceHandles, <Self::Surface as Surface>::Error>;
}

/// Marker trait for `Device`s able to provide [`RawSurface`](trait.RawSurface.html)s
pub trait RawDevice: Device<Surface = <Self as RawDevice>::Surface> {
    /// Associated [`RawSurface`](trait.RawSurface.html) of this `RawDevice` type
    type Surface: RawSurface;
}

/// An open crtc that can be used for rendering
pub trait Surface {
    /// Type repesenting a collection of
    /// [`connector`](https://docs.rs/drm/0.3.4/drm/control/connector/index.html)s
    /// returned by [`current_connectors`](#method.current_connectors) and
    /// [`pending_connectors`](#method.pending_connectors)
    type Connectors: IntoIterator<Item = connector::Handle>;
    /// Error type returned by methods of this trait
    type Error: Error + Send;

    /// Returns the underlying [`crtc`](https://docs.rs/drm/0.3.4/drm/control/crtc/index.html) of this surface
    fn crtc(&self) -> crtc::Handle;
    /// Currently used [`connector`](https://docs.rs/drm/0.3.4/drm/control/connector/index.html)s of this `Surface`
    fn current_connectors(&self) -> Self::Connectors;
    /// Returns the pending [`connector`](https://docs.rs/drm/0.3.4/drm/control/connector/index.html)s
    /// used after the next `commit` of this `Surface`
    ///
    /// *Note*: Only on a [`RawSurface`](trait.RawSurface.html) you may directly trigger
    /// a [`commit`](trait.RawSurface.html#method.commit). Other `Surface`s provide their
    /// own methods that *may* trigger a commit, you will need to read their docs.
    fn pending_connectors(&self) -> Self::Connectors;
    /// Tries to add a new [`connector`](https://docs.rs/drm/0.3.4/drm/control/connector/index.html)
    /// to be used after the next commit.
    ///
    /// Fails if the `connector` is not compatible with the underlying [`crtc`](https://docs.rs/drm/0.3.4/drm/control/crtc/index.html)
    /// (e.g. no suitable [`encoder`](https://docs.rs/drm/0.3.4/drm/control/encoder/index.html) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](https://docs.rs/drm/0.3.4/drm/control/struct.Mode.html).
    fn add_connector(&self, connector: connector::Handle) -> Result<(), Self::Error>;
    /// Tries to mark a [`connector`](https://docs.rs/drm/0.3.4/drm/control/connector/index.html)
    /// for removal on the next commit.
    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Self::Error>;
    /// Returns the currently active [`Mode`](https://docs.rs/drm/0.3.4/drm/control/struct.Mode.html)
    /// of the underlying [`crtc`](https://docs.rs/drm/0.3.4/drm/control/crtc/index.html)
    /// if any.
    fn current_mode(&self) -> Option<Mode>;
    /// Returns the currently pending [`Mode`](https://docs.rs/drm/0.3.4/drm/control/struct.Mode.html)
    /// to be used after the next commit, if any.
    fn pending_mode(&self) -> Option<Mode>;
    /// Tries to set a new [`Mode`](https://docs.rs/drm/0.3.4/drm/control/struct.Mode.html)
    /// to be used after the next commit.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`](https://docs.rs/drm/0.3.4/drm/control/crtc/index.html) or any of the
    /// pending [`connector`](https://docs.rs/drm/0.3.4/drm/control/connector/index.html)s.
    ///
    /// *Note*: Only on a [`RawSurface`](trait.RawSurface.html) you may directly trigger
    /// a [`commit`](trait.RawSurface.html#method.commit). Other `Surface`s provide their
    /// own methods that *may* trigger a commit, you will need to read their docs.
    fn use_mode(&self, mode: Option<Mode>) -> Result<(), Self::Error>;
}

/// An open bare crtc without any rendering abstractions
pub trait RawSurface: Surface + ControlDevice + BasicDevice {
    /// Returns true whenever any state changes are pending to be commited
    ///
    /// The following functions may trigger a pending commit:
    /// - [`add_connector`](trait.Surface.html#method.add_connector)
    /// - [`remove_connector`](trait.Surface.html#method.remove_connector)
    /// - [`use_mode`](trait.Surface.html#method.use_mode)
    fn commit_pending(&self) -> bool;
    /// Commit the pending state rendering a given framebuffer.
    ///
    /// *Note*: This will trigger a full modeset on the underlying device,
    /// potentially causing some flickering. Check before performing this
    /// operation if a commit really is necessary using [`commit_pending`](#method.commit_pending).
    ///
    /// This operation is blocking until the crtc is in the desired state.
    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<(), <Self as Surface>::Error>;
    /// Page-flip the underlying [`crtc`](https://docs.rs/drm/0.3.4/drm/control/crtc/index.html)
    /// to a new given [`framebuffer`].
    ///
    /// This will not cause the crtc to modeset.
    ///
    /// This operation is not blocking and will produce a `vblank` event once swapping is done.
    /// Make sure to [set a `DeviceHandler`](trait.Device.html#method.set_handler) and
    /// [register the belonging `Device`](fn.device_bind.html) before to receive the event in time.
    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), SwapBuffersError>;
}

/// Trait representing open devices that *may* return a `Path`
pub trait DevPath {
    /// Returns the path of the open device if possible
    fn dev_path(&self) -> Option<PathBuf>;
}

impl<A: AsRawFd> DevPath for A {
    fn dev_path(&self) -> Option<PathBuf> {
        use std::fs;

        fs::read_link(format!("/proc/self/fd/{:?}", self.as_raw_fd())).ok()
    }
}

/// Bind a `Device` to an `EventLoop`,
///
/// This will cause it to recieve events and feed them into a previously
/// set [`DeviceHandler`](trait.DeviceHandler.html).
pub fn device_bind<D: Device + 'static, Data>(
    handle: &LoopHandle<Data>,
    device: D,
) -> ::std::result::Result<Source<Generic<EventedFd<D>>>, InsertError<Generic<EventedFd<D>>>>
where
    D: Device,
    Data: 'static,
{
    let mut source = Generic::from_fd_source(device);
    source.set_interest(Ready::readable());

    handle.insert_source(source, |evt, _| {
        evt.source.borrow_mut().0.process_events();
    })
}
