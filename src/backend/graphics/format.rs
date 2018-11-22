/// Describes the pixel format of a framebuffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    /// is the format hardware accelerated
    pub hardware_accelerated: bool,
    /// number of bits used for colors
    pub color_bits: u8,
    /// number of bits used for alpha channel
    pub alpha_bits: u8,
    /// number of bits used for depth channel
    pub depth_bits: u8,
    /// number of bits used for stencil buffer
    pub stencil_bits: u8,
    /// is stereoscopy enabled
    pub stereoscopy: bool,
    /// is double buffering enabled
    pub double_buffer: bool,
    /// number of samples used for multisampling if enabled
    pub multisampling: Option<u16>,
    /// is srgb enabled
    pub srgb: bool,
}