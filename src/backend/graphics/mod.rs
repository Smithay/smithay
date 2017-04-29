//! Common traits for various ways to renderer on a given graphics backend.
//!
//! Note: Not every api may be supported by every backend

/// General functions any graphics backend should support independently from it's rendering
/// techique.
pub trait GraphicsBackend {
    /// Format representing the image drawn for the cursor.
    type CursorFormat;

    /// Sets the cursor position and therefor updates the drawn cursors position.
    /// Useful as well for e.g. pointer wrapping.
    ///
    /// Not guaranteed to be supported on every backend. The result usually
    /// depends on the backend, the cursor might be "owned" by another more priviledged
    /// compositor (running nested).
    ///
    /// In these cases setting the position is actually not required, as movement is done
    /// by the higher compositor and not by the backend. It is still good practice to update
    /// the position after every recieved event, but don't rely on pointer wrapping working.
    ///
    fn set_cursor_position(&mut self, x: u32, y: u32) -> Result<(), ()>;

    /// Set the cursor drawn on the `GraphicsBackend`.
    ///
    /// The format is entirely dictated by the concrete implementation and might range
    /// from raw image buffers over a fixed list of possible cursor types to simply the
    /// void type () to represent no possible customization of the cursor itself.
    fn set_cursor_representation(&mut self, cursor: Self::CursorFormat);
}

pub mod software;
pub mod opengl;
