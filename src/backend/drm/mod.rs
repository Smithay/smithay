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
//! [`drm-rs`](drm).
//!
//! Four entities are relevant for the initialization procedure.
//!
//! [`crtc`](drm::control::crtc)s represent scanout engines
//! of the device pointer to one framebuffer.
//! Their responsibility is to read the data of the framebuffer and export it into an "Encoder".
//! The number of crtc's represent the number of independant output devices the hardware may handle.
//!
//! An [`encoder`](drm::control::encoder) encodes the data of
//! connected crtcs into a video signal for a fixed set of connectors.
//! E.g. you might have an analog encoder based on a DAG for VGA ports, but another one for digital ones.
//! Also not every encoder might be connected to every crtc.
//!
//! A [`connector`](drm::control::connector) represents a port
//! on your computer, possibly with a connected monitor, TV, capture card, etc.
//!
//! On surface creation a matching encoder for your `encoder`-`connector` is automatically selected,
//! if it exists, which means you still need to check your configuration.
//!
//! At last a [`Mode`](drm::control::Mode) needs to be selected,
//! supported by the `crtc` in question.
//!

use drm::{
    control::{connector, crtc, encoder, framebuffer, plane, Device as ControlDevice, Mode, ResourceHandles},
    Device as BasicDevice, SystemError as DrmError,
};
use nix::libc::dev_t;

use std::error::Error;
use std::iter::IntoIterator;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use calloop::{generic::Generic, InsertError, LoopHandle, Source};

#[cfg(feature = "backend_drm_atomic")]
pub mod atomic;
#[cfg(feature = "backend_drm")]
pub mod common;
#[cfg(feature = "backend_drm_egl")]
pub mod egl;
#[cfg(feature = "backend_drm_gbm")]
pub mod gbm;
#[cfg(feature = "backend_drm_legacy")]
pub mod legacy;

/// Trait to receive events of a bound [`Device`]
///
/// See [`device_bind`]
pub trait DeviceHandler {
    /// The [`Device`] type this handler can handle
    type Device: Device + ?Sized;

    /// A vblank blank event on the provided crtc has happend
    fn vblank(&mut self, crtc: crtc::Handle);
    /// An error happend while processing events
    fn error(&mut self, error: <<<Self as DeviceHandler>::Device as Device>::Surface as Surface>::Error);
}

/// An open drm device
pub trait Device: AsRawFd + DevPath {
    /// Associated [`Surface`] of this [`Device`] type
    type Surface: Surface;

    /// Returns the id of this device node.
    fn device_id(&self) -> dev_t;

    /// Assigns a [`DeviceHandler`] called during event processing.
    ///
    /// See [`device_bind`] and [`DeviceHandler`]
    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static);
    /// Clear a set [`DeviceHandler`](trait.DeviceHandler.html), if any
    fn clear_handler(&mut self);

    /// Creates a new rendering surface.
    ///
    /// # Arguments
    ///
    /// Initialization of surfaces happens through the types provided by
    /// [`drm-rs`](drm).
    ///
    /// - [`crtc`](drm::control::crtc)s represent scanout engines of the device pointing to one framebuffer. \
    ///     Their responsibility is to read the data of the framebuffer and export it into an "Encoder". \
    ///     The number of crtc's represent the number of independant output devices the hardware may handle.
    /// - [`mode`](drm::control::Mode) describes the resolution and rate of images produced by the crtc and \
    ///     has to be compatible with the provided `connectors`.
    /// - [`connectors`] - List of connectors driven by the crtc. At least one(!) connector needs to be \
    ///     attached to a crtc in smithay.
    fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<Self::Surface, <Self::Surface as Surface>::Error>;

    /// Processes any open events of the underlying file descriptor.
    ///
    /// You should not call this function manually, but rather use
    /// [`device_bind`] to register the device
    /// to an [`EventLoop`](calloop::EventLoop)
    /// to synchronize your rendering to the vblank events of the open crtc's
    fn process_events(&mut self);

    /// Attempts to acquire a copy of the [`Device`]'s
    /// [`ResourceHandle`](drm::control::ResourceHandle)
    fn resource_handles(&self) -> Result<ResourceHandles, <Self::Surface as Surface>::Error>;

    /// Retrieve the information for a connector
    fn get_connector_info(&self, conn: connector::Handle) -> Result<connector::Info, DrmError>;

    /// Retrieve the information for a crtc
    fn get_crtc_info(&self, crtc: crtc::Handle) -> Result<crtc::Info, DrmError>;

    /// Retrieve the information for an encoder
    fn get_encoder_info(&self, enc: encoder::Handle) -> Result<encoder::Info, DrmError>;

    /// Retrieve the information for a framebuffer
    fn get_framebuffer_info(&self, fb: framebuffer::Handle) -> Result<framebuffer::Info, DrmError>;

    /// Retrieve the information for a plane
    fn get_plane_info(&self, plane: plane::Handle) -> Result<plane::Info, DrmError>;
}

/// Marker trait for [`Device`]s able to provide [`RawSurface`]s
pub trait RawDevice: Device<Surface = <Self as RawDevice>::Surface> {
    /// Associated [`RawSurface`] of this [`RawDevice`] type
    type Surface: RawSurface;
}

/// An open crtc that can be used for rendering
pub trait Surface {
    /// Type repesenting a collection of
    /// [`connector`](drm::control::connector)s
    /// returned by [`current_connectors`](Surface::current_connectors) and
    /// [`pending_connectors`](Surface::pending_connectors)
    type Connectors: IntoIterator<Item = connector::Handle>;
    /// Error type returned by methods of this trait
    type Error: Error + Send + 'static;

    /// Returns the underlying [`crtc`](drm::control::crtc) of this surface
    fn crtc(&self) -> crtc::Handle;
    /// Currently used [`connector`](drm::control::connector)s of this `Surface`
    fn current_connectors(&self) -> Self::Connectors;
    /// Returns the pending [`connector`](drm::control::connector)s
    /// used after the next [`commit`](RawSurface::commit) of this [`Surface`]
    ///
    /// *Note*: Only on a [`RawSurface`] you may directly trigger
    /// a [`commit`](RawSurface::commit). Other `Surface`s provide their
    /// own methods that *may* trigger a commit, you will need to read their docs.
    fn pending_connectors(&self) -> Self::Connectors;
    /// Tries to add a new [`connector`](drm::control::connector)
    /// to be used after the next commit.
    ///
    /// **Warning**: You need to make sure, that the connector is not used with another surface
    /// or was properly removed via `remove_connector` + `commit` before adding it to another surface.
    /// Behavior if failing to do so is undefined, but might result in rendering errors or the connector
    /// getting removed from the other surface without updating it's internal state.
    ///
    /// Fails if the `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).
    fn add_connector(&self, connector: connector::Handle) -> Result<(), Self::Error>;
    /// Tries to mark a [`connector`](drm::control::connector)
    /// for removal on the next commit.
    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Self::Error>;
    /// Tries to replace the current connector set with the newly provided one on the next commit.
    ///
    /// Fails if one new `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).
    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error>;
    /// Returns the currently active [`Mode`](drm::control::Mode)
    /// of the underlying [`crtc`](drm::control::crtc)
    fn current_mode(&self) -> Mode;
    /// Returns the currently pending [`Mode`](drm::control::Mode)
    /// to be used after the next commit.
    fn pending_mode(&self) -> Mode;
    /// Tries to set a new [`Mode`](drm::control::Mode)
    /// to be used after the next commit.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`](drm::control::crtc) or any of the
    /// pending [`connector`](drm::control::connector)s.
    ///
    /// *Note*: Only on a [`RawSurface`] you may directly trigger
    /// a [`commit`](RawSurface::commit). Other [`Surface`]s provide their
    /// own methods that *may* trigger a commit, you will need to read their docs.
    fn use_mode(&self, mode: Mode) -> Result<(), Self::Error>;
}

/// An open bare crtc without any rendering abstractions
pub trait RawSurface: Surface + ControlDevice + BasicDevice {
    /// Returns true whenever any state changes are pending to be commited
    ///
    /// The following functions may trigger a pending commit:
    /// - [`add_connector`](Surface::add_connector)
    /// - [`remove_connector`](Surface::remove_connector)
    /// - [`use_mode`](Surface::use_mode)
    fn commit_pending(&self) -> bool;
    /// Commit the pending state rendering a given framebuffer.
    ///
    /// *Note*: This will trigger a full modeset on the underlying device,
    /// potentially causing some flickering. Check before performing this
    /// operation if a commit really is necessary using [`commit_pending`](RawSurface::commit_pending).
    ///
    /// This operation is not necessarily blocking until the crtc is in the desired state,
    /// but will trigger a `vblank` event once done.
    /// Make sure to [set a `DeviceHandler`](Device::set_handler) and
    /// [register the belonging `Device`](device_bind) before to receive the event in time.
    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<(), <Self as Surface>::Error>;
    /// Page-flip the underlying [`crtc`](drm::control::crtc)
    /// to a new given [`framebuffer`].
    ///
    /// This will not cause the crtc to modeset.
    ///
    /// This operation is not blocking and will produce a `vblank` event once swapping is done.
    /// Make sure to [set a `DeviceHandler`](Device::set_handler) and
    /// [register the belonging `Device`](device_bind) before to receive the event in time.
    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), <Self as Surface>::Error>;
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

/// calloop source associated with a Device
pub type DrmSource<D> = Generic<D>;

/// Bind a `Device` to an [`EventLoop`](calloop::EventLoop),
///
/// This will cause it to recieve events and feed them into a previously
/// set [`DeviceHandler`](DeviceHandler).
pub fn device_bind<D: Device + 'static, Data>(
    handle: &LoopHandle<Data>,
    device: D,
) -> ::std::result::Result<Source<DrmSource<D>>, InsertError<DrmSource<D>>>
where
    D: Device,
    Data: 'static,
{
    let source = Generic::new(device, calloop::Interest::Readable, calloop::Mode::Level);

    handle.insert_source(source, |_, source, _| {
        source.process_events();
        Ok(())
    })
}
