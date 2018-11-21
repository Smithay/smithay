use drm::Device as BasicDevice;
use drm::control::Device as ControlDevice;
pub use drm::control::crtc;
pub use drm::control::connector;
pub use drm::control::framebuffer;
pub use drm::control::Mode;

use std::borrow::Borrow;
use std::error::Error;
use std::path::PathBuf;
use std::os::unix::io::AsRawFd;

use wayland_server::calloop::generic::{EventedFd, Generic};
use wayland_server::calloop::{LoopHandle, Source};
use wayland_server::calloop::mio::Ready;
pub use wayland_server::calloop::InsertError;

use super::graphics::SwapBuffersError;

#[cfg(feature = "backend_drm_legacy")]
pub mod legacy;
#[cfg(feature = "backend_drm_gbm")]
pub mod gbm;
#[cfg(feature = "backend_drm_egl")]
pub mod egl;

pub trait DeviceHandler {
    type Device: Device + ?Sized;
    fn vblank(&mut self, surface: &<<Self as DeviceHandler>::Device as Device>::Surface);
    fn error(&mut self, error: <<<Self as DeviceHandler>::Device as Device>::Surface as Surface>::Error);
}

pub trait Device: AsRawFd + DevPath {
    type Surface: Surface;
    type Return: Borrow<Self::Surface>;

    fn set_handler(&mut self, handler: impl DeviceHandler<Device=Self> + 'static);
    fn clear_handler(&mut self);
    fn create_surface(
        &mut self,
        ctrc: crtc::Handle,
        mode: Mode,
        connectors: impl Into<<Self::Surface as Surface>::Connectors>
    ) -> Result<Self::Return, <Self::Surface as Surface>::Error>;
    fn process_events(&mut self);
}

pub trait RawDevice: Device<Surface=<Self as RawDevice>::Surface>
where
    <Self as Device>::Return: Borrow<<Self as RawDevice>::Surface>
{
    type Surface: RawSurface;
}

pub trait Surface {
    type Connectors: IntoIterator<Item=connector::Handle>;
    type Error: Error + Send;

    fn crtc(&self) -> crtc::Handle;
    fn current_connectors(&self) -> Self::Connectors;
    fn pending_connectors(&self) -> Self::Connectors;
    fn add_connector(&self, connector: connector::Handle) -> Result<(), Self::Error>;
    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Self::Error>;
    fn current_mode(&self) -> Mode;
    fn pending_mode(&self) -> Mode;
    fn use_mode(&self, mode: Mode) -> Result<(), Self::Error>;
}

pub trait RawSurface: Surface + ControlDevice + BasicDevice {
    fn commit_pending(&self) -> bool;
    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<(), <Self as Surface>::Error>;
    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), SwapBuffersError>;
} 

/// Trait for types representing open devices
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
/// This will cause it to recieve events and feed them into an `DeviceHandler`
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
