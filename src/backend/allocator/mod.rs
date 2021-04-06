#[cfg(feature = "backend_gbm")]
pub mod gbm;
#[cfg(feature = "backend_drm")]
pub mod dumb;
pub mod dmabuf;

mod swapchain;
pub use swapchain::{Slot, Swapchain};

pub use drm_fourcc::{DrmFormat as Format, DrmFourcc as Fourcc, DrmModifier as Modifier, DrmVendor as Vendor, UnrecognizedFourcc, UnrecognizedVendor};

pub trait Buffer {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn size(&self) -> (u32, u32) { (self.width(), self.height()) }
    fn format(&self) -> Format;
}

pub trait Allocator<B: Buffer> {
    type Error: std::error::Error;

    fn create_buffer(&mut self, width: u32, height: u32, format: Format) -> Result<B, Self::Error>;
}