use wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_server::{
    backend::GlobalId, protocol::wl_buffer::WlBuffer, Dispatch, DisplayHandle, GlobalDispatch, Resource,
};

mod handlers;

/// Delegate state of WpSinglePixelBuffer protocol
#[derive(Debug)]
pub struct SinglePixelBufferState {
    global: GlobalId,
}

impl SinglePixelBufferState {
    /// Create a new [`WpSinglePixelBufferManagerV1`] global
    //
    /// The id provided by [`SinglePixelBufferState::global`] may be used to
    /// remove or disable this global in the future.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WpSinglePixelBufferManagerV1, ()>,
        D: Dispatch<WpSinglePixelBufferManagerV1, ()>,
        D: 'static,
    {
        let global = display.create_global::<D, WpSinglePixelBufferManagerV1, _>(1, ());

        Self { global }
    }

    /// Returns the id of the [`WpSinglePixelBufferManagerV1`] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// User data of `WlBuffer` backed by single pixel
#[derive(Debug)]
pub struct SinglePixelBufferUserData {
    /// Value of the buffer's red channel
    pub r: u32,
    /// Value of the buffer's green channel
    pub g: u32,
    /// Value of the buffer's blue channel
    pub b: u32,
    /// Value of the buffer's alpha channel
    pub a: u32,
}

impl SinglePixelBufferUserData {
    /// Check if pixel has alpha
    pub fn has_alpha(&self) -> bool {
        self.a != u32::MAX
    }

    /// RGAB8888 color buffer
    pub fn rgba8888(&self) -> [u8; 4] {
        let divisor = u32::MAX / 255;

        [
            (self.r / divisor) as u8,
            (self.g / divisor) as u8,
            (self.b / divisor) as u8,
            (self.a / divisor) as u8,
        ]
    }
}

/// Error that can occur when accessing an SinglePixelBuffer
#[derive(Debug, thiserror::Error)]
pub enum BufferAccessError {
    /// This buffer is not managed by the SinglePixelBuffer handler
    #[error("non-single-pixel buffer")]
    NotManaged,
}

/// Gets the data of a `SinglePixelBuffer` backed [`WlBuffer`].
pub fn get_single_pixel_buffer(buffer: &WlBuffer) -> Result<&SinglePixelBufferUserData, BufferAccessError> {
    buffer
        .data::<SinglePixelBufferUserData>()
        .ok_or(BufferAccessError::NotManaged)
}

/// Macro used to delegate `WpSinglePixelBuffer` events
#[macro_export]
macro_rules! delegate_single_pixel_buffer {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1: ()
        ] => $crate::wayland::single_pixel_buffer::SinglePixelBufferState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1: ()
        ] => $crate::wayland::single_pixel_buffer::SinglePixelBufferState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_buffer::WlBuffer: $crate::wayland::single_pixel_buffer::SinglePixelBufferUserData
        ] => $crate::wayland::single_pixel_buffer::SinglePixelBufferState);
    };
}
